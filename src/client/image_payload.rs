//! Keep image strings out of the intermediate provider JSON tree. Only explicit,
//! unpredictable per-call bindings are expanded, directly into the final byte buffer.
use crate::protocol::{ContentBlock, Message, Request, error::Error};
use serde_json::Value;
use std::{borrow::Cow, io::Write};

struct ImageBinding<'a> {
  token: String,
  data_base64: &'a str,
}
pub(super) struct Images<'a>(Vec<ImageBinding<'a>>);
impl<'a> Images<'a> {
  pub fn bind(request: &'a Request) -> Result<(Cow<'a, Request>, Self), Error> {
    let (conversation, images) = Self::bind_conversation(&request.conversation)?;
    if images.0.is_empty() {
      return Ok((Cow::Borrowed(request), images));
    }
    let projected = Request {
      stream: request.stream,
      model: request.model.clone(),
      conversation: conversation.into_owned(),
      tools: request.tools.clone(),
      tool_choice: request.tool_choice,
      max_output_tokens: request.max_output_tokens,
      reasoning: request.reasoning.clone(),
      cache: request.cache.clone(),
    };
    Ok((Cow::Owned(projected), images))
  }
  pub fn bind_conversation(messages: &'a [Message]) -> Result<(Cow<'a, [Message]>, Self), Error> {
    let has_images = messages.iter().any(|message| match message {
      Message::User { content, .. }
      | Message::Assistant { content, .. }
      | Message::System { content, .. }
      | Message::Developer { content, .. } => {
        content.iter().any(|block| matches!(block, ContentBlock::Image { .. }))
      }
      _ => false,
    });
    if !has_images {
      return Ok((Cow::Borrowed(messages), Self(Vec::new())));
    }
    let mut images = Vec::new();
    let mut total = 0usize;
    let mut content = |blocks: &'a [ContentBlock]| -> Result<Vec<ContentBlock>, Error> {
      blocks
        .iter()
        .map(|block| {
          if let ContentBlock::Image { mime_type, data_base64 } = block {
            total = total.saturating_add(data_base64.len());
            if total > 24 * 1024 * 1024 {
              return Err(Error::Build(
                "model image payload exceeds 24 MiB of Base64; use fewer images or crop them"
                  .into(),
              ));
            }
            let token = format!("wish-image-{}", uuid::Uuid::new_v4());
            images.push(ImageBinding { token: token.clone(), data_base64 });
            Ok(ContentBlock::Image { mime_type: mime_type.clone(), data_base64: token })
          } else {
            Ok(block.clone())
          }
        })
        .collect()
    };
    let projected = messages
      .iter()
      .map(|message| {
        Ok(match message {
          Message::User { content: blocks, metadata } => {
            Message::User { content: content(blocks)?, metadata: metadata.clone() }
          }
          Message::Assistant { content: blocks, metadata } => {
            Message::Assistant { content: content(blocks)?, metadata: metadata.clone() }
          }
          Message::System { content: blocks, metadata } => {
            Message::System { content: content(blocks)?, metadata: metadata.clone() }
          }
          Message::Developer { content: blocks, metadata, fixed } => Message::Developer {
            content: content(blocks)?,
            metadata: metadata.clone(),
            fixed: *fixed,
          },
          other => other.clone(),
        })
      })
      .collect::<Result<Vec<_>, Error>>()?;
    Ok((Cow::Owned(projected), Self(images)))
  }
  pub fn serialize(&self, value: &Value) -> Result<Vec<u8>, Error> {
    if self.0.is_empty() {
      return serde_json::to_vec(value).map_err(|e| Error::Build(e.to_string()));
    }
    let mut output = Vec::new();
    self.write(value, &mut output).map_err(|e| Error::Build(e.to_string()))?;
    Ok(output)
  }
  fn write(&self, value: &Value, out: &mut Vec<u8>) -> std::io::Result<()> {
    match value {
      Value::String(text) => {
        for ImageBinding { token, data_base64: data } in &self.0 {
          let prefix = if text == token {
            Some("")
          } else {
            text
              .strip_suffix(token.as_str())
              .filter(|prefix| prefix.starts_with("data:") && prefix.ends_with(";base64,"))
          };
          if let Some(prefix) = prefix {
            // Use JSON's string escaping for the MIME prefix and the encoded data too;
            // legacy input may contain invalid Base64 and must never inject raw JSON.
            out.push(b'"');
            for segment in [prefix, *data] {
              if segment.bytes().all(|byte| byte >= 0x20 && byte != b'"' && byte != b'\\') {
                out.write_all(segment.as_bytes())?;
              } else {
                let encoded = serde_json::to_string(segment)?;
                out.write_all(&encoded.as_bytes()[1..encoded.len() - 1])?;
              }
            }
            out.push(b'"');
            return Ok(());
          }
        }
        serde_json::to_writer(out, value)?;
      }
      Value::Array(values) => {
        out.push(b'[');
        for (index, value) in values.iter().enumerate() {
          if index > 0 {
            out.push(b',');
          }
          self.write(value, out)?;
        }
        out.push(b']');
      }
      Value::Object(values) => {
        out.push(b'{');
        for (index, (key, value)) in values.iter().enumerate() {
          if index > 0 {
            out.push(b',');
          }
          serde_json::to_writer(&mut *out, key)?;
          out.push(b':');
          self.write(value, out)?;
        }
        out.push(b'}');
      }
      _ => serde_json::to_writer(out, value)?,
    }
    Ok(())
  }
}
