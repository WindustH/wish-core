//! Perplexity's Search API (`POST https://api.perplexity.ai/search`, bearer key).
//!
//! One domain list serves both ways: `"nature.com"` allows a domain and `"-reddit.com"` blocks
//! one, but a request may not mix the two, so blocked domains are sent only when none is allowed
//! (and filtered after the reply otherwise). `search_recency_filter` takes the period by name.
//! Each result has `title`, `url`, `snippet` (sometimes empty or missing) and `date`
//! (`YYYY-MM-DD`). A key that is invalid or out of credits is refused with 401; 429 is the rate.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  let mut body = json!({"query": query.query, "max_results": query.limit.clamp(1, 20)});
  let domains: Vec<String> = if query.allowed_domains.is_empty() {
    query.blocked_domains.iter().map(|domain| format!("-{domain}")).collect()
  } else {
    query.allowed_domains.clone()
  };
  if !domains.is_empty() {
    body["search_domain_filter"] = json!(domains.into_iter().take(20).collect::<Vec<_>>());
  }
  if let Some(recency) = query.recency {
    body["search_recency_filter"] = json!(recency.get_name());
  }
  Draft::post_json(&body)
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "results", "perplexity search")?;
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["snippet"]),
          published: read_trimmed_text(&result["date"]),
          ..build_result(result, "url", "title")?
        })
      })
      .collect(),
  )
}
