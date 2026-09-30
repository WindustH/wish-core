//! Bocha 博查 (`POST https://api.bocha.cn/v1/web-search`, bearer key).
//!
//! The body takes `query`, `count` (up to 50), `freshness` (`oneDay`, `oneWeek`, `oneMonth`,
//! `oneYear`, or `noLimit`), and `include` / `exclude`: domains joined by `|`. A success is
//! `{"code": 200, "data": {"webPages": {"value": [...]}}}`, each page with `name` (the title),
//! `url`, `snippet`, `siteName` and `datePublished` - or only `dateLastCrawled`, which is the
//! publication time in Beijing time despite its `Z`. Errors are non-`2xx` with the status repeated
//! as a string `code` and the text in `message`: 401 a bad key, 403 an empty balance, 429 the rate.

use serde_json::{Value, json};

use super::{Recency, SearchQuery, SearchResult, build_result, date_only};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  let mut body = json!({
    "query": query.query,
    "count": query.limit.clamp(1, 50),
    "summary": false,
    "freshness": match query.recency {
      None => "noLimit",
      Some(Recency::Day) => "oneDay",
      Some(Recency::Week) => "oneWeek",
      Some(Recency::Month) => "oneMonth",
      Some(Recency::Year) => "oneYear",
    },
  });
  if !query.allowed_domains.is_empty() {
    body["include"] = json!(query.allowed_domains.join("|"));
  }
  if !query.blocked_domains.is_empty() {
    body["exclude"] = json!(query.blocked_domains.join("|"));
  }
  Draft::post_json(&body)
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  if let Some(code) =
    body.get("code").filter(|code| code.as_i64() != Some(200) && code.as_str() != Some("200"))
  {
    return Err(Error::from_in_band(
      Some(code.to_string().trim_matches('"').to_owned()),
      read_trimmed_text(&body["message"])
        .or_else(|| read_trimmed_text(&body["msg"]))
        .unwrap_or_else(|| "search failed".to_owned()),
    ));
  }
  let Some(pages) = body.pointer("/data/webPages/value").and_then(Value::as_array) else {
    return if body.get("data").is_some() {
      Ok(Vec::new())
    } else {
      Err(Error::Malformed("bocha search reply has no data".to_owned()))
    };
  };
  Ok(
    pages
      .iter()
      .filter_map(|page| {
        Some(SearchResult {
          snippet: read_trimmed_text(&page["snippet"]),
          site: read_trimmed_text(&page["siteName"]),
          published: read_trimmed_text(&page["datePublished"])
            .or_else(|| read_trimmed_text(&page["dateLastCrawled"]))
            .map(date_only),
          ..build_result(page, "url", "name")?
        })
      })
      .collect(),
  )
}
