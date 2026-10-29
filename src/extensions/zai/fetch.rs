//! Z.AI reader: Markdown output, optional cache bypass, no inline image data.
use crate::extensions::shared::web_fetch::{FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(crate) fn build_draft(request: &Request) -> Result<Draft, Error> {
  Draft::post_json(
    &json!({"url":request.url,"return_format":"markdown","no_cache":request.refresh,"keep_img_data_url":false}),
  )
}
pub(crate) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(&body["reader_result"], "content")
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub url: String,
  pub refresh: bool,
}

use crate::extensions::shared::web_fetch::FetchRequest;
impl From<&FetchRequest> for Request {
  fn from(value: &FetchRequest) -> Self {
    Self { url: value.url.clone(), refresh: value.refresh }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::FetchCodec =
  crate::extensions::shared::read_codecs::FetchCodec {
    id: "zai_reader",
    source: super::sources::fetch,
    build: |request| build_draft(&request.into()),
    parse: parse_body,
  };
