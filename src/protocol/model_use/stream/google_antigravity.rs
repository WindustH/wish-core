//! Unwrap each Cloud Code SSE record before passing it to the Gemini stream state.
use super::{WireDecoder, google_generate_content};
use crate::protocol::{StreamEvent, error::Error, model_use::response::google_antigravity};

#[derive(Clone, Default)]
pub struct Decoder {
  inner: google_generate_content::Decoder,
}
impl WireDecoder for Decoder {
  fn feed(&mut self, event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    let body = serde_json::from_str(data)
      .map_err(|e| Error::Malformed(format!("Antigravity chunk is not JSON: {e}")))?;
    self.inner.feed(event, &google_antigravity::unwrap(&body)?.to_string())
  }
  fn finish(&mut self) -> Result<Vec<StreamEvent>, Error> {
    self.inner.finish()
  }
}
