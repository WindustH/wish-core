//! MCP binary result materialization; callers execute this on the bounded IO pool.
use serde_json::{Map, Value, json};
use std::path::Path;
/// Saves a binary content block of a result - an image, audio, a resource's bytes - as a file, and
/// puts its path where the data was, so neither the bridge nor the output carries base64.
pub fn store_binary_content(result: &mut Value, directory: &Path) -> std::io::Result<()> {
  let mut encoded_total = 0usize;
  let mut save = |item: &mut Map<String, Value>, field: &str| -> std::io::Result<()> {
    let Some(data) = item.get(field).and_then(Value::as_str) else { return Ok(()) };
    use base64::Engine;
    const MAX_ENCODED_BYTES: usize = 128 * 1024 * 1024;
    encoded_total = encoded_total.saturating_add(data.len());
    if encoded_total > MAX_ENCODED_BYTES {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "MCP binary result exceeds 128 MiB encoded limit",
      ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
      .decode(data)
      .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("");
    let extension = match mime {
      "image/png" => ".png",
      "image/jpeg" => ".jpg",
      "image/gif" => ".gif",
      "image/webp" => ".webp",
      "image/svg+xml" => ".svg",
      "audio/wav" | "audio/x-wav" => ".wav",
      "audio/mpeg" => ".mp3",
      "audio/ogg" => ".ogg",
      "application/pdf" => ".pdf",
      "application/json" => ".json",
      _ => "",
    };
    use sha2::Digest;
    let path = directory.join(format!("{:x}{extension}", sha2::Sha256::digest(&bytes)));
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    if let Err(error) =
      std::fs::write(&temporary, &bytes).and_then(|_| std::fs::rename(&temporary, &path))
    {
      let _ = std::fs::remove_file(temporary);
      return Err(error);
    }
    item.remove(field);
    item.insert("path".into(), json!(path));
    Ok(())
  };
  let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) else {
    return Ok(());
  };
  for block in content {
    let Some(item) = block.as_object_mut() else { continue };
    match item.get("type").and_then(Value::as_str) {
      Some("image" | "audio") => save(item, "data")?,
      Some("resource") => {
        if let Some(resource) = item.get_mut("resource").and_then(Value::as_object_mut) {
          save(resource, "blob")?;
        }
      }
      _ => {}
    }
  }
  Ok(())
}
