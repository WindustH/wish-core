//! NovelAI precise-reference kinds, supported models and bounded canvas preparation.
pub use crate::extensions::image_runtime::{PreciseReference, ReferenceKind};

pub(crate) fn supported(model: &str) -> bool {
  matches!(model, "nai-diffusion-4-5-full" | "nai-diffusion-4-5-curated")
}
