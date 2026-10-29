//! NovelAI precise-reference kinds, supported models and bounded canvas preparation.
use crate::extensions::shared::images::ReferenceImage;
use serde::Deserialize;
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
  Character,
  Style,
  CharacterStyle,
}
impl ReferenceKind {
  pub fn caption(self) -> &'static str {
    match self {
      Self::Character => "character",
      Self::Style => "style",
      Self::CharacterStyle => "character&style",
    }
  }
}
pub struct PreciseReference {
  pub image: ReferenceImage,
  pub kind: ReferenceKind,
  pub strength: f64,
  pub fidelity: f64,
}

pub(crate) fn supported(model: &str) -> bool {
  matches!(model, "nai-diffusion-4-5-full" | "nai-diffusion-4-5-curated")
}
