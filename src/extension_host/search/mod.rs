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

mod client;
use serde_json::Value;

use crate::protocol::endpoint::Draft;
use crate::protocol::error::Error;

use crate::extensions::bocha::search as bocha;
use crate::extensions::brave::search as brave;
use crate::extensions::deepseek::search as deepseek;
use crate::extensions::exa::search as exa;
use crate::extensions::openai::search as codex;
pub mod fetch;
use crate::extensions::jina::search as jina;
use crate::extensions::kimi::search as kimi;
use crate::extensions::metaso::search as metaso;
use crate::extensions::minimax::search as minimax;
use crate::extensions::perplexity::search as perplexity;
use crate::extensions::searxng::search as searxng;
use crate::extensions::serper::search as serper;
mod source;
use crate::extensions::tavily::search as tavily;
use crate::extensions::zai::search as zai;

pub use fetch::search;

pub use crate::extensions::shared::web_search::{Recency, SearchResult, SearchResults};
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

crate::protocol::text_id_enum! {
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
    /// DeepSeek's search, with a DeepSeek API key.
    DeepseekSearch => "deepseek_search",
    ZaiSearch => "zai_search",
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
      | Self::BochaSearch
      | Self::DeepseekSearch => true,
      Self::ZaiSearch
      | Self::JinaSearch
      | Self::MetasoSearch
      | Self::KimiCodeSearch
      | Self::MinimaxCodingPlanSearch => false,
    }
  }

  /// The request for one search, besides where it goes and how it is proven. `now` is the time
  /// in seconds, for a service that takes a period as a start date.
  fn build_draft(self, query: &SearchQuery, now: u64) -> Result<Draft, Error> {
    match self {
      Self::CodexAlphaSearch => codex::build_draft(&query.into()),
      Self::TavilySearch | Self::TavilyKeylessSearch => tavily::build_draft(&query.into()),
      Self::ExaSearch => exa::build_draft(&query.into(), now),
      Self::PerplexitySearch => perplexity::build_draft(&query.into()),
      Self::BraveSearch => Ok(brave::build_draft(&query.into())),
      Self::SerperSearch => serper::build_draft(&query.into()),
      Self::JinaSearch => jina::build_draft(&query.into()),
      Self::SearxngSearch => Ok(searxng::build_draft(&query.into())),
      Self::BochaSearch => bocha::build_draft(&query.into()),
      Self::MetasoSearch => metaso::build_draft(&query.into()),
      Self::KimiCodeSearch => kimi::build_draft(&query.into()),
      Self::MinimaxCodingPlanSearch => minimax::build_draft(&query.into()),
      Self::DeepseekSearch => deepseek::build_draft(&query.into(), now),
      Self::ZaiSearch => zai::build_draft(&query.into()),
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
      Self::DeepseekSearch => deepseek::parse_body(body),
      Self::ZaiSearch => zai::parse_body(body),
    }
  }
}

impl From<&SearchQuery> for crate::extensions::exa::search::Request {
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
impl From<&SearchQuery> for crate::extensions::jina::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self {
      query: value.query.clone(),
      allowed_domains: value.allowed_domains.clone(),
      blocked_domains: value.blocked_domains.clone(),
      limit: value.limit,
    }
  }
}
impl From<&SearchQuery> for crate::extensions::zai::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone(), limit: value.limit }
  }
}
impl From<&SearchQuery> for crate::extensions::serper::search::Request {
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
impl From<&SearchQuery> for crate::extensions::metaso::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone(), limit: value.limit }
  }
}
impl From<&SearchQuery> for crate::extensions::openai::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self {
      query: value.query.clone(),
      allowed_domains: value.allowed_domains.clone(),
      blocked_domains: value.blocked_domains.clone(),
      recency: value.recency,
      conversation: value.conversation.clone(),
      model: value.model.clone(),
    }
  }
}
impl From<&SearchQuery> for crate::extensions::minimax::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone() }
  }
}
impl From<&SearchQuery> for crate::extensions::perplexity::search::Request {
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
impl From<&SearchQuery> for crate::extensions::kimi::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone(), limit: value.limit }
  }
}
impl From<&SearchQuery> for crate::extensions::bocha::search::Request {
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
impl From<&SearchQuery> for crate::extensions::deepseek::search::Request {
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
impl From<&SearchQuery> for crate::extensions::brave::search::Request {
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
impl From<&SearchQuery> for crate::extensions::tavily::search::Request {
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
impl From<&SearchQuery> for crate::extensions::searxng::search::Request {
  fn from(value: &SearchQuery) -> Self {
    Self {
      query: value.query.clone(),
      allowed_domains: value.allowed_domains.clone(),
      blocked_domains: value.blocked_domains.clone(),
      recency: value.recency,
    }
  }
}
