//! BFL tools keep generation/reference editing separate from masked Fill operations.
use crate::{
  extensions::catalog::{Invocation, Permission, Registration},
  protocol::Tool,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generate {
  #[serde(default)]
  pub service: Option<String>,
  pub prompt: String,
  #[serde(default)]
  pub reference_images: Vec<String>,
  #[serde(default)]
  pub parameters: Map<String, Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fill {
  #[serde(default)]
  pub service: Option<String>,
  pub prompt: String,
  pub source_image: String,
  #[serde(default)]
  pub mask: Option<String>,
  #[serde(default)]
  pub parameters: Map<String, Value>,
}
pub fn generation_model(model: &str) -> bool {
  matches!(
    model,
    "flux-2-pro"
      | "flux-2-pro-preview"
      | "flux-2-max"
      | "flux-2-flex"
      | "flux-2-klein-4b"
      | "flux-2-klein-9b"
      | "flux-2-klein-9b-preview"
      | "flux-kontext-pro"
      | "flux-kontext-max"
  )
}
fn parameters(fill: bool) -> Value {
  let mut properties = json!({
    "seed":{"type":"integer"},
    "output_format":{"type":"string","enum":["png","jpeg","webp"]},
    "prompt_upsampling":{"type":"boolean"},
    "steps":{"type":"integer","minimum":if fill {15} else {1},"maximum":50},
    "guidance":{"type":"number","minimum":1.5,"maximum":if fill {100.0} else {10.0}}
  });
  if !fill {
    properties["width"] = json!({"type":"integer","description":"0 for automatic, otherwise at least 64; FLUX.2 only."});
    properties["height"] = properties["width"].clone();
    properties["aspect_ratio"] =
      json!({"type":"string","description":"Kontext only; WIDTH:HEIGHT between 9:21 and 21:9."});
  }
  json!({"type":"object","additionalProperties":false,"properties":properties})
}
pub fn generation_specification() -> Tool {
  Tool {
    name:"bfl_generate".into(),
    description:"Generate or reference-edit with a configured BFL FLUX.2 or Kontext service. Discover services with extension_tools. Omit service to use the first compatible BFL service in configured priority order. reference_images are ordered session blob IDs or public HTTPS URLs: FLUX.2 Pro/Max/Flex accept up to 8; Klein and Kontext accept up to 4. Explain each reference by position in prompt. Omit references for text-to-image. Parameters are model-specific: FLUX.2 supports width/height, seed and output_format; Pro/Max also prompt_upsampling; Flex also prompt_upsampling, steps and guidance. Kontext supports aspect_ratio, seed, output_format and prompt_upsampling. Unsupported parameters fail before submission. Use bfl_fill for masks/outpainting. Returns saved session blob IDs and local paths. Paid submission is never automatically repeated after an unknown outcome.".into(),
    input_schema: json!({"type":"object","additionalProperties":false,"required":["prompt"],"properties":{
      "service":{"type":"string","description":"Optional configured service ID; omitted selects the first compatible service in configured priority order."},"prompt":{"type":"string","minLength":1,"maxLength":32768},
      "reference_images":{"type":"array","maxItems":8,"items":{"type":"string"}},
      "parameters":parameters(false)
    }})
  }
}
pub fn fill_specification() -> Tool {
  Tool {
    name:"bfl_fill".into(),
    description:"Inpaint or outpaint with BFL FLUX.1 Fill. service optionally selects a configured flux-pro-1.0-fill model from extension_tools; omit to use the first compatible Fill service in configured priority order. source_image is one session-owned PNG/JPEG/WebP blob. Optional mask is an opaque black/white PNG blob with the same oriented dimensions: white edits, black preserves. Without mask, source_image must have transparent pixels defining the edited area. For outpainting, first prepare an expanded transparent source canvas or a matching mask for the added area. Public URLs are not accepted. Parameters override this service for this call. Returns saved session blob IDs and local paths. Never automatically repeat an interrupted paid submission.".into(),
    input_schema:json!({"type":"object","additionalProperties":false,"required":["prompt","source_image"],"properties":{
      "service":{"type":"string","description":"Optional configured service ID; omitted selects the first compatible service in configured priority order."},"prompt":{"type":"string","minLength":1,"maxLength":32768},
      "source_image":{"type":"string"},"mask":{"type":"string"},"parameters":parameters(true)
    }})
  }
}
pub const GENERATE: Registration = Registration {
  extension: "bfl",
  name: "bfl_generate",
  summary: "FLUX image generation and multi-reference editing",
  tags: &["image", "generation", "editing", "reference"],
  service_presets: &["bfl_images", "bfl_fill_images"],
  accepts_model: generation_model,
  permission: Permission::ImageGeneration,
  specification: generation_specification,
  examples: || json!([{"service":"<configured BFL service ID>","prompt":"product photograph of the object in image 1, on a wooden table","reference_images":["<session blob ID>"],"parameters":{"seed":42}}]),
  parse: |value| {
    serde_json::from_value::<Generate>(value).map(Invocation::new).map_err(|e| e.to_string())
  },
};
pub const FILL: Registration = Registration {
  extension: "bfl",
  name: "bfl_fill",
  summary: "FLUX Fill masked inpainting and transparent-canvas outpainting",
  tags: &["image", "editing", "inpainting", "outpainting", "mask"],
  service_presets: &["bfl_images", "bfl_fill_images"],
  accepts_model: |model| model == "flux-pro-1.0-fill",
  permission: Permission::ImageGeneration,
  specification: fill_specification,
  examples: || json!([{"service":"<configured BFL Fill service ID>","prompt":"replace the masked jacket with a red coat","source_image":"<session blob ID>","mask":"<session black/white mask blob ID>","parameters":{"steps":30,"guidance":30}}]),
  parse: |value| {
    serde_json::from_value::<Fill>(value).map(Invocation::new).map_err(|e| e.to_string())
  },
};
pub mod endpoint;
pub(crate) mod image;
pub mod preparation;

pub(crate) mod image_driver;

pub(crate) mod execution;

pub(crate) mod package;
