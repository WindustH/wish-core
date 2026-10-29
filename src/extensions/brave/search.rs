//! Brave Search (`GET https://api.search.brave.com/res/v1/web/search`, key in
//! `X-Subscription-Token`).
//!
//! A query string: `q` (at most 600 characters and 75 words), `count` up to 20, `freshness` as
//! `pd`/`pw`/`pm`/`py`, and `text_decorations=false` for snippets without highlight markup. There is
//! no domain parameter; `site:` operators in `q` stand in. Each of `web.results[]` has `title`, `url`,
//! `description`, `profile.name` and, when known, `page_age`. Errors carry
//! `{"type": "ErrorResponse", "error": {"code", "detail", ...}}`, where the code (`QUOTA_LIMITED`,
//! `SUBSCRIPTION_TOKEN_INVALID`, `RATE_LIMITED`, ...) says more than the status.

use serde_json::Value;

use crate::extensions::shared::web_search::{
  Recency, SearchResult, build_result, date_only, with_site_operators,
};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Draft {
  let mut parameters = vec![
    (
      "q".to_owned(),
      with_site_operators(&query.query, &query.allowed_domains, &query.blocked_domains),
    ),
    ("count".to_owned(), query.limit.clamp(1, 20).to_string()),
    ("text_decorations".to_owned(), "false".to_owned()),
    ("result_filter".to_owned(), "web".to_owned()),
  ];
  if let Some(recency) = query.recency {
    let period = match recency {
      Recency::Day => "pd",
      Recency::Week => "pw",
      Recency::Month => "pm",
      Recency::Year => "py",
    };
    parameters.push(("freshness".to_owned(), period.to_owned()));
  }
  Draft::get(parameters)
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let Some(results) = body.pointer("/web/results").and_then(Value::as_array) else {
    // A search with nothing to show has no `web` section at all.
    return if body.get("type").is_some() {
      Ok(Vec::new())
    } else {
      Err(Error::Malformed("brave search reply is not a search response".to_owned()))
    };
  };
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["description"]),
          site: read_trimmed_text(&result["profile"]["name"]),
          published: read_trimmed_text(&result["page_age"]).map(date_only),
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
    id: "brave_search",
    source: super::sources::search,
    recency: true,
    build: |query, _| Ok(build_draft(&query.into())),
    parse: parse_body,
    refusal: |_| None,
  };
