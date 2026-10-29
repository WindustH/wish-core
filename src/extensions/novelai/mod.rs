//! NovelAI's independent tool contract; host adapters supply session-owned inputs and credentials.
pub(crate) mod characters;
pub(crate) mod preparation;
pub(crate) mod references;
pub(crate) mod wire;
use crate::protocol::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
pub const TOOL_NAME: &str = "novelai_generate";
pub const TAGS: &[&str] = &["image", "generation", "anime", "editing", "reference", "character"];
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  pub prompt: String,
  #[serde(default)]
  pub service: Option<String>,
  #[serde(default)]
  pub source_image: Option<String>,
  #[serde(default)]
  pub parameters: serde_json::Map<String, Value>,
  #[serde(default)]
  pub references: Vec<Reference>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
  pub image: String,
  pub kind: ReferenceKind,
  #[serde(default = "full_strength")]
  pub strength: f64,
  #[serde(default = "full_strength")]
  pub fidelity: f64,
}
pub use references::ReferenceKind;
fn full_strength() -> f64 {
  1.0
}

pub fn specification() -> Tool {
  Tool { name: TOOL_NAME.into(), description: "Generate an illustration or edit one session-owned source image using NovelAI. service optionally selects a configured NovelAI service ID returned by extension_tools; omit to use the first compatible NovelAI service in configured priority order. Parameters override that service for this call only. width/height are multiples of64 in64..2048; steps1..50; scale0..10; seed is unsigned32-bit. strength/noise0..1 require source_image. sampler and noise_schedule accept provider-native identifiers (for example k_euler_ancestral and karras); model compatibility is checked by NovelAI. cfg_rescale0..1 controls prompt guidance rescaling; dynamic_thresholding toggles Decrisper. Explicit zero/false overrides configured defaults. character_prompts supplies up to six separate V4.5 character descriptions with individual negative prompts and optional normalized centers; base prompt describes the scene and subject count. Use references for V4.5 precise character/style conditioning, separate from source_image; they can be combined. Up to eight session PNG/JPEG/WebP blobs, each edge at most 4096 pixels and combined inputs at most 12 MiB. Wish pads them to the supported reference canvases. strength/fidelity default to 1 and must be in 0..1. References incur additional upstream charges; multiple character references may blend characters. Results are session blob IDs and local paths. Generation is paid: never automatically resubmit an interrupted request. This tool does not support masks or video.".into(), input_schema: json!({"type":"object","additionalProperties":false,"required":["prompt"],"properties":{
    "prompt":{"type":"string","minLength":1,"maxLength":32768},
    "service":{"type":"string","minLength":1},
    "source_image":{"type":"string","description":"Session image blob ID for image-to-image; omit for text-to-image."},
    "references":{"type":"array","maxItems":8,
      "description":"V4.5 precise character/style references. Local session image blobs only; additional upstream charges apply.",
      "items":{"type":"object","additionalProperties":false,"required":["image","kind"],"properties":{
        "image":{"type":"string","description":"Session-owned PNG/JPEG/WebP blob ID."},
        "kind":{"type":"string","enum":["character","style","character_style"]},
        "strength":{"type":"number","minimum":0,"maximum":1,"default":1},
        "fidelity":{"type":"number","minimum":0,"maximum":1,"default":1}
      }}},
    "parameters":{"type":"object","additionalProperties":false,"properties":{
      "character_prompts": characters::schema(),
      "width":{"type":"integer","minimum":64,"maximum":2048,"multipleOf":64},
      "height":{"type":"integer","minimum":64,"maximum":2048,"multipleOf":64},
      "steps":{"type":"integer","minimum":1,"maximum":50},"scale":{"type":"number","minimum":0,"maximum":10},
      "seed":{"type":"integer","minimum":0,"maximum":4294967295u64},
      "n_samples":{"type":"integer","minimum":1,"maximum":8},
      "sampler":{"type":"string","minLength":1,"maxLength":128,"description":"NovelAI sampler identifier; upstream model compatibility applies, e.g. k_euler_ancestral."},
      "noise_schedule":{"type":"string","minLength":1,"maxLength":128,"description":"NovelAI noise schedule identifier; upstream model compatibility applies, e.g. karras."},
      "cfg_rescale":{"type":"number","minimum":0,"maximum":1,"description":"Prompt Guidance Rescale; 0 disables rescaling."},
      "dynamic_thresholding":{"type":"boolean","description":"Decrisper; false explicitly disables it."},
      "negative_prompt":{"type":"string"},"strength":{"type":"number","minimum":0,"maximum":1},"noise":{"type":"number","minimum":0,"maximum":1}
    }}
  }}) }
}

pub const REGISTRATION: super::catalog::Registration = super::catalog::Registration {
  extension: "novelai",
  name: TOOL_NAME,
  summary: "NovelAI illustrations, image-to-image edits and precise character/style references",
  tags: TAGS,
  service_presets: &["novelai_images"],
  accepts_model: |_| true,
  permission: super::catalog::Permission::ImageGeneration,
  specification,
  examples: || {
    json!([
      {"prompt":"watercolor landscape, soft evening light", "service":"<configured NovelAI service ID>", "parameters":{"width":1024,"height":1024,"sampler":"k_euler_ancestral","noise_schedule":"karras","cfg_rescale":0.3,"dynamic_thresholding":false}},
      {"prompt":"preserve the composition, change to winter", "service":"<configured NovelAI service ID>", "source_image":"<session image blob ID>", "parameters":{"strength":0.45}},
      {"prompt":"the referenced character walking in a snowy forest", "service":"<configured NovelAI V4.5 service ID>", "references":[{"image":"<session image blob ID>","kind":"character","strength":0.8,"fidelity":1}]},
      {"prompt":"2girls, watercolor, a park in autumn", "parameters":{"character_prompts":[{"prompt":"girl, red coat","negative_prompt":"blue coat","centers":[{"x":0.25,"y":0.5}]},{"prompt":"girl, blue coat","centers":[{"x":0.75,"y":0.5}]}]}}
    ])
  },
  parse: |value| {
    serde_json::from_value(value)
      .map(super::catalog::Invocation::Novelai)
      .map_err(|error| error.to_string())
  },
};

// Keep character_prompts last: it is advertised only for verified V4.5 models.
const REQUEST_OPTIONS: &[&str] = &[
  "width",
  "height",
  "steps",
  "n_samples",
  "scale",
  "seed",
  "negative_prompt",
  "strength",
  "noise",
  "sampler",
  "noise_schedule",
  "cfg_rescale",
  "dynamic_thresholding",
  "character_prompts",
];
pub(crate) fn request_options(model: &str) -> &'static [&'static str] {
  if characters::supported(model) {
    REQUEST_OPTIONS
  } else {
    &REQUEST_OPTIONS[..REQUEST_OPTIONS.len() - 1]
  }
}
pub mod endpoint;
