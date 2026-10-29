//! Search result presentation and query-building primitives. No provider registry.
use crate::protocol::error::Error;
use crate::utils::time::civil_date;
use serde::{Deserialize, Serialize};
use serde_json::Value;
/// How far back results may date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recency {
  Day,
  Week,
  Month,
  Year,
}

impl Recency {
  /// The period's name, as most services spell it.
  pub fn get_name(self) -> &'static str {
    match self {
      Self::Day => "day",
      Self::Week => "week",
      Self::Month => "month",
      Self::Year => "year",
    }
  }

  /// The period in days, for the services that take a count.
  pub fn get_days(self) -> u64 {
    match self {
      Self::Day => 1,
      Self::Week => 7,
      Self::Month => 31,
      Self::Year => 366,
    }
  }
}

/// One page a search found.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SearchResult {
  pub title: String,
  pub url: String,
  /// A short excerpt of the page, as the service chose it.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub snippet: Option<String>,
  /// The site's own name, when the service reports one.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub site: Option<String>,
  /// When the page was published, in the service's own format.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub published: Option<String>,
}

/// What a search found, and what it could not do as asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SearchResults {
  pub results: Vec<SearchResult>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub warnings: Vec<String>,
}

/// The query with search-engine operators for the domains, for a service without parameters of
/// its own for them: `site:` for one allowed domain, `(site:a OR site:b)` for several, `-site:` for
/// each blocked one. A handful at most, so the query stays within what engines accept; the rest
/// are filtered after the reply.
pub(crate) fn with_site_operators(query: &str, allowed: &[String], blocked: &[String]) -> String {
  const MOST: usize = 5;
  let mut text = query.to_owned();
  match allowed {
    [] => {}
    [domain] => text.push_str(&format!(" site:{domain}")),
    domains => {
      let sites: Vec<String> =
        domains.iter().take(MOST).map(|domain| format!("site:{domain}")).collect();
      text.push_str(&format!(" ({})", sites.join(" OR ")));
    }
  }
  for domain in blocked.iter().take(MOST) {
    text.push_str(&format!(" -site:{domain}"));
  }
  text
}

/// The array a reply lists its results in, under `key`, or the malformed reply `service` names
/// when it has none.
pub(crate) fn read_result_array<'a>(
  body: &'a Value,
  key: &str,
  service: &str,
) -> Result<&'a Vec<Value>, Error> {
  body
    .get(key)
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed(format!("{service} reply has no {key}")))
}

/// One result's address and title, read from the members this service names them by; `None` for
/// a result without an address, which is no result at all. The rest of the result is the
/// service's own to read.
pub(crate) fn build_result(item: &Value, url_key: &str, title_key: &str) -> Option<SearchResult> {
  Some(SearchResult {
    url: item[url_key].as_str()?.to_owned(),
    title: item[title_key].as_str().unwrap_or_default().to_owned(),
    snippet: None,
    site: None,
    published: None,
  })
}

/// The date part of a timestamp a service wrote as an ISO date or date-time: its first ten
/// characters.
pub(crate) fn date_only(timestamp: String) -> String {
  timestamp.chars().take(10).collect()
}

/// A time in seconds as its UTC date, `YYYY-MM-DD`.
pub(crate) fn format_date(seconds: u64) -> String {
  let (year, month, day) = civil_date(seconds / 86_400);
  format!("{year:04}-{month:02}-{day:02}")
}

/// One search, as the tool was asked for it.
#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
  pub query: String,
  /// Only pages on these domains (and their subdomains). Empty for any domain.
  pub allowed_domains: Vec<String>,
  /// Never pages on these domains (or their subdomains).
  pub blocked_domains: Vec<String>,
  pub recency: Option<Recency>,
  /// The most results wanted.
  pub limit: usize,
  /// The conversation the search belongs to, for a service that keeps one across searches
  /// (Codex addresses later page reads by it).
  pub conversation: Option<String>,
  /// The model the search is made for, for a service whose request names one.
  pub model: Option<String>,
}
