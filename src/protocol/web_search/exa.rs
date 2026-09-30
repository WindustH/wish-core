//! Exa (`POST https://api.exa.ai/search`, key in `x-api-key`).
//!
//! Domains are filtered by the request (`includeDomains`, `excludeDomains`); a period is a start
//! date (`startPublishedDate`, ISO 8601), since Exa has no named recency. Without `contents` a
//! reply carries no text at all, so the request asks for `highlights`, the passages it judges most
//! relevant, which serve as the snippet. Each result has `title`, `url`, `highlights[]` and, when
//! known, `publishedDate`. Errors are non-`2xx` with `{"error": message, "tag": ...}`; 402 is a
//! spent balance or budget.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult, build_result, date_only, format_date, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery, now: u64) -> Result<Draft, Error> {
  let mut body = json!({
    "query": query.query,
    "numResults": query.limit.clamp(1, 20),
    "type": "auto",
    "contents": {"highlights": {"maxCharacters": 600}},
  });
  if !query.allowed_domains.is_empty() {
    body["includeDomains"] = json!(query.allowed_domains);
  }
  if !query.blocked_domains.is_empty() {
    body["excludeDomains"] = json!(query.blocked_domains);
  }
  if let Some(recency) = query.recency {
    let start = now.saturating_sub(recency.get_days() * 86_400);
    body["startPublishedDate"] = json!(format!("{}T00:00:00.000Z", format_date(start)));
  }
  Draft::post_json(&body)
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "results", "exa search")?;
  Ok(
    results
      .iter()
      .filter_map(|result| {
        let highlights: Vec<&str> = result["highlights"]
          .as_array()
          .into_iter()
          .flatten()
          .filter_map(Value::as_str)
          .map(str::trim)
          .filter(|text| !text.is_empty())
          .collect();
        Some(SearchResult {
          snippet: (!highlights.is_empty())
            .then(|| highlights.join(" … "))
            .or_else(|| read_trimmed_text(&result["summary"])),
          published: read_trimmed_text(&result["publishedDate"]).map(date_only),
          ..build_result(result, "url", "title")?
        })
      })
      .collect(),
  )
}
