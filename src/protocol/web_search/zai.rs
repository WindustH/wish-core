use super::{SearchQuery, SearchResult, build_result, read_result_array};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(super) fn build_draft(query: &SearchQuery) -> Result<Draft, Error> {
  // Only search-prime is documented as an engine; unsupported filters are applied locally.
  Draft::post_json(
    &json!({"search_engine":"search-prime", "search_query":query.query,"count":query.limit.clamp(1,50)}),
  )
}
pub(super) fn parse_body(body: &Value) -> Result<Vec<SearchResult>, Error> {
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
