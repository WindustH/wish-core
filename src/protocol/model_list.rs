//! What models a service says it offers, in one shape across providers.
//!
//! A service lists its models in an envelope of its own: an OpenAI-shaped `{object, data}`, a
//! DashScope page with a total, an Anthropic page with forward cursors, a Gemini resource list.
//! This module is the shape those bodies are read into, so a caller can offer a model picker
//! without knowing which service answered.
//!
//! Two rules run through every field:
//!
//! - Ids are kept as the service spelled them, except for the `publishers/<publisher>/models/`
//!   prefix of a Gemini resource name, which is stripped because the wire wants the bare id back.
//! - What cannot be represented is kept as text in [`ModelCatalog::warnings`] rather than guessed
//!   at, and a cursor only ever comes from the service: this reader never invents one.
//!
//! Each service's own reading of its page lives in a module below this one, and
//! [`parse_catalog_page`] picks the one a protocol id names, while `source.rs` states where each
//! of those pages is read from. Bedrock's catalog is read here as
//! well, from a request its source signs with SigV4 - the same mechanism its Converse wire
//! authenticates by.

use serde_json::Value;

use crate::Error;

pub mod anthropic;
pub mod bedrock;
pub mod codex;
pub mod fetch;
pub mod google;
pub mod openai;
pub mod qwen;
mod source;

pub use fetch::{ModelListQuery, fetch};

/// One model a service offers.
#[derive(Clone, Debug)]
pub struct Model {
  /// The id a call passes back, with any resource prefix stripped.
  pub id: String,
  /// Display name, when the service reports one apart from the id.
  pub name: Option<String>,
  /// Who publishes it, when the service says so.
  pub owner: Option<String>,
  /// When it was published, as the service reported it.
  pub created_at: Option<String>,
  /// Largest prompt it takes, when the service reports one.
  pub context_window: Option<u64>,
  /// Largest answer it gives, when the service reports one.
  pub max_output_tokens: Option<u64>,
}

/// One page of a service's model list.
#[derive(Debug)]
pub struct ModelCatalog {
  /// The protocol that read this page.
  pub protocol: ModelListProtocol,
  /// The models on this page, in the order the service listed them.
  pub models: Vec<Model>,
  /// The cursor of the next page, when the service said there is one.
  pub next_cursor: Option<String>,
  /// Fields that were dropped or unknown, kept as text.
  pub warnings: Vec<String>,
}

/// Which service's model list a page is read by.
///
/// One variant per service: the vocabulary this crate knows. [`ModelListProtocol::get_id`] is the same name
/// in text, so the source table and any configuration boundary can carry it and
/// [`FromStr`](std::str::FromStr) brings it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelListProtocol {
  /// The list an OpenAI-compatible service mounts itself.
  OpenAiModels,
  /// The list a Codex subscription's own endpoint serves.
  OpenAiCodexModels,
  /// DashScope's list.
  QwenModels,
  /// Anthropic's list.
  AnthropicModels,
  /// Google's list.
  GoogleModels,
  /// Bedrock's list, which is read with a signed request.
  BedrockModels,
}

impl ModelListProtocol {
  /// Every protocol this crate knows.
  pub const ALL: &[ModelListProtocol] = &[
    ModelListProtocol::OpenAiModels,
    ModelListProtocol::OpenAiCodexModels,
    ModelListProtocol::QwenModels,
    ModelListProtocol::AnthropicModels,
    ModelListProtocol::GoogleModels,
    ModelListProtocol::BedrockModels,
  ];

  /// The name this protocol is known by in text: what the source table, the documentation and a
  /// page's own [`ModelCatalog::protocol`] say.
  pub const fn get_id(self) -> &'static str {
    match self {
      ModelListProtocol::OpenAiModels => "openai_models",
      ModelListProtocol::OpenAiCodexModels => "openai_codex_models",
      ModelListProtocol::QwenModels => "qwen_models",
      ModelListProtocol::AnthropicModels => "anthropic_models",
      ModelListProtocol::GoogleModels => "google_models",
      ModelListProtocol::BedrockModels => "bedrock_models",
    }
  }
}

/// Why a model list cannot be asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
  /// No source serves the protocol: the wire publishes no list a request can ask for.
  NoListing,
}

impl Unsupported {
  /// The reason in the words a caller reads back.
  pub const fn get_text(self) -> &'static str {
    match self {
      Unsupported::NoListing => "publishes no model list a request can ask for",
    }
  }
}

impl std::fmt::Display for ModelListProtocol {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str(self.get_id())
  }
}

impl std::str::FromStr for ModelListProtocol {
  type Err = Error;

  /// Reads back what [`ModelListProtocol::get_id`] wrote, for a boundary that carries text.
  ///
  /// # Errors
  ///
  /// [`Error::Build`] for a name this crate does not know.
  fn from_str(id: &str) -> Result<Self, Self::Err> {
    Self::ALL
      .iter()
      .copied()
      .find(|protocol| protocol.get_id() == id)
      .ok_or_else(|| Error::Build(format!("unknown model list protocol `{id}`")))
  }
}

/// Reads one page of a model list for a known protocol.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when the page is missing a container the protocol cannot work
/// without; a merely absent field stays absent, with a warning.
pub fn parse_catalog_page(
  protocol: ModelListProtocol,
  body: &Value,
) -> Result<ModelCatalog, Error> {
  match protocol {
    ModelListProtocol::OpenAiModels => openai::parse(body),
    ModelListProtocol::OpenAiCodexModels => codex::parse(body),
    ModelListProtocol::QwenModels => qwen::parse(body),
    ModelListProtocol::AnthropicModels => anthropic::parse(body),
    ModelListProtocol::GoogleModels => google::parse(body),
    ModelListProtocol::BedrockModels => bedrock::parse(body),
  }
}

/// The query a page of a model list is asked with: the cursor the previous page handed over, and
/// how many models to ask for where the wire has a page size.
pub fn build_page_query(
  protocol: ModelListProtocol,
  cursor: Option<&str>,
  page_size: u32,
) -> Result<Vec<(String, String)>, Error> {
  match protocol {
    ModelListProtocol::OpenAiModels => Ok(openai::build_page_query(cursor)),
    ModelListProtocol::OpenAiCodexModels => Ok(codex::build_page_query()),
    ModelListProtocol::QwenModels => Ok(qwen::build_page_query(cursor, page_size)),
    ModelListProtocol::AnthropicModels => Ok(anthropic::build_page_query(cursor, page_size)),
    ModelListProtocol::GoogleModels => Ok(google::build_page_query(cursor, page_size)),
    ModelListProtocol::BedrockModels => Ok(bedrock::build_page_query()),
  }
}
