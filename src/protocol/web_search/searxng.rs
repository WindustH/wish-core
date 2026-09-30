//! SearXNG, self-hosted (`GET {base}/search?format=json`, no key).
//!
//! JSON output must be switched on in the instance (`search.formats: [html, json]` in
//! settings.yml); an instance that does not serve it answers 403 with an HTML page. The query string
//! takes `q`, `time_range` (`day`, `week`, `month`, `year`, applied by the engines that support it)
//! and `pageno`; how many results a page holds is up to the engines. Domains are `site:` operators
//! in `q`. `results[]` has `title`, `url`, `content` and `publishedDate`. When every engine failed
//! the reply is still `200`, with no results and the engines' reasons in `unresponsive_engines`.

use serde_json::Value;

use super::{
  SearchQuery, SearchResult, build_result, date_only, read_result_array, with_site_operators,
};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Draft {
  let mut parameters = vec![
    (
      "q".to_owned(),
      with_site_operators(&query.query, &query.allowed_domains, &query.blocked_domains),
    ),
    ("format".to_owned(), "json".to_owned()),
    ("pageno".to_owned(), "1".to_owned()),
  ];
  if let Some(recency) = query.recency {
    parameters.push(("time_range".to_owned(), recency.get_name().to_owned()));
  }
  let mut draft = Draft::get(parameters);
  // An instance with its limiter on wants a browser's headers.
  draft.headers.push(("accept".to_owned(), "application/json, text/html;q=0.9".to_owned()));
  draft.headers.push(("accept-language".to_owned(), "en-US,en;q=0.9".to_owned()));
  draft
}

/// An instance that does not serve JSON answers 403 with an HTML page, which says nothing of the
/// setting that is missing.
pub(super) fn explain_refusal(status: u16) -> Option<Error> {
  (status == 403).then(|| {
    Error::from_http(
      403,
      None,
      "HTTP 403: this SearXNG instance does not answer in JSON; add `json` to `search.formats` in its settings.yml".to_owned(),
    )
  })
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "results", "searxng")?;
  let found: Vec<SearchResult> = results
    .iter()
    .filter_map(|result| {
      Some(SearchResult {
        snippet: read_trimmed_text(&result["content"]),
        published: read_trimmed_text(&result["publishedDate"]).map(date_only),
        ..build_result(result, "url", "title")?
      })
    })
    .collect();
  let failed: Vec<String> = body["unresponsive_engines"]
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(|engine| {
      let pair = engine.as_array()?;
      Some(format!(
        "{} ({})",
        pair.first()?.as_str()?,
        pair.get(1).and_then(Value::as_str).unwrap_or("failed")
      ))
    })
    .collect();
  if found.is_empty() && !failed.is_empty() {
    return Err(Error::from_in_band(
      None,
      format!("every search engine of the instance failed: {}", failed.join(", ")),
    ));
  }
  Ok(found)
}
