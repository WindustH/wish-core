//! Tavily (`POST https://api.tavily.com/search`), with a key or in its keyless mode.
//!
//! The same request and reply either way: keyless access sends `X-Tavily-Access-Mode: keyless`
//! instead of a bearer key and is rate-limited by Tavily. The request filters domains
//! (`include_domains`, `exclude_domains`) and periods (`time_range`) itself. Each result carries
//! `title`, `url`, `content` (the snippet) and, with `include_published_date`, a
//! `published_date` in RFC 1123 form. Errors are non-`2xx`, with `{"detail": {"error": ...}}`;
//! 432 and 433 are a spent plan and a spent pay-as-you-go budget.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  let mut body = json!({
    "query": query.query,
    "max_results": query.limit.clamp(1, 20),
    "search_depth": "basic",
    "topic": "general",
    "include_published_date": true,
  });
  if !query.allowed_domains.is_empty() {
    body["include_domains"] = json!(query.allowed_domains);
  }
  if !query.blocked_domains.is_empty() {
    body["exclude_domains"] = json!(query.blocked_domains);
  }
  if let Some(recency) = query.recency {
    body["time_range"] = json!(recency.get_name());
  }
  Draft::post_json(&body)
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "results", "tavily search")?;
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["content"]),
          published: read_trimmed_text(&result["published_date"]),
          ..build_result(result, "url", "title")?
        })
      })
      .collect(),
  )
}
