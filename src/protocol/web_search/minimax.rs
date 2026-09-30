//! MiniMax Token Plan search (`POST {host}/v1/coding_plan/search`, with the plan's key).
//!
//! The body is `{"q": ...}` alone: at most ten results, no domain or period filters. The reply's
//! `organic[]` has `title`, `link`, `snippet` and `date`. Failures come back as HTTP 200 too, with
//! `base_resp.status_code` other than 0: 1004 or 2049 a bad key (or one from the other region),
//! 1002 the rate, 1008 no balance, 2056 the plan's window used up.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult, build_result};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  Draft::post_json(&json!({"q": query.query}))
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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
