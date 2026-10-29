//! Metaso 秘塔 (`POST https://metaso.cn/api/v1/search`, bearer key).
//!
//! The body takes `q`, `scope` (`webpage` for web pages), `size` and `includeSummary`; there are no
//! domain or period filters, so domains are filtered after the reply. The reply's `webpages[]` has
//! `title`, `link`, `snippet` and `date`. That reply shape is read from Metaso's own playground
//! and the clients built on it, not from a published schema, so it is read loosely.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::SearchResult;
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
  Draft::post_json(&json!({
    "q": query.query,
    "scope": "webpage",
    "size": query.limit.clamp(1, 20),
    "includeSummary": false,
  }))
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub query: String,
  pub limit: usize,
}

use crate::extensions::shared::web_search::SearchQuery;
impl From<&SearchQuery> for Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone(), limit: value.limit }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::SearchCodec =
  crate::extensions::shared::read_codecs::SearchCodec {
    id: "metaso_search",
    source: super::sources::search,
    recency: false,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
