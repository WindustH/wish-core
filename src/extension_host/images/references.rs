//! Compatibility queries for the legacy image service. NovelAI owns reference semantics.
use super::ImageProtocol;
pub use crate::extensions::novelai::references::{PreciseReference, ReferenceKind};
impl ImageProtocol {
  pub fn precise_references(self, model: &str) -> bool {
    self == Self::Novelai && crate::extensions::novelai::references::supported(model)
  }
}
