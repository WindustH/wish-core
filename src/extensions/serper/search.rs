//! Serper, Google results (`POST https://google.serper.dev/search`, key in `X-API-KEY`).
//!
//! The body takes `q`, `num` (ten a page since Google dropped larger pages) and `tbs` for a period
//! (`qdr:d`, `qdr:w`, `qdr:m`, `qdr:y`). Domains are Google's `site:` operators in `q`. Each of
//! `organic[]` has `title`, `link`, `snippet` and sometimes `date` in Google's own words ("Mar 10,
//! 2022", "4 months ago"). Errors are `{"message", "statusCode"}`: 403 for a bad key, 400 "Not
//! enough credits" for a spent balance.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{
  Recency, SearchResult, build_result, with_site_operators,
};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
  let mut body = json!({
    "q": with_site_operators(&query.query, &query.allowed_domains, &query.blocked_domains),
    "num": query.limit.clamp(1, 10),
  });
  if let Some(recency) = query.recency {
    body["tbs"] = json!(match recency {
      Recency::Day => "qdr:d",
      Recency::Week => "qdr:w",
      Recency::Month => "qdr:m",
      Recency::Year => "qdr:y",
    });
  }
  Draft::post_json(&body)
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let Some(results) = body.get("organic").and_then(Value::as_array) else {
    return if body.get("searchParameters").is_some() {
      Ok(Vec::new())
    } else {
      Err(Error::Malformed("serper reply has no organic results".to_owned()))
    };
  };
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["snippet"]),
          published: read_trimmed_text(&result["date"]),
          ..build_result(result, "link", "title")?
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
    id: "serper_search",
    source: super::sources::search,
    recency: true,
    build: |query, _, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
