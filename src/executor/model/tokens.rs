//! Semantic fallback for requests which have no provider-side count endpoint.
use crate::protocol::{ContentBlock, Message, Request};

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct TokenEstimator {
  pub bytes_per_token: f64,
  pub image_tokens: u64,
}
impl Default for TokenEstimator {
  fn default() -> Self {
    Self { bytes_per_token: 4.0, image_tokens: 2048 }
  }
}
impl TokenEstimator {
  pub fn estimate_request(&self, request: &Request) -> u64 {
    let mut total = self.estimate_messages(&request.conversation);
    for tool in &request.tools {
      total = total.saturating_add(12).saturating_add(
        self.estimate_text(&serde_json::to_string(tool).expect("tool serializes to JSON")),
      );
    }
    total
  }
  pub fn estimate_messages(&self, messages: &[Message]) -> u64 {
    messages
      .iter()
      .fold(0u64, |total, message| total.saturating_add(self.estimate_message(message)))
  }
  pub fn estimate_message(&self, message: &Message) -> u64 {
    let content = match message {
      Message::System { content, .. }
      | Message::Developer { content, .. }
      | Message::User { content, .. }
      | Message::Assistant { content, .. } => content.iter().fold(0u64, |total, block| {
        total.saturating_add(match block {
          ContentBlock::Text { text } => 4u64.saturating_add(self.estimate_text(text)),
          ContentBlock::Image { .. } => self.image_tokens,
        })
      }),
      // Display summaries and application metadata are never replayed. Opaque material has no
      // known tokenizer; its byte ratio is explicitly only a fallback estimate.
      Message::Reasoning { plaintext, ciphertext, signature, .. } => self
        .estimate_text(plaintext)
        .saturating_add(self.estimate_text(ciphertext))
        .saturating_add(self.estimate_text(signature)),
      Message::ToolUse { call_id, name, arguments, .. } => self
        .estimate_text(call_id)
        .saturating_add(self.estimate_text(name))
        .saturating_add(self.estimate_text(&arguments.to_string())),
      Message::ToolResult { call_id, name, content, .. } => self
        .estimate_text(call_id)
        .saturating_add(self.estimate_text(name))
        .saturating_add(self.estimate_text(&content.to_string())),
      Message::UpstreamCompaction { encrypted_content, .. } => {
        self.estimate_text(encrypted_content)
      }
    };
    16u64.saturating_add(content)
  }
  fn estimate_text(&self, text: &str) -> u64 {
    (text.len() as f64 / self.bytes_per_token).ceil() as u64
  }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum TokenMeasurementSource {
  ProviderCount,
  Estimate,
  CalibratedEstimate,
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct TokenMeasurement {
  pub tokens: u64,
  pub source: TokenMeasurementSource,
}
