//! Gemini's grounded search. Only upstream grounding records become search results;
//! an ungrounded generated answer is not evidence that a search took place.
use crate::extensions::shared::{read_codecs::SearchCodec, web_search::SearchResult};
use crate::protocol::error::Error;
use serde_json::{Value, json};

fn parse(body: &Value) -> Result<Vec<SearchResult>, Error> {
  let response = super::antigravity::response(body)?;
  let candidates = response["candidates"]
    .as_array()
    .ok_or_else(|| Error::Malformed("Antigravity search has no candidates".into()))?;
  let mut results = Vec::new();
  let mut grounded = false;
  for candidate in candidates {
    let Some(grounding) = candidate.get("groundingMetadata") else { continue };
    grounded = true;
    for (index, chunk) in grounding["groundingChunks"].as_array().into_iter().flatten().enumerate()
    {
      let Some(url) = chunk["web"]["uri"].as_str() else { continue };
      let snippets: Vec<_> = grounding["groundingSupports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|support| {
          support["groundingChunkIndices"]
            .as_array()
            .is_some_and(|indices| indices.iter().any(|n| n.as_u64() == Some(index as u64)))
        })
        .filter_map(|support| support["segment"]["text"].as_str())
        .collect();
      results.push(SearchResult {
        title: chunk["web"]["title"].as_str().unwrap_or(url).to_owned(),
        url: url.to_owned(),
        snippet: (!snippets.is_empty()).then(|| snippets.join("\n")),
        site: None,
        published: None,
      });
    }
  }
  if !grounded {
    return Err(Error::Malformed("Antigravity returned no search grounding".into()));
  }
  Ok(results)
}

pub static CODEC: SearchCodec = SearchCodec {
  id: "google_antigravity_search",
  source: super::antigravity::source,
  recency: false,
  build: |query, _, credentials| {
    super::antigravity::draft(
      "gemini-3.5-flash-lite",
      json!({
        "contents": [{"role": "user", "parts": [{"text": format!("Search the web for: {}. Cite the sources.", query.query)}]}],
        "tools": [{"googleSearch": {}}],
        "generationConfig": {"maxOutputTokens": 4096},
      }),
      "web_search",
      credentials,
    )
  },
  parse,
  refusal: |_| None,
};
