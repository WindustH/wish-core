//! Runway-owned video tool contract, separate from other suppliers' media requests.
pub(crate) mod archive;
pub(crate) mod edit;
pub(crate) mod wire;
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
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Generate {
    prompt: String,
    service: Option<String>,
    image: Option<Asset>,
    duration: Option<u32>,
    aspect_ratio: Option<String>,
    seed: Option<u32>,
  },
  Edit {
    prompt: Option<String>,
    service: Option<String>,
    video: Asset,
    first_frame: Option<Asset>,
    last_frame: Option<Asset>,
    #[serde(default)]
    keyframes: Vec<edit::Keyframe>,
    seed: Option<u32>,
    target_aspect_ratio: Option<String>,
    output_format: Option<edit::OutputFormat>,
    prores_profile: Option<String>,
  },
  Status {
    job_id: String,
  },
  ListJobs {},
}
fn asset() -> Value {
  json!({"oneOf":[
    {"type":"object","additionalProperties":false,"required":["blob"],"properties":{"blob":{"type":"string","pattern":"^[a-f0-9]{64}$"}}},
    {"type":"object","additionalProperties":false,"required":["url"],"properties":{"url":{"type":"string","description":"Public HTTPS media URL or Runway upload URI."}}}
  ]})
}
pub fn specification() -> Tool {
  Tool { name: "runway_video".into(), description: "Runway video generation and editing. generate uses Gen-4.5: text or one first image, integer duration2..10 seconds (default5),720p, aspect_ratio16:9 or9:16. edit uses Aleph2: one source video, optional first_frame/last_frame shortcuts OR up to5 keyframes using at (0..1 fraction) or seconds (absolute time), optionally range {start_seconds,end_seconds}. All frames must set range or none; end is exclusive. Fraction-to-window and actual duration compatibility is upstream validated. Omit prompt for frame-guided edits. target_aspect_ratio requests outpainting; otherwise preserves input framing and length, source maximum30s (validated upstream). Assets are {blob:session-owned blob ID} or {url:public HTTPS URL or runway upload URI}; local video uploads are managed by Wish. Omit service to select the first compatible configured Runway service in priority order. Returns a durable job; status resumes polling and saving output as owned Wish blobs, list_jobs only lists this session's Runway video jobs. Never resubmit an outcome_unknown generation. seed controls generation/edit reproducibility (unsigned32bit). Edit output_format can be mp4, prores (MOV) or sdr_rec709_10bit (HEVC MP4); non-default formats incur additional upstream charges and may require an external player. prores_profile applies only to prores. png_sequence returns a ZIP of PNG frames (extra upstream charges). Wish validates without extracting:512MiB download,10000entries,8MiB directory,2GiB total expanded frame bytes,256MiB per frame. Multi-disk/ZIP64 central directories are not accepted. Do not combine first_frame/last_frame with keyframes.".into(), input_schema: json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
    "action":{"type":"string","enum":["generate","edit","status","list_jobs"]},
    "service":{"type":"string","description":"Configured Runway service ID; submission only."},
    "prompt":{"type":"string","minLength":1,"description":"Required for generate; optional for frame-guided editing."},
    "image":asset(),"video":asset(),"first_frame":asset(),"last_frame":asset(),
    "output_format":{"type":"string","enum":["mp4","prores","sdr_rec709_10bit","png_sequence"],"description":"edit only; prores and10-bit SDR incur extra upstream charges."},
    "prores_profile":{"type":"string","enum":["422","4444","422 Proxy","422 LT","422 HQ","4444 XQ"],"description":"Only with output_format=prores; upstream default4444. Alpha channel is opaque, not a transparency matte."},
    "seed":{"type":"integer","minimum":0,"maximum":4294967295u64,"description":"generate/edit only."},
    "target_aspect_ratio":{"type":"string","enum":["16:9","4:3","3:2","1:1","2:3","3:4","9:16","21:9"],"description":"Aleph edit only: expand/outpaint to this framing."},
    "keyframes":{"type":"array","maxItems":5,"description":"Edit only; each frame needs exactly one of at or seconds. Incompatible with first_frame/last_frame.","items":{"type":"object","additionalProperties":false,"required":["image"],"properties":{
      "image":asset(),"at":{"type":"number","minimum":0,"maximum":1},"seconds":{"type":"number","minimum":0,"maximum":30},
      "range":{"type":"object","additionalProperties":false,"required":["start_seconds","end_seconds"],"properties":{"start_seconds":{"type":"integer","minimum":0,"maximum":29},"end_seconds":{"type":"integer","minimum":1,"maximum":30}}}
    }}},
    "duration":{"type":"integer","minimum":2,"maximum":10,"description":"generate only."},
    "aspect_ratio":{"type":"string","enum":["16:9","9:16"],"description":"generate only."},
    "job_id":{"type":"string","description":"status only; list_jobs accepts only action."}
  }}) }
}
pub const REGISTRATION: Registration = Registration {
  extension: "runway",
  name: "runway_video",
  summary: "Runway Gen-4.5 generation and Aleph 2 video editing",
  tags: &["video", "generation", "image-to-video", "editing", "keyframes", "outpainting"],
  service_presets: &["runway_video", "runway_aleph_video"],
  accepts_model: |model| matches!(model, "gen4.5" | "aleph2"),
  permission: Permission::VideoGeneration,
  specification,
  examples: || {
    json!([
      {"action":"generate","prompt":"A cinematic camera pan","image":{"blob":"<session image blob ID>"},"duration":5},
      {"action":"edit","prompt":"Change the background to a forest","video":{"blob":"<session video blob ID>"},"first_frame":{"blob":"<guidance image blob ID>"}},
      {"action":"edit","video":{"blob":"<session video blob ID>"},"seed":0,"target_aspect_ratio":"21:9","keyframes":[{"image":{"blob":"<guidance image blob ID>"},"seconds":2.5,"range":{"start_seconds":1,"end_seconds":4}}]},
      {"action":"status","job_id":"<returned job ID>"},{"action":"list_jobs"}
    ])
  },
  parse: |value| {
    serde_json::from_value(value).map(Invocation::RunwayVideo).map_err(|e| e.to_string())
  },
};
pub mod endpoint;
pub mod video_contract;
pub mod video_options;
pub mod video_status;

pub mod upload;

pub(crate) mod operation;
