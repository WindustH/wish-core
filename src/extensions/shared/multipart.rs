//! In-memory bounded multipart encoding. The caller owns file limits and field semantics.
use crate::protocol::endpoint::Draft;
pub fn file(fields: &[(&str, String)], bytes: &[u8], mime: &str, extension: &str) -> Draft {
  let boundary = format!("wish-{}", uuid::Uuid::new_v4());
  let mut body = Vec::new();
  for (name, value) in fields {
    body.extend_from_slice(
      format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
        .as_bytes(),
    );
  }
  body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.{extension}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
  body.extend_from_slice(bytes);
  body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
  Draft::post(
    None,
    vec![("content-type".into(), format!("multipart/form-data; boundary={boundary}"))],
    body,
  )
}
pub fn audio_extension(mime: &str) -> Result<&'static str, crate::protocol::error::Error> {
  match mime {
    "audio/mpeg" => Ok("mp3"),
    "audio/wav" => Ok("wav"),
    "audio/flac" => Ok("flac"),
    "audio/ogg" => Ok("ogg"),
    "video/mp4" => Ok("mp4"),
    "video/webm" => Ok("webm"),
    _ => Err(crate::protocol::error::Error::Build("unsupported audio container".into())),
  }
}
