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

use super::{SearchQuery, SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

/// Answered like any other; Codex itself sends the model of the turn.
const DEFAULT_MODEL: &str = "gpt-5.5";

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
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

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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
