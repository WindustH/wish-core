use crate::extensions::shared::web_search::{SearchResult, build_result, read_result_array};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(crate) fn build_draft(query: &Request) -> Result<Draft, Error> {
  // Only search-prime is documented as an engine; unsupported filters are applied locally.
  Draft::post_json(
    &json!({"search_engine":"search-prime", "search_query":query.query,"count":query.limit.clamp(1,50)}),
  )
}
pub(crate) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
  Ok(
    read_result_array(body, "search_result", "Z.AI search")?
      .iter()
      .filter_map(|r| {
        Some(SearchResult {
          snippet: r["content"].as_str().map(str::to_owned),
          site: r["media"].as_str().map(str::to_owned),
          published: r["publish_date"].as_str().map(str::to_owned),
          ..build_result(r, "link", "title")?
        })
      })
      .collect(),
  )
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub query: String,
  pub limit: usize,
}

use crate::extensions::shared::web_search::SearchQuery;
impl From<&SearchQuery> for Request {
  fn from(value: &SearchQuery) -> Self {
    Self { query: value.query.clone(), limit: value.limit }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::SearchCodec =
  crate::extensions::shared::read_codecs::SearchCodec {
    id: "zai_search",
    source: super::sources::search,
    recency: false,
    build: |query, _| build_draft(&query.into()),
    parse: parse_body,
    refusal: |_| None,
  };
