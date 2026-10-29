//! xAI video tool contract. Inputs reflect xAI workflows rather than a universal media schema.
use crate::{
  extensions::catalog::{Invocation, Permission, Registration},
  protocol::Tool,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Asset {
  Blob(String),
  Url(String),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
  pub image: Asset,
  pub timestamp_s: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generate {
  #[serde(default)]
  pub prompt: String,
  pub service: Option<String>,
  pub image: Option<Asset>,
  pub last_frame: Option<Asset>,
  #[serde(default)]
  pub reference_images: Vec<Asset>,
  #[serde(default)]
  pub keyframes: Vec<Keyframe>,
  #[serde(default)]
  pub reference_voices: Vec<String>,
  pub duration: Option<u32>,
  pub generate_audio: Option<bool>,
  pub resolution: Option<String>,
  pub aspect_ratio: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Generate(Box<Generate>),
  Edit { prompt: String, video: Asset, service: Option<String> },
  Extend { prompt: String, video: Asset, duration: Option<u32>, service: Option<String> },
  Status { job_id: String },
  ListJobs {},
}
fn asset_schema() -> Value {
  json!({"oneOf":[
    {"type":"object","additionalProperties":false,"required":["blob"],"properties":{"blob":{"type":"string","pattern":"^[a-f0-9]{64}$"}}},
    {"type":"object","additionalProperties":false,"required":["url"],"properties":{"url":{"type":"string","description":"Public HTTPS URL; never a local path."}}}
  ]})
}
pub fn specification() -> Tool {
  Tool {
    name:"xai_video".into(),
    description:"Generate, edit or extend videos using xAI. Omit service for the first compatible configured xAI service in priority order. generate supports text, first image, references and (grok-imagine-video-1.5) last_frame, up to4 keyframes and3 preset reference_voices. Assets are {blob:session-owned blob ID} or {url:public HTTPS URL}. Local image and classic MP4 input blobs are supported; images/videos are bounded by the host. Classic supports up to7 image references, 1.5 up to14. Reference duration classic <=10s, otherwise generation 1..15s. 1.5-lite supports text/first-image only, including upscaled1080p; no references/keyframes/edit/extend. generate_audio=false requests a silent video. 1080p requires 1.5 or1.5-lite text/first-image without other references; other modes <=720p. Image-only animation preserves aspect ratio; omit aspect_ratio. Keyframes lie inside duration in distinct rounded 1/3-second slots. edit/extend require classic grok-imagine-video; editing source <=8.7s, extending source2..15s by2..10s (upstream validates duration/codec). Custom audio/video references are not exposed. Returns a durable job; status resumes polling/download without regeneration; list_jobs lists only xAI video jobs in this session. Output video becomes a local Wish blob path. Do not repeat an outcome_unknown submission.".into(),
    input_schema:json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
      "action":{"type":"string","enum":["generate","edit","extend","status","list_jobs"]},
      "prompt":{"type":"string","description":"Required for edit/extend; optional for generate with image/keyframe guidance."},
      "service":{"type":"string","description":"Optional configured xAI video service ID; submission actions only."},
      "image":asset_schema(),"last_frame":asset_schema(),"video":asset_schema(),
      "reference_images":{"type":"array","maxItems":14,"items":asset_schema()},
      "keyframes":{"type":"array","maxItems":4,"items":{"type":"object","additionalProperties":false,"required":["image","timestamp_s"],"properties":{"image":asset_schema(),"timestamp_s":{"type":"number","exclusiveMinimum":0}}}},
      "reference_voices":{"type":"array","maxItems":3,"items":{"type":"string","description":"Preset xAI voice ID, e.g. eve; refer to <AUDIO_0> in prompt."}},
      "duration":{"type":"integer","minimum":1,"maximum":15,"description":"generate:1..15; classic reference <=10. extend:2..10, default6. Not accepted for edit."},
      "generate_audio":{"type":"boolean","description":"generate only; false requests silent output; omitted uses upstream default audio."},
      "resolution":{"type":"string","enum":["480p","720p","1080p"],"description":"generate only."},
      "aspect_ratio":{"type":"string","enum":["1:1","16:9","9:16","4:3","3:4","3:2","2:3","21:9","5:2"],"description":"generate only; not accepted for first-image-only animation."},
      "job_id":{"type":"string","description":"status only; list_jobs accepts only action."}
    }})
  }
}
pub const REGISTRATION: Registration = Registration {
  extension: "xai",
  name: "xai_video",
  summary: "xAI video generation, image/keyframe guidance, editing and extension",
  tags: &["video", "generation", "image-to-video", "references", "editing", "extension"],
  service_presets: &["xai_video", "xai_video_lite", "xai_video_classic"],
  accepts_model: |m| {
    matches!(m, "grok-imagine-video" | "grok-imagine-video-1.5" | "grok-imagine-video-1.5-lite")
  },
  permission: Permission::VideoGeneration,
  specification,
  examples: || {
    json!([
    {"action":"generate","prompt":"A slow cinematic pan","image":{"blob":"<session image blob ID>"},"duration":8},
    {"action":"edit","prompt":"Make the scene snowy","video":{"blob":"<session MP4 blob ID>"}},
    {"action":"extend","prompt":"Continue the camera movement","video":{"url":"https://example.com/source.mp4"},"duration":6},
    {"action":"status","job_id":"<returned job ID>"},{"action":"list_jobs"}])
  },
  parse: |v| serde_json::from_value::<Request>(v).map(Invocation::new).map_err(|e| e.to_string()),
};
