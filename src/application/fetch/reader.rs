//! Session-owned immutable page snapshots, independent of tool schemas and provider wire formats.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};
#[derive(Clone, Serialize, Deserialize)]
pub struct FetchPage {
  pub url: String,
  pub title: String,
  pub content: String,
  pub provider: String,
  pub fetched_at: u64,
}
pub type FetchBackend = Arc<
  dyn Fn(
      String,
      bool,
      Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<FetchPage, String>> + Send>>
    + Send
    + Sync,
>;
#[derive(Serialize, Deserialize)]
struct PageSnapshot {
  extension: String,
  page: FetchPage,
}
pub struct PageReader {
  backend: FetchBackend,
  snapshots: crate::storage::snapshots::SnapshotStore,
}
pub struct PageView {
  pub output: Value,
  pub metadata: Value,
}
impl PageReader {
  pub fn new(backend: FetchBackend, directory: PathBuf) -> Self {
    Self { backend, snapshots: crate::storage::snapshots::SnapshotStore::new(directory) }
  }
  pub async fn read(
    &self,
    url: Option<&str>,
    cursor: Option<&str>,
    refresh: bool,
    provider: Option<&str>,
    allowed_services: &[&str],
    extension: &str,
  ) -> Result<PageView, String> {
    const CURSOR_ERROR: &str = "web_fetch: invalid cursor; copy `next_cursor` exactly from the preceding result, or omit `cursor` and provide `url` to start a new fetch";
    let (page, snapshot_id, start) = if let Some(cursor) =
      cursor.map(str::trim).filter(|cursor| !cursor.is_empty())
    {
      let (snapshot, offset) = cursor.split_once(':').ok_or(CURSOR_ERROR)?;
      let snapshot_id = uuid::Uuid::parse_str(snapshot).map_err(|_| CURSOR_ERROR)?;
      let start: usize = offset.parse().map_err(|_| CURSOR_ERROR)?;
      let snapshot: PageSnapshot = self
        .snapshots
        .read(snapshot_id)
        .await
        .map_err(|_| "page snapshot is unavailable; fetch the URL again")?;
      if snapshot.extension != extension {
        return Err("cursor belongs to another extension".into());
      }
      let page = snapshot.page;
      if !allowed_services.contains(&page.provider.as_str()) {
        return Err("cursor belongs to an unavailable service or another extension".into());
      }
      if provider.is_some_and(|id| id != page.provider) {
        return Err(
          "cursor belongs to another service; omit service to continue this snapshot".into(),
        );
      }
      (page, snapshot_id, start)
    } else {
      let url = url
        .map(str::trim)
        .filter(|url| !url.is_empty() && url.len() <= 8192)
        .ok_or("web_fetch: `url` or a returned `cursor` is required. Example: {\"url\":\"https://example.com\"}")?;
      let mut page = (self.backend)(url.into(), refresh, provider.map(str::to_owned)).await?;
      page.title = page.title.chars().take(512).collect();
      if page.content.len() > 4 * 1024 * 1024 {
        return Err("page exceeds the 4 MiB snapshot limit".into());
      }
      let snapshot = PageSnapshot { extension: extension.into(), page };
      let snapshot_id =
        self.snapshots.insert(&snapshot).await.map_err(|error| error.to_string())?;
      (snapshot.page, snapshot_id, 0)
    };
    let total_chars = page.content.chars().count();
    if start > total_chars {
      return Err(format!("{CURSOR_ERROR}; offset is outside the saved page"));
    }
    // Leave room for URL/title/continuation and shell presentation. Both entry points
    // return the same snapshot range, without shell truncating a second time.
    let mut content = String::new();
    let mut lines = 0;
    for ch in page.content.chars().skip(start).take(20_000) {
      if content.len() + ch.len_utf8() > 32 * 1024 || lines >= 1800 {
        break;
      }
      content.push(ch);
      if ch == '\n' {
        lines += 1;
      }
    }
    let end = start + content.chars().count();
    let next_cursor = (end < total_chars).then(|| format!("{snapshot_id}:{end}"));
    let output = json!({"provider":page.provider,"url":page.url,"title":page.title,"content":content,"start":start,"total_chars":total_chars,"next_cursor":next_cursor,"content_kind":"provider_extracted","fetched_at":page.fetched_at});
    Ok(PageView {
      output,
      metadata: json!({"provider":page.provider,"snapshot":snapshot_id.to_string(),"url":page.url,"title":page.title}),
    })
  }
}

/// Page presentation used by extension tool adapters.
pub fn render(output: &serde_json::Value, tool: &str) -> String {
  let title: String = output["title"].as_str().unwrap_or_default().chars().take(200).collect();
  let mut text = format!(
    "{title}\n{}\nProvider: {}\n\n{}",
    output["url"].as_str().unwrap_or_default(),
    output["provider"].as_str().unwrap_or_default(),
    output["content"].as_str().unwrap_or_default()
  );
  if let Some(cursor) = output["next_cursor"].as_str() {
    text.push_str(&format!("\n\n[Page incomplete: starts at character {} of {}. Continue with cursor {cursor}; CLI: wish extension call {tool} --json '{{\"cursor\":\"{cursor}\"}}']", output["start"], output["total_chars"]));
  }
  text
}
