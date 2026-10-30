//! Kimi Code's search (`POST {base}/search`, with the Kimi Code key or sign-in).
//!
//! The service beside the Kimi Code model API, the one Kimi's own coding CLI searches with. The
//! body takes `text_query`, `limit` (1 to 20), `enable_page_crawling` and `timeout_seconds` (the
//! service stops at 30). There are no domain or period filters. The reply's `search_results[]`
//! has `site_name`, `title`, `url`, `snippet` and `date`. Kimi asks clients to identify themselves
//! truthfully, so this sends what the model provider's own calls send.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
  Draft::post_json(&json!({
    "text_query": query.query,
    "limit": query.limit.clamp(1, 20),
    "enable_page_crawling": false,
    "timeout_seconds": 30,
  }))
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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
    id: "kimi_code_search",
    source: super::sources::search,
    recency: false,
    build: |query, _, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
