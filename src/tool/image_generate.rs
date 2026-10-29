//! Image generation returns stored artifact paths, never image base64 to the language model.
use crate::{
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
  tool::contract::ToolExecutor,
  utils::control::ExecutionControl,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRequest {
  pub prompt: String,
  #[serde(default)]
  pub options: serde_json::Map<String, Value>,
  #[serde(default)]
  pub images: Vec<String>,
  #[serde(default)]
  pub mask: Option<String>,
  #[serde(default)]
  pub markup: Option<String>,
  #[serde(default)]
  pub references: Vec<PreciseReferenceRequest>,
  #[serde(default)]
  pub provider: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreciseReferenceRequest {
  pub image: String,
  pub kind: crate::extension_host::images::ReferenceKind,
  #[serde(default = "reference_default")]
  pub strength: f64,
  #[serde(default = "reference_default")]
  pub fidelity: f64,
}
fn reference_default() -> f64 {
  1.0
}
pub type Backend = Arc<
  dyn Fn(
      ImageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Value, super::paid_operation::Failure>> + Send>>
    + Send
    + Sync,
>;
pub struct ImageGenerateTool(pub Backend, pub super::providers::Listing);
impl ImageGenerateTool {
  pub fn specification() -> Tool {
    super::providers::specification(Tool{name:"image_generate".into(),description:"Generate images using the configured image service. Describe the desired image or edits in prompt. Supply ordered images as session blob IDs or public HTTPS URLs (OpenAI GPT Image, Seedream, Jimeng, RunningHub and supported BFL models) for reference-guided generation or editing; describe each image by its 1-based position. Reuse output blob IDs for subsequent edits. Check list_providers reference_images before choosing a service. When images are supplied and provider is omitted, choose the first enabled service supporting reference images in configuration order. NovelAI uses images for one image-to-image source; use the separate references array for character/style conditioning on V4.5 models. Each reference is a session blob with kind character, style or character_style, strength and fidelity (0–1; defaults 1). Up to eight references (PNG/JPEG/WebP, at most 4096 pixels per edge) are normalized to the documented padded sizes; they incur additional upstream charges. Multiple character references can blend characters. list_providers exposes precise_references availability. References may be combined with one image-to-image source. Jimeng accepts up to 14 HTTPS image URLs and cannot read local blobs directly; Seedream accepts up to 8 blobs or URLs. BFL FLUX.2 pro/max/flex accepts up to 8 blobs or HTTPS URLs; Klein and Kontext accept up to 4. Other BFL model IDs are not advertised for reference editing. OpenAI GPT Image accepts up to 16 blobs or HTTPS references for prompt-guided editing; this capability does not apply to the ChatGPT/Codex subscription adapter. Gemini and Gemini Interactions accept up to 14 image blobs. RunningHub Seedream 5 Pro accepts up to ten blobs or HTTPS references; local PNG/JPEG/WebP files are normalized to PNG and uploaded before generation (4096-pixel edge and 12 MiB combined prepared limit). Other adapters require blobs. For BFL Fill, supply exactly one source image blob and optionally mask as a same-size opaque black/white PNG blob (white edits, black preserves). Without a mask, the source must have transparent pixels which define the edited region. For outpainting, prepare an enlarged source canvas and mask its added areas. OpenAI GPT Image also accepts a black/white mask applied to the first local source, with extra references allowed. Wish converts white edit regions to transparent alpha. Supported GPT Image 2.5 models accept markup: a transparent PNG annotation overlay matching the first local source dimensions. mask and markup are mutually exclusive. list_providers exposes mask_editing, markup_editing and image_required. Other adapters currently reject mask. RunningHub Seedream accepts output_format (png/jpeg) and either image_size (1K/2K) or width plus height (integers 240-8192). Explicit dimensions replace configured resolution for that call. BFL exposes model-specific width/height, seed, output_format and prompt_upsampling; Flex additionally steps (1–50) and guidance (1.5–10), Fill steps (15–50) and guidance (1.5–100), Kontext aspect_ratio (9:21 through 21:9). Pro/max prompt_upsampling is mapped to the inverse disable_pup flag. Optional options override provider defaults for this call only; list_providers exposes supported image_options. GPT Image accepts size (auto or WIDTHxHEIGHT), quality, background, output_format, output_compression (0-100 for jpeg/webp), n (1-8) and moderation. Model-specific limits apply; GPT Image 1.5/mini additionally expose input_fidelity for edits. Transparent backgrounds require png/webp. xAI Grok Imagine 2.0 accepts up to 5 references, aspect_ratio, image_size (1K/2K), quality (auto/low/medium) and n (1..10); it does not support GPT Image masks/markup. Gemini native and Interactions accept aspect_ratio and image_size (1K, 2K, 4K). Native Gemini 3.1 Flash Image additionally accepts 512 and panoramic ratios 1:4/4:1/1:8/8:1; native 2.5 Flash Image only accepts aspect_ratio. NovelAI accepts width/height (multiples of 64, 64–2048), steps (1–50), n_samples (1–8), scale (0–10), seed (unsigned 32-bit integer), negative_prompt (text), strength/noise (0–1, require a source image). Unsupported options are rejected before submission; automatic selection skips incompatible providers. Returns images saved in this session's Wish blob store, including absolute local path, blob id and MIME type. Use the returned paths for further processing; do not claim to have visually inspected the images unless you read them. Generation may incur charges. If a request is interrupted or times out, do not automatically repeat it: upstream may have completed it.".into(),input_schema:json!({"type":"object","additionalProperties":false,"required":["prompt"],"properties":{"markup":{"type":"string","description":"Session-owned transparent PNG annotation overlay for supported GPT Image models; same oriented dimensions as the first local source image. Cannot combine with mask."},"mask":{"type":"string","description":"Session-owned opaque black/white PNG mask blob, same dimensions as the first local source image. White edits and black preserves; Wish converts to upstream alpha semantics when necessary. Cannot combine with markup."},"references":{"type":"array","maxItems":8,"description":"NovelAI V4.5 precise character/style references, distinct from the image-to-image source. Combined input limit 12 MiB; reference images must be PNG/JPEG/WebP with each edge at most 4096 pixels. Additional upstream fees apply.","items":{"type":"object","additionalProperties":false,"required":["image","kind"],"properties":{"image":{"type":"string","description":"Session-owned image blob ID; HTTPS URLs are not supported here."},"kind":{"type":"string","enum":["character","style","character_style"]},"strength":{"type":"number","minimum":0,"maximum":1},"fidelity":{"type":"number","minimum":0,"maximum":1}}}},"options":image_options(),"images":{"type":"array","maxItems":16,"items":{"type":"string"},"description":"Ordered session image blob IDs, or public HTTPS URLs for OpenAI/xAI/Seedream/Jimeng/BFL/RunningHub/Wan. Provider limits apply. Omit for text-to-image."},"prompt":{"type":"string"},"provider":{"type":"string","description":"Optional configured image service ID; omit to use the first enabled service."}}})})
  }
}
impl ToolExecutor for ImageGenerateTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let arguments =
      match super::providers::prepare(&call.arguments, &Self::specification().input_schema) {
        Ok(arguments) => arguments,
        Err(error) => return ToolOutcome::Failed(error),
      };
    if let Some(outcome) = super::providers::listing(&arguments, &self.1) {
      return outcome;
    }
    let args = match serde_json::from_value::<ImageRequest>(arguments) {
      Ok(args) => args,
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    match control.run_until_cancelled((self.0)(args)).await {
      None => ToolOutcome::Unknown(
        "Image request cancelled; upstream may still complete it. Do not automatically resubmit."
          .into(),
      ),
      Some(Err(e)) => e.outcome(),
      Some(Ok(output)) => ToolOutcome::Report {
        text: format!("Generated images saved in Wish blob storage:\n{output}"),
        metadata: output.clone(),
        output,
        failed: false,
      },
    }
  }
}

fn image_options() -> Value {
  json!({"type":"object","description":"Per-call image controls. Use only keys advertised in the selected provider image_options; these override defaults without changing configuration.","additionalProperties":false,"properties":{"enable_sequential":{"type":"boolean","description":"Wan only: generate a related image set, up to n images."},"watermark":{"type":"boolean","description":"Wan visible watermark."},"thinking_mode":{"type":"boolean","description":"Wan only; effective for text-to-image without image sets."},"color_palette":{"type":"array","minItems":3,"maxItems":10,"description":"Wan only, incompatible with image sets; ratios must total 100.00%.","items":{"type":"object","additionalProperties":false,"required":["hex","ratio"],"properties":{"hex":{"type":"string","pattern":"^#[0-9A-Fa-f]{6}$"},"ratio":{"type":"string","pattern":"^[0-9]{1,3}\\.[0-9]{2}%$"}}}},"bbox_list":{"type":"array","minItems":1,"maxItems":9,"description":"Wan 2.7 only: one array per image, with up to two [x1,y1,x2,y2] pixel boxes. Use oriented local image dimensions; an empty array leaves that image unboxed.","items":{"type":"array","maxItems":2,"items":{"type":"array","minItems":4,"maxItems":4,"items":{"type":"integer","minimum":0,"maximum":8000}}}},"size":{"type":"string","maxLength":32},"quality":{"type":"string","enum":["auto","low","medium","high","xhigh","max"]},"background":{"type":"string","enum":["auto","opaque","transparent"]},"output_format":{"type":"string","enum":["png","jpeg","webp"]},"output_compression":{"type":"integer","minimum":0,"maximum":100},"n":{"type":"integer","minimum":1,"maximum":12,"description":"Wan 1..4 normally, up to 12 with enable_sequential. xAI supports 1..10. Other providers have lower limits."},"moderation":{"type":"string","enum":["auto","low"]},"input_fidelity":{"type":"string","enum":["low","high"]},"aspect_ratio":{"type":"string","enum":["1:1","2:3","3:2","3:4","4:3","4:5","5:4","9:16","16:9","21:9","auto","2:1","1:2","19.5:9","9:19.5","20:9","9:20","5:2","1:8","8:1","1:4","4:1"]},"image_size":{"type":"string","enum":["512","1K","2K","4K"]},"width":{"type":"integer","minimum":64,"maximum":8192},"height":{"type":"integer","minimum":64,"maximum":8192},"steps":{"type":"integer","minimum":1,"maximum":50},"n_samples":{"type":"integer","minimum":1,"maximum":8},"scale":{"type":"number","minimum":0,"maximum":10},"seed":{"type":"integer","minimum":0,"maximum":4294967295u64},"negative_prompt":{"type":"string","maxLength":32768},"strength":{"type":"number","minimum":0,"maximum":1},"prompt_upsampling":{"type":"boolean"},"guidance":{"type":"number","minimum":1.5,"maximum":100},"noise":{"type":"number","minimum":0,"maximum":1}}})
}
