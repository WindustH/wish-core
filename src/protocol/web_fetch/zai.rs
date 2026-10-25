//! Z.AI reader: Markdown output, optional cache bypass, no inline image data.
use super::{FetchRequest, FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(super) fn build_draft(request: &FetchRequest) -> Result<Draft, Error> {
  Draft::post_json(
    &json!({"url":request.url,"return_format":"markdown","no_cache":request.refresh,"keep_img_data_url":false}),
  )
}
pub(super) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(&body["reader_result"], "content")
}
