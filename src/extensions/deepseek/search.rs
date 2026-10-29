//! DeepSeek's search (`POST https://api.deepseek.com/v1/tools/search`, with a DeepSeek API key).
//!
//! The body takes `query` (1 to 128 characters) and `count` (1 to 50). There are no filter
//! parameters, but the query takes search-engine operators, so the domains travel as `site:` and
//! `-site:` and a period as `after:` with its start date. The reply's `results[]` has `title`,
//! `url`, `snippet` (a line for display), `summary` (several hundred characters written for a model
//! to read), `site_name` and `date` (RFC 3339); the summary is the snippet here, since a model is
//! what reads it. Errors are non-`2xx` in OpenAI's envelope; 402 is a spent balance.

use serde_json::{Value, json};

use crate::extensions::shared::web_search::{
  SearchResult, build_result, date_only, format_date, read_result_array, with_site_operators,
};
use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::protocol::json_read::read_trimmed_text;

pub(crate) fn build_draft(query: &Request, now: u64) -> Result<Draft, Error> {
  let mut text = with_site_operators(&query.query, &query.allowed_domains, &query.blocked_domains);
  if let Some(recency) = query.recency {
    let start = now.saturating_sub(recency.get_days() * 86_400);
    text.push_str(&format!(" after:{}", format_date(start)));
  }
  Draft::post_json(&json!({"query": text, "count": query.limit.clamp(1, 50)}))
}

pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let results = read_result_array(body, "results", "deepseek search")?;
  Ok(
    results
      .iter()
      .filter_map(|result| {
        Some(SearchResult {
          snippet: read_trimmed_text(&result["summary"])
            .or_else(|| read_trimmed_text(&result["snippet"])),
          site: read_trimmed_text(&result["site_name"]),
          published: read_trimmed_text(&result["date"]).map(date_only),
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
