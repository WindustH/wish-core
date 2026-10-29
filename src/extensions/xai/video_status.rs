//! Supplier-owned task identity, polling request and completion semantics.
use crate::extensions::shared::video::{VideoStatus, task_identifier};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::Value;
pub fn task_id(value: &Value) -> Result<String, String> {
  task_identifier(&value["request_id"])
}
pub fn request(task: &str) -> Result<(String, Draft), Error> {
  Ok((format!("/v1/videos/{task}"), Draft::get(Vec::new())))
}
pub fn decode(value: &Value) -> Result<VideoStatus, Error> {
  if value["video"]["respect_moderation"] == false {
    return Ok(VideoStatus::Failed);
  }
  let status = &value["status"];
  let url = &value["video"]["url"];
  match status.as_str().unwrap_or("").to_ascii_lowercase().as_str() {
    "succeeded" | "completed" | "done" => {
      let url =
        url.as_str().ok_or_else(|| Error::Build("completed task has no artifact URL".into()))?;
      crate::extensions::shared::urls::validate_url(url)?;
      Ok(VideoStatus::Complete(url.into()))
    }
    "failed" | "cancelled" | "canceled" => Ok(VideoStatus::Failed),
    "queued" | "pending" | "running" | "processing" | "throttled" | "dreaming" => {
      Ok(VideoStatus::Pending)
    }
    _ => Err(Error::Build("unrecognized upstream task status; existing task was retained".into())),
  }
}
