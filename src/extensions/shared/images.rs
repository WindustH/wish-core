//! Encoded image assets and download outcomes; no provider configuration.
pub struct ReferenceImage {
  pub mime: String,
  pub data: String,
}
pub enum ReferenceInput {
  Inline(ReferenceImage),
  Url(String),
}
pub enum EditOverlay {
  Mask(ReferenceImage),
  Markup(ReferenceImage),
}
impl EditOverlay {
  pub fn image(&self) -> &ReferenceImage {
    match self {
      Self::Mask(image) | Self::Markup(image) => image,
    }
  }
  pub fn field(&self) -> &'static str {
    match self {
      Self::Mask(_) => "mask",
      Self::Markup(_) => "markup",
    }
  }
}
pub enum ImageSource {
  Base64(String),
  Url(String),
}
pub enum ImageReply {
  Complete(Vec<ImageSource>),
  PendingTask { task_id: Option<String> },
  Pending { url: String, result_url: Option<String> },
}

/// Discovery/asset-admission metadata. This does not prescribe a tool request schema.
#[derive(Default)]
pub struct ImageCapabilities {
  pub reference_limit: usize,
  pub local_references: bool,
  pub remote_references: bool,
  pub image_required: bool,
  pub mask: bool,
  pub markup: bool,
  pub options: &'static [&'static str],
}

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
