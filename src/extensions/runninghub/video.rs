//! RunningHub standard model API. Task IDs survive restarts through the shared job journal.
use super::video_options::VideoOptions;
use crate::extensions::shared::video::{InputRole, ResolvedInput, VideoMode, VideoStatus};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};

pub(crate) fn submission(
  model: &str,
  prompt: &str,
  mode: VideoMode,
  options: &VideoOptions,
  inputs: &[ResolvedInput],
) -> Result<(String, Draft), Error> {
  let invalid = |message: &str| Error::Build(message.into());
  if prompt.trim().is_empty() || prompt.chars().count() > 4100 {
    return Err(invalid("prompt must contain 1..4100 characters"));
  }
  if model != "bytedance/seedance-2.5-token" {
    return Err(invalid("unsupported RunningHub video model"));
  }
  let duration = options.duration.unwrap_or(if mode == VideoMode::Edit { -1.0 } else { 5.0 });
  if duration != -1.0 && (!(4.0..=30.0).contains(&duration) || duration.fract() != 0.0) {
    return Err(invalid("RunningHub Seedance duration must be -1 or an integer in 4..30"));
  }
  let resolution = options.resolution.as_deref().unwrap_or("720p");
  if !["480p", "720p", "native1080p", "1080p", "2k", "4k"].contains(&resolution) {
    return Err(invalid("unsupported RunningHub Seedance resolution"));
  }
  let ratio = options.aspect_ratio.as_deref().unwrap_or("adaptive");
  if !["adaptive", "16:9", "4:3", "1:1", "3:4", "9:16", "21:9"].contains(&ratio) {
    return Err(invalid("unsupported RunningHub Seedance aspect ratio"));
  }
  if matches!(mode, VideoMode::Edit | VideoMode::Extend)
    && (ratio != "adaptive" || mode == VideoMode::Edit && duration != -1.0)
  {
    return Err(invalid("Seedance edit/extend requires adaptive ratio; edit requires duration -1"));
  }
  let mut body = json!({"prompt":prompt,"duration":(duration as i64).to_string(),"resolution":resolution,"ratio":ratio,"outputFormat":"mp4"});
  let endpoint = match mode {
    VideoMode::Text => "text-to-video",
    VideoMode::Frames => {
      for input in inputs {
        let key = match input.role {
          InputRole::FirstFrame => "firstFrameUrl",
          InputRole::LastFrame => "lastFrameUrl",
          _ => return Err(invalid("invalid frame role")),
        };
        body[key] = json!(input.uri);
      }
      "image-to-video"
    }
    VideoMode::Reference | VideoMode::Edit | VideoMode::Extend => {
      for (key, roles) in [
        ("imageUrls", &[InputRole::ReferenceImage][..]),
        ("videoUrls", &[InputRole::ReferenceVideo, InputRole::SourceVideo][..]),
        ("audioUrls", &[InputRole::ReferenceAudio][..]),
      ] {
        let urls: Vec<_> = inputs
          .iter()
          .filter(|input| roles.contains(&input.role))
          .map(|input| &input.uri)
          .collect();
        if !urls.is_empty() {
          body[key] = json!(urls);
        }
      }
      body["omniReferenceTaskType"] = json!(match mode {
        VideoMode::Edit => "edit",
        VideoMode::Extend => "extend",
        _ => "reference",
      });
      "multimodal-video"
    }
  };
  Ok((format!("/openapi/v2/{model}/{endpoint}"), Draft::post_json(&body)?))
}

pub(crate) fn status(value: &Value) -> Result<VideoStatus, Error> {
  match value["status"].as_str() {
    Some("SUCCESS") => {
      // APIs can return auxiliary images alongside the video. Never download the first item blindly.
      let results = value["results"]
        .as_array()
        .ok_or_else(|| Error::Build("completed RunningHub task has no results".into()))?;
      let videos: Vec<_> = results
        .iter()
        .filter(|item| {
          matches!(item["outputType"].as_str(), Some("mp4" | "mov" | "webm" | "video"))
        })
        .collect();
      if videos.len() != 1 {
        return Err(Error::Build("RunningHub task must return exactly one video artifact".into()));
      }
      let url = videos[0]["url"]
        .as_str()
        .ok_or_else(|| Error::Build("RunningHub video result has no URL".into()))?;
      crate::extensions::shared::urls::validate_url(url)?;
      Ok(VideoStatus::Complete(url.into()))
    }
    Some("RUNNING" | "QUEUED") => Ok(VideoStatus::Pending),
    Some("FAILED") => Ok(VideoStatus::Failed),
    _ => Err(Error::Build("unrecognized RunningHub status; existing task was retained".into())),
  }
}

pub fn task_id(value: &serde_json::Value) -> Result<String, String> {
  crate::extensions::shared::video::task_identifier(&value["taskId"])
}
pub fn status_request(
  task: &str,
) -> Result<(String, crate::protocol::endpoint::Draft), crate::protocol::error::Error> {
  Ok((
    "/openapi/v2/query".into(),
    crate::protocol::endpoint::Draft::post_json(&serde_json::json!({"taskId":task}))?,
  ))
}
