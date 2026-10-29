//! Xiaoyunque 2.0 reference-video agent, served by Volcengine Visual OpenAPI.
use super::video_options::VideoOptions;
use crate::extensions::shared::video::{InputRole, ResolvedInput, VideoStatus};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
const MODEL: &str = "pippit_iv2v_v20_cvtob_with_vinput";
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
fn request(action: &str, body: Value) -> Result<(String, Draft), Error> {
  let mut draft = Draft::post_json(&body)?;
  draft.query = vec![("Action".into(), action.into()), ("Version".into(), "2022-08-31".into())];
  Ok(("/".into(), draft))
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  options: &VideoOptions,
  inputs: &[ResolvedInput],
) -> Result<(String, Draft), Error> {
  if model != MODEL {
    return Err(invalid("unsupported Xiaoyunque model"));
  }
  if prompt.trim().is_empty() || prompt.chars().count() > 2000 {
    return Err(invalid("Xiaoyunque prompt must contain 1..2000 characters"));
  }
  if options.duration.is_some() || options.resolution.is_some() {
    return Err(invalid(
      "Xiaoyunque chooses output duration/resolution; use duration_hint instead of duration",
    ));
  }
  let mut body = json!({"req_key":model,"prompt":prompt});
  if let Some(ratio) = &options.aspect_ratio {
    if !["16:9", "9:16", "4:3", "3:4"].contains(&ratio.as_str()) {
      return Err(invalid("unsupported Xiaoyunque aspect ratio"));
    }
    body["ratio"] = json!(ratio);
  }
  if let Some(duration) = &options.duration_hint {
    if !["～15s", "～30s", "40～60s", "1min+"].contains(&duration.as_str()) {
      return Err(invalid("unsupported Xiaoyunque duration hint"));
    }
    body["duration"] = json!(duration);
  }
  if let Some(language) = &options.language {
    if ![
      "Chinese",
      "English",
      "Japanese",
      "Thai",
      "SouthAfrican",
      "French",
      "Turkish",
      "Malay",
      "German",
      "Korean",
      "Russian",
      "Spanish",
      "Indonesian",
      "Italian",
      "Portuguese",
      "Filipino",
      "Vietnamese",
      "Dutch",
      "Arabic",
    ]
    .contains(&language.as_str())
    {
      return Err(invalid("unsupported Xiaoyunque language"));
    }
    body["language"] = json!(language);
  }
  if let Some(watermark) = options.watermark {
    body["enable_watermark"] = json!(watermark);
  }
  for (role, key) in
    [(InputRole::ReferenceImage, "img_url_list"), (InputRole::ReferenceVideo, "video_url_list")]
  {
    let urls: Vec<_> = inputs.iter().filter(|i| i.role == role).map(|i| &i.uri).collect();
    if !urls.is_empty() {
      body[key] = json!(urls);
    }
  }
  request("CVSync2AsyncSubmitTask", body)
}
pub(crate) fn task_id(value: &Value) -> Result<String, String> {
  if value["code"].as_u64() != Some(10000) {
    return Err(
      "Xiaoyunque did not acknowledge a task; inspect upstream status before retrying".into(),
    );
  }
  value["data"]["task_id"]
    .as_str()
    .filter(|id| {
      !id.is_empty()
        && id.len() <= 200
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
    .map(str::to_owned)
    .ok_or("Xiaoyunque returned no valid task ID; do not automatically resubmit".into())
}
pub(crate) fn status_request(task: &str) -> Result<(String, Draft), Error> {
  request("CVSync2AsyncGetResult", json!({"req_key":MODEL,"task_id":task}))
}
pub(crate) fn status(value: &Value) -> Result<VideoStatus, Error> {
  if matches!(value["code"].as_u64(), Some(50411 | 50412 | 50413 | 50511 | 50512 | 50513 | 50514)) {
    return Ok(VideoStatus::Failed);
  }
  if value["code"].as_u64() != Some(10000) {
    // Query throttling/transient errors must not discard a live upstream task.
    return Err(invalid("Xiaoyunque query failed; existing task was retained"));
  }
  match value["data"]["status"].as_str() {
    Some("processing" | "in_queue" | "generating") => Ok(VideoStatus::Pending),
    Some("done") => {
      let url = value["data"]["video_url"]
        .as_str()
        .ok_or_else(|| invalid("completed Xiaoyunque task has no video URL"))?;
      crate::extensions::shared::urls::validate_url(url)?;
      Ok(VideoStatus::Complete(url.into()))
    }
    Some("not_found" | "expired") => Ok(VideoStatus::Failed),
    _ => Err(invalid("unknown Xiaoyunque task status; existing task was retained")),
  }
}
