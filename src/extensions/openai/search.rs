//! ChatGPT's search for Codex (`POST {base}/alpha/search`).
//!
//! The request names a conversation (`id`) - later page reads refer to its results by `ref_id` -
//! and a model, which the service does not check: any name is answered the same way. `commands`
//! carries up to four queries; `recency` is a number of days and `domains` the allowed ones, while
//! blocked domains go in `settings.filters`.
//!
//! The reply's `output` is a digest written for GPT, with private-use citation marks and crawl
//! notes that other models read as noise, so only `results` is read: `{type: "text_result", domain,
//! ref_id, snippet, title, url}`, where other types (images, finance cards) are left out.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

/// Answered like any other; Codex itself sends the model of the turn.
const DEFAULT_MODEL: &str = "gpt-5.5";

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
  let mut search = json!({"q": query.query});
  if let Some(recency) = query.recency {
    search["recency"] = json!(recency.get_days());
  }
  if !query.allowed_domains.is_empty() {
    search["domains"] = json!(query.allowed_domains);
  }
  let mut body = json!({
    "id": query.conversation.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
    "model": query.model.as_deref().unwrap_or(DEFAULT_MODEL),
    "commands": {"search_query": [search], "response_length": "short"},
  });
  if !query.blocked_domains.is_empty() {
    body["settings"] = json!({"filters": {"blocked_domains": query.blocked_domains}});
  }
  Draft::post_json(&body)
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "results", "codex search")?;
  Ok(
    results
      .iter()
      .filter(|result| result["type"].as_str().is_none_or(|kind| kind == "text_result"))
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["snippet"]),
          site: read_trimmed_text(&result["domain"]),
          ..build_result(result, "url", "title")?
        })
      })
      .collect(),
  )
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub query: String,
  pub allowed_domains: Vec<String>,
  pub blocked_domains: Vec<String>,
  pub recency: Option<crate::extensions::shared::web_search::Recency>,
  pub conversation: Option<String>,
  pub model: Option<String>,
}

use crate::extensions::shared::web_search::SearchQuery;
impl From<&SearchQuery> for Request {
  fn from(value: &SearchQuery) -> Self {
    Self {
      query: value.query.clone(),
      allowed_domains: value.allowed_domains.clone(),
      blocked_domains: value.blocked_domains.clone(),
      recency: value.recency,
      conversation: value.conversation.clone(),
      model: value.model.clone(),
    }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::SearchCodec =
  crate::extensions::shared::read_codecs::SearchCodec {
    id: "codex_alpha_search",
    source: super::sources::search,
    recency: true,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
