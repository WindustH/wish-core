//! Independent agent Chromium and attached native device browser targets.
mod actions;
mod client;
use super::{attachments, error::ApplicationError as Error, session::SessionSlot};
use crate::{
  protocol::{ContentBlock, Tool},
  session::ToolOutcome,
};
use chromiumoxide::{Browser, BrowserConfig, Page, page::ScreenshotParams};
pub use client::ClientConnection;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};
use tokio::sync::Mutex;
use uuid::Uuid;
type Result<T> = std::result::Result<T, Error>;

struct Profile(PathBuf, tokio_util::task::TaskTracker);
impl Drop for Profile {
  fn drop(&mut self) {
    let path = std::mem::take(&mut self.0);
    self.1.spawn_blocking(move || {
      if let Err(error) = std::fs::remove_dir_all(&path)
        && error.kind() != std::io::ErrorKind::NotFound
      {
        eprintln!("remove browser profile {}: {error}", path.display());
      }
    });
  }
}
struct Instance {
  browser: Browser,
  page: Page,
  _profile: Profile,
  _permit: tokio::sync::OwnedSemaphorePermit,
  handler: tokio::task::JoinHandle<()>,
  used: std::time::Instant,
}
impl Drop for Instance {
  fn drop(&mut self) {
    self.handler.abort();
  }
}
impl Instance {
  async fn launch(
    executable: &str,
    proxy: &super::config::ProxyConfig,
    tasks: tokio_util::task::TaskTracker,
  ) -> Result<Self> {
    static SLOTS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
      std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(6)));
    let permit = SLOTS
      .clone()
      .try_acquire_owned()
      .map_err(|_| Error::conflict("all server browser slots are in use"))?;
    let profile =
      Profile(std::env::temp_dir().join(format!("wish-browser-{}", Uuid::new_v4())), tasks.clone());
    super::session_temp::ensure(&profile.0).await?;
    let mut builder = BrowserConfig::builder()
      .user_data_dir(&profile.0)
      .respect_https_errors()
      .launch_timeout(Duration::from_secs(16))
      .request_timeout(Duration::from_secs(12));
    use super::config::ProxyMode;
    builder = match proxy.mode {
      ProxyMode::Direct => builder.arg("no-proxy-server"),
      ProxyMode::Environment => builder,
      ProxyMode::Manual => {
        if !proxy.username.is_empty() {
          return Err(Error::invalid(
            "server Chromium does not support authenticated manual proxies",
          ));
        }
        let url = proxy.url.replacen("socks5h://", "socks5://", 1);
        builder.arg(("proxy-server", url.as_str()))
      }
    };
    if !executable.is_empty() {
      builder = builder.chrome_executable(executable);
    }
    let config = builder.build().map_err(Error::invalid)?;
    let (mut browser, mut handler) = Browser::launch(config).await.map_err(Error::internal)?;
    let handler = tasks.spawn(async move { while let Some(Ok(_)) = handler.next().await {} });
    let page = async {
      match browser.pages().await?.into_iter().next() {
        Some(page) => Ok(page),
        None => browser.new_page("about:blank").await,
      }
    }
    .await;
    let page = match page {
      Ok(page) => page,
      Err(error) => {
        let _ = browser.kill().await;
        let _ = browser.wait().await;
        handler.abort();
        return Err(Error::internal(error));
      }
    };
    Ok(Self {
      browser,
      page,
      _profile: profile,
      _permit: permit,
      handler,
      used: std::time::Instant::now(),
    })
  }
  fn shutdown(mut self) -> impl Future<Output = ()> {
    let tasks = self._profile.1.clone();
    let cleanup = tasks.spawn(async move {
      let exited = tokio::time::timeout(Duration::from_secs(2), async {
        self.browser.close().await.ok();
        self.browser.wait().await
      })
      .await;
      if !matches!(exited, Ok(Ok(_))) {
        let _ = self.browser.kill().await;
        let _ = self.browser.wait().await;
      }
    });
    async move {
      let _ = cleanup.await;
    }
  }
  async fn act(&mut self, a: &Value) -> Result<Output> {
    let action = actions::string(a, "action")?;
    let pages = self.browser.pages().await.map_err(Error::internal)?;
    if action == "tabs" {
      let mut rows = Vec::new();
      for p in pages {
        rows.push(json!({"tab":p.target_id(),"url":p.url().await.map_err(Error::internal)?,"title":p.get_title().await.map_err(Error::internal)?}));
      }
      return Ok(Output::Json(json!({"active":self.page.target_id(),"tabs":rows})));
    }
    let before: Vec<_> = pages.iter().map(|p| p.target_id().clone()).collect();
    let page = if let Some(tab) = a["tab"].as_str() {
      pages
        .iter()
        .find(|p| p.target_id().as_ref() == tab)
        .cloned()
        .ok_or_else(|| Error::conflict("tab is closed"))?
    } else if action == "switch_tab" {
      return Err(Error::invalid("switch_tab requires tab"));
    } else {
      self.page.clone()
    };
    let value = match action {
      "open" | "new_tab" => {
        let url = actions::address(a)?;
        let selected = if action == "new_tab" {
          if pages.len() >= 16 {
            return Err(Error::conflict("server browser tab limit reached"));
          }
          self.browser.new_page(url).await.map_err(Error::internal)?
        } else {
          page.goto(url).await.map_err(Error::internal)?;
          page
        };
        self.page = selected;
        self.observe().await?
      }
      "switch_tab" => {
        page.bring_to_front().await.map_err(Error::internal)?;
        self.page = page;
        self.observe().await?
      }
      "close_tab" => {
        if page.target_id() == self.page.target_id() {
          let next = match pages.into_iter().find(|p| p.target_id() != page.target_id()) {
            Some(next) => next,
            None => self.browser.new_page("about:blank").await.map_err(Error::internal)?,
          };
          self.page = next;
        }
        page.close().await.map_err(Error::internal)?;
        self.observe().await?
      }
      "observe" | "read" => {
        let mut v = actions::dom(&page, a).await?;
        v["tab"] = json!(page.target_id());
        v
      }
      "wait" => actions::wait(&page, a).await?,
      "screenshot" => {
        return Ok(Output::Image(
          page.screenshot(ScreenshotParams::default()).await.map_err(Error::internal)?,
        ));
      }
      "evaluate" => {
        let source = actions::string(a, "script")?;
        actions::evaluate(&page, format!("({source}\n)({})", a["args"])).await?
      }
      _ => {
        actions::interact(&page, action, a).await?;
        let after = self.browser.pages().await.map_err(Error::internal)?;
        let next = after.into_iter().find(|p| !before.contains(p.target_id())).unwrap_or(page);
        self.page = next;
        self
          .observe()
          .await
          .unwrap_or_else(|error| json!({"action_dispatched":true,"observe_again":true,"observation_error":error.to_string()}))
      }
    };
    Ok(Output::Json(value))
  }
  async fn observe(&self) -> Result<Value> {
    let mut v = actions::dom(&self.page, &json!({"action":"observe"})).await?;
    v["tab"] = json!(self.page.target_id());
    Ok(v)
  }
}
enum Output {
  Json(Value),
  Image(Vec<u8>),
}
#[derive(Default)]
pub struct Room {
  client: client::Client,
  instance: Arc<Mutex<Option<Instance>>>,
  enabled: AtomicBool,
}
impl Room {
  pub async fn set_enabled(&self, enabled: bool) {
    self.enabled.store(enabled, Ordering::Release);
    if !enabled {
      self.client.disconnect();
      let mut guard = self.instance.lock().await;
      if let Some(instance) = guard.take() {
        instance.shutdown().await;
      }
    }
  }
  pub(super) fn can_unload(&self) -> bool {
    self.instance.try_lock().is_ok_and(|guard| guard.is_none()) && !self.client.connected()
  }
  pub(super) async fn reap_idle(&self) {
    let Ok(mut guard) = self.instance.try_lock() else { return };
    if guard
      .as_ref()
      .is_some_and(|i| i.handler.is_finished() || i.used.elapsed() >= Duration::from_secs(600))
    {
      if let Some(instance) = guard.take() {
        instance.shutdown().await;
      }
    }
  }
  async fn instance(
    &self,
    slot: &SessionSlot,
  ) -> Result<tokio::sync::OwnedMappedMutexGuard<Option<Instance>, Instance>> {
    let mut guard = self.instance.clone().lock_owned().await;
    slot.require_live()?;
    if !self.enabled.load(Ordering::Acquire) {
      return Err(Error::conflict("browser is disabled for this session"));
    }
    if guard.as_ref().is_some_and(|i| i.handler.is_finished())
      || guard
        .as_mut()
        .and_then(|i| i.browser.get_mut_child())
        .is_some_and(|child| child.try_wait().ok().flatten().is_some())
    {
      if let Some(stopped) = guard.take() {
        stopped.shutdown().await;
      }
    }
    if guard.is_none() {
      let app = slot.app.upgrade().ok_or_else(Error::shutting_down)?;
      let (path, proxy) = {
        let config = app.config_file.lock().await;
        (config.config.browser_executable.clone(), config.config.proxy.clone())
      };
      // The worker owns the slot throughout startup. Cancelling the calling tool
      // cannot drop Chromium halfway through launch, or race a second launch.
      let tasks = app.lifecycle.tasks.clone();
      let (send, recv) = tokio::sync::oneshot::channel();
      app
        .lifecycle
        .while_open(|| {
          app.lifecycle.tasks.spawn(async move {
            match Instance::launch(&path, &proxy, tasks).await {
              Ok(instance) if send.is_closed() => instance.shutdown().await,
              Ok(instance) => {
                *guard = Some(instance);
                if let Err(Ok(mut guard)) = send.send(Ok(guard))
                  && let Some(instance) = guard.take()
                {
                  instance.shutdown().await;
                }
              }
              Err(error) => {
                let _ = send.send(Err(error));
              }
            }
          })
        })
        .ok_or_else(Error::shutting_down)?;
      guard = recv.await.map_err(Error::internal)??;
    }
    let mut active = tokio::sync::OwnedMutexGuard::map(guard, |g| g.as_mut().unwrap());
    active.used = std::time::Instant::now();
    Ok(active)
  }
}
pub(crate) async fn execute(
  slot: &Arc<SessionSlot>,
  a: &Value,
  publication: Option<super::session::BlobPublication>,
) -> Result<ToolOutcome> {
  let action = actions::string(a, "action")?;
  let readonly = matches!(action, "observe" | "read" | "screenshot" | "wait" | "tabs");
  let room = &slot.tools.browser;
  slot.require_live()?;
  if !room.enabled.load(Ordering::Acquire) {
    return Err(Error::conflict("browser is disabled"));
  }
  if action == "targets" {
    return Ok(ToolOutcome::Success(client::targets(room)));
  }
  match a.get("target") {
    Some(v) if v == "client" => {
      return match room.client.call(a).await {
        Err(e) if e.details.as_ref().is_some_and(|v| v["kind"] == "unknown") => {
          Ok(ToolOutcome::Unknown(e.message))
        }
        result => result.map(ToolOutcome::Success),
      };
    }
    Some(v) if v != "server" => {
      return Err(Error::invalid("expected browser target client or server"));
    }
    None if room.client.connected() => {
      return Err(Error::invalid("choose target client or server"));
    }
    _ => {}
  }
  let mut instance = room.instance(slot).await?;
  let result = tokio::time::timeout(Duration::from_secs(16), Box::pin(instance.act(a))).await;
  drop(instance);
  let out = match result {
    Ok(Ok(out)) => out,
    Ok(Err(e)) => return Err(e),
    Err(_) if readonly => return Err(Error::conflict("browser observation timed out")),
    Err(_) => {
      return Ok(ToolOutcome::Unknown("Browser action timed out. Observe before retrying.".into()));
    }
  };
  let (name, bytes, image_mime) = match out {
    Output::Image(bytes) => {
      let image =
        crate::utils::model_image::process(move || crate::utils::model_image::prepare(&bytes))
          .await
          .map_err(Error::invalid)?;
      let name = if image.mime == "image/jpeg" { "browser.jpg" } else { "browser.png" };
      (name, image.bytes.clone(), Some(image.mime))
    }
    Output::Json(value) => {
      let bytes = serde_json::to_vec(&value).map_err(Error::internal)?;
      if bytes.len() <= 24 * 1024 {
        return Ok(ToolOutcome::Success(value));
      }
      ("browser-result.json", bytes, None)
    }
  };
  let app = slot.app.upgrade().ok_or_else(Error::shutting_down)?;
  let stream = futures_util::stream::once(async { Ok::<_, std::io::Error>(bytes) });
  let blob = attachments::upload_published(
    &app,
    &slot.get_descriptor().id,
    stream,
    false,
    Some(name.into()),
    publication,
  )
  .await?;
  let output = json!({"blob":blob.id,"name":name,"bytes":blob.byte_count,"path":blob.path});
  Ok(if let Some(mime) = image_mime {
    ToolOutcome::SuccessWithInput {
      output,
      input: vec![ContentBlock::ImageRef {
        mime_type: mime.into(),
        blob_id: blob.id,
        byte_count: blob.byte_count,
      }],
      model_text: None,
    }
  } else {
    ToolOutcome::Success(output)
  })
}
pub fn specification() -> Tool {
  Tool{name:"browser".into(),description:"Use action targets to discover two independent browsers. Set target client for the connected Wish device or server for session Chromium. When client is connected, use open with an HTTP(S) URL to launch or navigate its local page directly. The user can keep chatting while the sidebar is closed. Client supports only actions listed by targets, click/fill/select by ref. Never switch targets after a disconnect without user intent. Server actions: open navigates the active tab. observe returns visible controls with document-specific refs. click uses ref or screenshot x/y; fill uses ref and text; select uses ref and value; press uses key; scroll uses dy/dx. read pages text via section/offset. wait for text or selector. evaluate runs and awaits a JavaScript function(script) with one JSON args argument. screenshot produces a session image. Large results become session files. Refresh stale refs, and inspect uncertain actions before retrying.".into(),
    input_schema:json!({"type":"object","required":["action"],"additionalProperties":false,"properties":{
      "target":{"type":"string","enum":["client","server"]},"action":{"type":"string","enum":["targets","open","new_tab","tabs","switch_tab","close_tab","back","observe","read","click","fill","press","select","scroll","wait","screenshot","evaluate"]},
      "tab":{"type":"string"},"url":{"type":"string"},"ref":{"type":"string"},"text":{"type":"string","x-preserve-whitespace":true},"selector":{"type":"string"},"section":{"type":"string"},"offset":{"type":"integer","minimum":0},"x":{"type":"number"},"y":{"type":"number"},"dy":{"type":"number"},"dx":{"type":"number"},"key":{"type":"string"},"value":{"type":"string"},"script":{"type":"string","x-preserve-whitespace":true},"args":{}
    }})}
}
