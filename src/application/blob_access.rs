//! One session-scoped file capability for the model tool and `wish blob` command.
use super::{app::App, attachments, error::ApplicationError, session::SessionSlot};
use crate::{protocol::Tool, storage::blobs};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncReadExt;

fn yes() -> bool {
  true
}
fn limit() -> usize {
  80
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  List {
    #[serde(default = "yes")]
    all: bool,
    #[serde(default)]
    start: usize,
    #[serde(default = "limit")]
    limit: usize,
  },
  Add {
    path: String,
    name: Option<String>,
    visible: Option<bool>,
  },
  #[serde(alias = "symlink")]
  Link {
    path: String,
    visible: Option<bool>,
  },
  Mention {
    source: String,
    label: Option<String>,
    page: Option<u32>,
    time: Option<f64>,
    visible: Option<bool>,
  },
  Update {
    id: String,
    visible: Option<bool>,
    pinned: Option<bool>,
  },
  Delete {
    id: String,
  },
}
pub fn specification() -> Tool {
  Tool {
    name: "blob".into(),
    description: "Manage files belonging to this session. list includes hidden files by default. add copies a local file; link (symlink) records its local path without copying or modifying it. mention accepts a blob ID or local path (silently linked), returns a clickable user-facing reference and Markdown; optional page is a 1-based PDF page and time is video/audio seconds. Include the returned Markdown in your answer. add/link appear in the Library by default; mention alone does not add to the displayed Library. update sets visible/pinned. delete removes stored bytes or the link, NEVER the original linked file; refused for images required by current/standby context or pending input. Hiding is always available.".into(),
    input_schema: json!({"type":"object","required":["action"],"properties":{
      "action":{"type":"string","enum":["list","add","link","symlink","mention","update","delete"]},
      "all":{"type":"boolean"},"start":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":200},
      "id":{"type":"string","description":"Session-local blob ID"},
      "path":{"type":"string","x-preserve-whitespace":true},
      "source":{"type":"string","description":"Blob ID or local file path, resolved relative to this session's working directory","x-preserve-whitespace":true},
      "name":{"type":"string"},"label":{"type":"string"},
      "page":{"type":"integer","minimum":1},"time":{"type":"number","minimum":0},
      "visible":{"type":"boolean"},"pinned":{"type":"boolean"}
    },"additionalProperties":false}),
  }
}
pub fn parse_request(arguments: &Value) -> Result<Request, ApplicationError> {
  let value = crate::tool::arguments::normalize(arguments, &specification().input_schema)
    .map_err(ApplicationError::invalid)?;
  serde_json::from_value(value)
    .map_err(|e| ApplicationError::invalid(format!("{e}; use wish blob --help")))
}
fn path(slot: &SessionSlot, value: &str) -> Result<PathBuf, ApplicationError> {
  if value.is_empty() || value.contains('\0') {
    return Err(ApplicationError::invalid("supply a local file path"));
  }
  let path = PathBuf::from(value);
  Ok(if path.is_absolute() { path } else { slot.get_descriptor().cwd.join(path) })
}
pub async fn execute(
  app: &Arc<App>,
  slot: Arc<SessionSlot>,
  request: Request,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  slot.require_live()?;
  let owner = slot.get_descriptor().id;
  match request {
    Request::List { all, start, limit } => {
      serde_json::to_value(attachments::list(app, &owner, start, limit, all).await?)
        .map_err(ApplicationError::internal)
    }
    Request::Add { path: input, name, visible } => {
      let path = path(&slot, &input)?;
      let name = name.or_else(|| path.file_name().map(|name| name.to_string_lossy().into_owned()));
      let file = tokio::fs::File::open(path).await.map_err(ApplicationError::internal)?;
      if !file.metadata().await.map_err(ApplicationError::internal)?.is_file() {
        return Err(ApplicationError::invalid("path must be a regular file"));
      }
      let stream = futures_util::stream::try_unfold(file, |mut file| async move {
        let mut bytes = vec![0u8; 64 * 1024];
        let count = file.read(&mut bytes).await?;
        if count == 0 {
          return Ok::<_, std::io::Error>(None);
        }
        bytes.truncate(count);
        Ok(Some((bytes, file)))
      });
      let blob =
        attachments::upload_stream(app, &owner, stream, visible == Some(false), name).await?;
      Ok(json!(blob))
    }
    Request::Link { path: input, visible } => {
      let path = path(&slot, &input)?;
      Ok(json!(
        attachments::link(app, &owner, &path.to_string_lossy(), visible == Some(false)).await?
      ))
    }
    Request::Update { id, visible, pinned } => Ok(json!(
      attachments::update_library(app, &owner, &id, blobs::library::Patch { visible, pinned })
        .await?
    )),
    Request::Delete { id } => {
      attachments::delete(app, &owner, &id).await?;
      Ok(json!({"deleted":true,"id":id}))
    }
    Request::Mention { source, label, page, time, visible } => {
      if page == Some(0) || time.is_some_and(|time| !time.is_finite() || time < 0.) {
        return Err(ApplicationError::invalid(
          "page must be at least 1; time must be finite non-negative seconds",
        ));
      }
      if label.as_ref().is_some_and(|s| s.len() > 1024) {
        return Err(ApplicationError::invalid("label exceeds 1024 bytes"));
      }
      let (id, file_name) = if blobs::is_blob_id(&source) {
        (source, None)
      } else {
        let path = path(&slot, &source)?;
        let blob = attachments::link(app, &owner, &path.to_string_lossy(), true).await?;
        (blob.id, path.file_name().map(|s| s.to_string_lossy().into_owned()))
      };
      let metadata = serde_json::to_value(attachments::metadata(app, &owner, &id).await?)
        .map_err(ApplicationError::internal)?;
      if let Some(visible) = visible {
        attachments::update_library(
          app,
          &owner,
          &id,
          blobs::library::Patch { visible: Some(visible), pinned: None },
        )
        .await?;
      }
      let name = label
        .or(file_name)
        .or_else(|| metadata["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| id[..12].into());
      let mut url = reqwest::Url::parse("wish://blob/").map_err(ApplicationError::internal)?;
      url
        .path_segments_mut()
        .map_err(|_| ApplicationError::internal("blob URL"))?
        .clear()
        .push(&owner)
        .push(&id);
      if page.is_some() || time.is_some() {
        let mut query = url.query_pairs_mut();
        if let Some(page) = page {
          query.append_pair("page", &page.to_string());
        }
        if let Some(time) = time {
          query.append_pair("time", &time.to_string());
        }
      }
      let label = name
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ");
      Ok(
        json!({"mention":{"owner":owner,"id":id,"name":name,"mime_type":metadata["mime_type"],"page":page,"time":time},"markdown":format!("[{label}](<{url}>)")}),
      )
    }
  }
}
