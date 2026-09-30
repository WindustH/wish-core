//! Jina's search (`POST https://s.jina.ai/`, bearer key; search needs one).
//!
//! The body takes `q` and `num` (up to 20). By default Jina reads every result page and returns its
//! text; `X-Respond-With: no-content` asks for the search results alone, which is what a list of
//! results needs and much faster. Allowed domains go in `X-Site`, which Jina turns into `site:`
//! operators; blocked ones are `-site:` operators in `q`. There is no period filter. `data[]` has
//! `title`, `url`, `description` and, when known, `date` or `publishedTime`. Errors carry `{"code",
//! "name", "status", "message"}`; 402 is a spent balance, and a search that finds nothing is a 422.

use serde_json::{Value, json};

use super::{SearchQuery, SearchResult, build_result, read_result_array, with_site_operators};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  let body = json!({
    "q": with_site_operators(&query.query, &[], &query.blocked_domains),
    "num": query.limit.clamp(1, 20),
  });
  let mut draft = Draft::post_json(&body)?;
  draft.headers.push(("accept".to_owned(), "application/json".to_owned()));
  draft.headers.push(("x-respond-with".to_owned(), "no-content".to_owned()));
  if !query.allowed_domains.is_empty() {
    draft.headers.push(("x-site".to_owned(), query.allowed_domains.join(", ")));
  }
  Ok(draft)
}

pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "data", "jina search")?;
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["description"]),
          published: read_trimmed_text(&result["date"])
            .or_else(|| read_trimmed_text(&result["publishedTime"])),
          ..build_result(result, "url", "title")?
        })
      })
      .collect(),
  )
}
