//! Kimi Code's search (`POST {base}/search`, with the Kimi Code key or sign-in).
//!
//! The service beside the Kimi Code model API, the one Kimi's own coding CLI searches with. The
//! body takes `text_query`, `limit` (1 to 20), `enable_page_crawling` and `timeout_seconds` (the
//! service stops at 30). There are no domain or period filters. The reply's `search_results[]`
//! has `site_name`, `title`, `url`, `snippet` and `date`. Kimi asks clients to identify themselves
//! truthfully, so this sends what the model provider's own calls send.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  Draft::post_json(&json!({
    "text_query": query.query,
    "limit": query.limit.clamp(1, 20),
    "enable_page_crawling": false,
    "timeout_seconds": 30,
  }))
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "search_results", "kimi search")?;
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["snippet"]),
          site: read_trimmed_text(&result["site_name"]),
          published: read_trimmed_text(&result["date"]),
          ..build_result(result, "url", "title")?
        })
      })
      .collect(),
  )
}
