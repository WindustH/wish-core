//! Web search in one shape across services.
//!
//! A search service answers in its own vocabulary: Codex's `results` beside a digest written for
//! GPT, Tavily's scored `results`, Brave's `web.results`, Bocha's Bing-like `webPages.value`. This
//! module is the query they are all asked with and the results they are all read into, so the tool
//! that serves the model and the page that shows the results never learn which service answered.
//!
//! Each service's own spelling of a query and reading of a reply lives in a module below this one,
//! and where each is asked is `source.rs`, this tree's slice of the read-side table. The domains a
//! search asked for are filtered here after every reply, whatever the service could do with them
//! itself, and a period a service cannot filter by is said in [`SearchResults::warnings`] rather
//! than quietly ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;
use crate::utils::time::civil_date;

mod bocha;
mod brave;
mod codex;
mod exa;
pub mod fetch;
mod jina;
mod kimi;
mod metaso;
mod minimax;
mod perplexity;
mod searxng;
mod serper;
mod source;
mod tavily;

pub use fetch::search;

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

text_id_enum! {
  /// Every search service this crate can ask.
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
  #[allow(clippy::enum_variant_names)] // The variants spell their ids.
  pub enum SearchProtocol (unknown: "unknown web search protocol: {}") {
    /// ChatGPT's search for Codex: `POST {base}/alpha/search`, with the subscription's token.
    CodexAlphaSearch => "codex_alpha_search",
    /// Tavily with a key.
    TavilySearch => "tavily_search",
    /// Tavily without one: free and rate-limited.
    TavilyKeylessSearch => "tavily_keyless_search",
    /// Exa's search.
    ExaSearch => "exa_search",
    /// Perplexity's Search API.
    PerplexitySearch => "perplexity_search",
    /// Brave's web search.
    BraveSearch => "brave_search",
    /// Google results through Serper.
    SerperSearch => "serper_search",
    /// Jina's search.
    JinaSearch => "jina_search",
    /// A self-hosted SearXNG instance.
    SearxngSearch => "searxng_search",
    /// Bocha 博查's web search.
    BochaSearch => "bocha_search",
    /// Metaso 秘塔's search.
    MetasoSearch => "metaso_search",
    /// Kimi Code's search, with the subscription's key.
    KimiCodeSearch => "kimi_code_search",
    /// MiniMax Token Plan's search, with the plan's key.
    MinimaxCodingPlanSearch => "minimax_coding_plan_search",
  }
}

impl SearchProtocol {
  /// The service's own address, for a protocol with one; `None` when the caller must say where
  /// the service is (a self-hosted one, or a subscription's beside its model API).
  pub fn get_default_base_url(self) -> Option<&'static str> {
    source::find_source(self).base_url
  }

  /// Whether the service can limit results to a period itself. What it cannot is said in the
  /// results' warnings; domains need no such answer, because the results are filtered by domain
  /// after every reply, whatever the service did with them.
  pub fn can_filter_recency(self) -> bool {
    match self {
      Self::CodexAlphaSearch
      | Self::TavilySearch
      | Self::TavilyKeylessSearch
      | Self::ExaSearch
      | Self::PerplexitySearch
      | Self::BraveSearch
      | Self::SerperSearch
      | Self::SearxngSearch
      | Self::BochaSearch => true,
      Self::JinaSearch
      | Self::MetasoSearch
      | Self::KimiCodeSearch
      | Self::MinimaxCodingPlanSearch => false,
    }
  }

  /// The request for one search, besides where it goes and how it is proven. `now` is the time
  /// in seconds, for a service that takes a period as a start date.
  fn build_draft(self, query: &SearchQuery, now: u64) -> Result<Draft, Error> {
    match self {
      Self::CodexAlphaSearch => codex::build_draft(query),
      Self::TavilySearch | Self::TavilyKeylessSearch => tavily::build_draft(query),
      Self::ExaSearch => exa::build_draft(query, now),
      Self::PerplexitySearch => perplexity::build_draft(query),
      Self::BraveSearch => Ok(brave::build_draft(query)),
      Self::SerperSearch => serper::build_draft(query),
      Self::JinaSearch => jina::build_draft(query),
      Self::SearxngSearch => Ok(searxng::build_draft(query)),
      Self::BochaSearch => bocha::build_draft(query),
      Self::MetasoSearch => metaso::build_draft(query),
      Self::KimiCodeSearch => kimi::build_draft(query),
      Self::MinimaxCodingPlanSearch => minimax::build_draft(query),
    }
  }

  /// What a refusal of this service means, when the service is known to mean something more by
  /// one status than its body says - worded as what to do about it. `None` leaves the reply to the
  /// generic envelope.
  fn explain_refusal(self, status: u16) -> Option<Error> {
    match self {
      Self::SearxngSearch => searxng::explain_refusal(status),
      _ => None,
    }
  }

  /// The results in a successful reply. A failure reported inside it is an error.
  fn parse_body(self, body: &Value) -> Result<Vec<SearchResult>, Error> {
    match self {
      Self::CodexAlphaSearch => codex::parse_body(body),
      Self::TavilySearch | Self::TavilyKeylessSearch => tavily::parse_body(body),
      Self::ExaSearch => exa::parse_body(body),
      Self::PerplexitySearch => perplexity::parse_body(body),
      Self::BraveSearch => brave::parse_body(body),
      Self::SerperSearch => serper::parse_body(body),
      Self::JinaSearch => jina::parse_body(body),
      Self::SearxngSearch => searxng::parse_body(body),
      Self::BochaSearch => bocha::parse_body(body),
      Self::MetasoSearch => metaso::parse_body(body),
      Self::KimiCodeSearch => kimi::parse_body(body),
      Self::MinimaxCodingPlanSearch => minimax::parse_body(body),
    }
  }
}

/// The query with search-engine operators for the domains, for a service without parameters of
/// its own for them: `site:` for one allowed domain, `(site:a OR site:b)` for several, `-site:` for
/// each blocked one. A handful at most, so the query stays within what engines accept; the rest
/// are filtered after the reply.
fn with_site_operators(query: &str, allowed: &[String], blocked: &[String]) -> String {
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
fn read_result_array<'a>(
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
fn build_result(item: &Value, url_key: &str, title_key: &str) -> Option<SearchResult> {
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
fn date_only(timestamp: String) -> String {
  timestamp.chars().take(10).collect()
}

/// A time in seconds as its UTC date, `YYYY-MM-DD`.
fn format_date(seconds: u64) -> String {
  let (year, month, day) = civil_date(seconds / 86_400);
  format!("{year:04}-{month:02}-{day:02}")
}
