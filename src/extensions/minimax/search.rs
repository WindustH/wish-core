//! MiniMax Token Plan search (`POST {host}/v1/coding_plan/search`, with the plan's key).
//!
//! The body is `{"q": ...}` alone: at most ten results, no domain or period filters. The reply's
//! `organic[]` has `title`, `link`, `snippet` and `date`. Failures come back as HTTP 200 too, with
//! `base_resp.status_code` other than 0: 1004 or 2049 a bad key (or one from the other region),
//! 1002 the rate, 1008 no balance, 2056 the plan's window used up.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{SearchResult, build_result};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
  Draft::post_json(&json!({"q": query.query}))
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  if let Some(code) =
    body.pointer("/base_resp/status_code").and_then(Value::as_i64).filter(|code| *code != 0)
  {
    return Err(Error::from_in_band(
      Some(code.to_string()),
      read_trimmed_text(&body["base_resp"]["status_msg"])
        .unwrap_or_else(|| "search failed".to_owned()),
    ));
  }
  let Some(results) = body.get("organic").and_then(Value::as_array) else {
    return if body.get("base_resp").is_some() {
      Ok(Vec::new())
    } else {
      Err(Error::Malformed("minimax search reply has no organic results".to_owned()))
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
}

use crate::extensions::shared::web_search::SearchQuery;
impl From<&SearchQuery> for Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone() }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::SearchCodec =
  crate::extensions::shared::read_codecs::SearchCodec {
    id: "minimax_coding_plan_search",
    source: super::sources::search,
    recency: false,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
