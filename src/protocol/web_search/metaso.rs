//! Metaso 秘塔 (`POST https://metaso.cn/api/v1/search`, bearer key).
//!
//! The body takes `q`, `scope` (`webpage` for web pages), `size` and `includeSummary`; there are no
//! domain or period filters, so domains are filtered after the reply. The reply's `webpages[]` has
//! `title`, `link`, `snippet` and `date`. That reply shape is read from Metaso's own playground
//! and the clients built on it, not from a published schema, so it is read loosely.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  Draft::post_json(&json!({
    "q": query.query,
    "scope": "webpage",
    "size": query.limit.clamp(1, 20),
    "includeSummary": false,
  }))
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let pages = body
    .get("webpages")
    .or_else(|| body.pointer("/data/webpages"))
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("metaso search reply has no webpages".to_owned()))?;
  Ok(
    pages
      .iter()
      .filter_map(|page| {
        Some(SearchResult {
          url: page["link"].as_str().or_else(|| page["url"].as_str())?.to_owned(),
          title: page["title"].as_str().unwrap_or_default().to_owned(),
          snippet: read_trimmed_text(&page["snippet"])
            .or_else(|| read_trimmed_text(&page["summary"])),
          site: None,
          published: read_trimmed_text(&page["date"]),
        })
      })
      .collect(),
  )
}
