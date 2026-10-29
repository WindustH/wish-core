//! Tavily (`POST https://api.tavily.com/search`), with a key or in its keyless mode.
//!
//! The same request and reply either way: keyless access sends `X-Tavily-Access-Mode: keyless`
//! instead of a bearer key and is rate-limited by Tavily. The request filters domains
//! (`include_domains`, `exclude_domains`) and periods (`time_range`) itself. Each result carries
//! `title`, `url`, `content` (the snippet) and, with `include_published_date`, a
//! `published_date` in RFC 1123 form. Errors are non-`2xx`, with `{"detail": {"error": ...}}`;
//! 432 and 433 are a spent plan and a spent pay-as-you-go budget.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
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

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub query: String,
  pub allowed_domains: Vec<String>,
  pub blocked_domains: Vec<String>,
  pub recency: Option<crate::extensions::shared::web_search::Recency>,
  pub limit: usize,
}

use crate::extensions::shared::web_search::SearchQuery;
impl From<&SearchQuery> for Request {
  fn from(value: &SearchQuery) -> Self {
    Self {
      query: value.query.clone(),
      allowed_domains: value.allowed_domains.clone(),
      blocked_domains: value.blocked_domains.clone(),
      recency: value.recency,
      limit: value.limit,
    }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::SearchCodec =
  crate::extensions::shared::read_codecs::SearchCodec {
    id: "tavily_search",
    source: super::sources::search,
    recency: true,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };

pub static KEYLESS_CODEC: crate::extensions::shared::read_codecs::SearchCodec =
  crate::extensions::shared::read_codecs::SearchCodec {
    id: "tavily_keyless_search",
    source: super::sources::keyless_search,
    recency: true,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
