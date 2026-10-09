//! What models a service says it offers, in one shape across providers.
//!
//! A service lists its models in an envelope of its own: an OpenAI-shaped `{object, data}`, a
//! DashScope page with a total, an Anthropic page with forward cursors, a Gemini resource list.
//! This module is the shape those bodies are read into, so a caller can offer a model picker
//! without knowing which service answered.
//!
//! Two rules run through every field:
//!
//! - Ids are kept as the service spelled them, except for the `models/` or
//!   `publishers/<publisher>/models/` prefix of a Google resource name, which is stripped because
//!   the wire wants the bare id back.
//! - What cannot be represented is kept as text in [`ModelListPage::warnings`] rather than guessed
//!   at. A cursor comes from the service, except Qwen's next page number, which follows from the
//!   page's own `page_no`, `page_size` and `total`.
//!
//! Each service's own reading of its page lives in a module below this one, and [`parse_page`]
//! picks the one a protocol id names, while `source.rs` states where each of those pages is read
//! from and [`build_page_query`] how the next one is asked for. Bedrock's list is read here as
//! well, from a request its source signs with SigV4 - the same mechanism its Converse wire
//! authenticates by.

use serde_json::Value;

use crate::Error;

pub mod anthropic;
pub mod bedrock;
pub mod codex;
pub mod copilot;
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

impl Model {
  /// A model known by `id`, before anything else its entry says is read into it.
  pub(crate) fn new(id: impl Into<String>) -> Self {
    Self {
      id: id.into(),
      name: None,
      owner: None,
      created_at: None,
      context_window: None,
      max_output_tokens: None,
    }
  }
}

/// One page of a service's model list.
#[derive(Clone, Debug)]
pub struct ModelListPage {
  /// The protocol that read this page.
  pub protocol: ModelListProtocol,
  /// The models on this page, in the order the service listed them.
  pub models: Vec<Model>,
  /// The cursor of the next page, when the service said there is one.
  pub next_cursor: Option<String>,
  /// Fields that were dropped or unknown, kept as text.
  pub warnings: Vec<String>,
}

text_id_enum! {
  /// Which service's model list a page is read by.
  ///
  /// One variant per service: the vocabulary this crate knows. The text id is what the source
  /// table, the documentation and any configuration boundary carry.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  #[allow(clippy::enum_variant_names)] // The variants spell their ids.
  pub enum ModelListProtocol (unknown: "unknown model list protocol `{}`") {
    /// The list an OpenAI-compatible service mounts itself.
    OpenAiModels => "openai_models",
    /// The list a Codex subscription's own endpoint serves.
    OpenAiCodexModels => "openai_codex_models",
    /// DashScope's list.
    QwenModels => "qwen_models",
    /// Anthropic's list.
    AnthropicModels => "anthropic_models",
    /// Google's list.
    GoogleModels => "google_models",
    /// Bedrock's list, which is read with a signed request.
    BedrockModels => "bedrock_models",
    /// The models a GitHub Copilot subscription may pick.
    GitHubCopilotModels => "github_copilot_models",
  }
}

/// Reads one page of a model list for a known protocol.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when the page is missing a container the protocol cannot work
/// without; a merely absent field stays absent, with a warning.
pub fn parse_page(protocol: ModelListProtocol, body: &Value) -> Result<ModelListPage, Error> {
  match protocol {
    ModelListProtocol::OpenAiModels => openai::parse(body),
    ModelListProtocol::OpenAiCodexModels => codex::parse(body),
    ModelListProtocol::QwenModels => qwen::parse(body),
    ModelListProtocol::AnthropicModels => anthropic::parse(body),
    ModelListProtocol::GoogleModels => google::parse(body),
    ModelListProtocol::BedrockModels => bedrock::parse(body),
    ModelListProtocol::GitHubCopilotModels => copilot::parse(body),
  }
}

/// The query a page of a model list is asked with: the cursor the previous page handed over, and
/// how many models to ask for where the wire has a page size.
pub fn build_page_query(
  protocol: ModelListProtocol,
  cursor: Option<&str>,
  page_size: u32,
) -> Vec<(String, String)> {
  match protocol {
    ModelListProtocol::OpenAiModels => openai::build_page_query(cursor),
    ModelListProtocol::OpenAiCodexModels => codex::build_page_query(),
    ModelListProtocol::GitHubCopilotModels => Vec::new(),
    ModelListProtocol::QwenModels => qwen::build_page_query(cursor, page_size),
    ModelListProtocol::AnthropicModels => anthropic::build_page_query(cursor, page_size),
    ModelListProtocol::GoogleModels => google::build_page_query(cursor, page_size),
    ModelListProtocol::BedrockModels => bedrock::build_page_query(),
  }
}

/// One entry of a page that a call can use: its id, beside the entry itself.
pub(crate) type Entry<'a> = (&'a str, &'a Value);

/// The entries of a page that a call can use, in the order the service listed them, read from the
/// `container` array with their ids under `id_key`.
///
/// An entry without an id is dropped with a warning, because a model that cannot be called is not
/// a model; the warnings are the page's first ones, in the order the entries came.
///
/// # Errors
///
/// [`Error::Malformed`] when `container` is missing or is not an array, named as `service`'s.
pub(crate) fn read_entries<'a>(
  body: &'a Value,
  container: &str,
  id_key: &str,
  service: &str,
) -> Result<(Vec<Entry<'a>>, Vec<String>), Error> {
  let entries = body
    .get(container)
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed(format!("{service} model list missing `{container}` array")))?;
  let article = if id_key.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
  let mut read = Vec::new();
  let mut warnings = Vec::new();
  for entry in entries {
    match entry.get(id_key).and_then(Value::as_str).filter(|id| !id.is_empty()) {
      Some(id) => read.push((id, entry)),
      None => warnings.push(format!("an entry without {article} `{id_key}` was dropped")),
    }
  }
  Ok((read, warnings))
}

/// The forward cursor of a page that pages by id: its `last_id`, while `has_more` says a next page
/// exists - an id on its own promises none.
pub(crate) fn read_forward_cursor(body: &Value) -> Option<String> {
  match body.get("has_more").and_then(Value::as_bool) {
    Some(true) => body.get("last_id").and_then(Value::as_str).map(str::to_owned),
    _ => None,
  }
}
