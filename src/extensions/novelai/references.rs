//! NovelAI precise-reference kinds, supported models and bounded canvas preparation.
pub use crate::extensions::image_runtime::{PreciseReference, ReferenceKind};

pub(crate) fn supported(model: &str) -> bool {
  !matches!(
    model,
    "nai-diffusion-3"
      | "nai-diffusion-2"
      | "nai-diffusion"
      | "safe-diffusion"
      | "nai-diffusion-furry"
      | "nai-diffusion-furry-3"
  )
}
