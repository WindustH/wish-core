//! Perplexity's Search API (`POST https://api.perplexity.ai/search`, bearer key).
//!
//! One domain list serves both ways: `"nature.com"` allows a domain and `"-reddit.com"` blocks
//! one, but a request may not mix the two, so blocked domains are sent only when none is allowed
//! (and filtered after the reply otherwise). `search_recency_filter` takes the period by name.
//! Each result has `title`, `url`, `snippet` (sometimes empty or missing) and `date`
//! (`YYYY-MM-DD`). A key that is invalid or out of credits is refused with 401; 429 is the rate.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{SearchResult, build_result, read_result_array};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
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

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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
    id: "perplexity_search",
    source: super::sources::search,
    recency: true,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
