pub(crate) use crate::protocol::endpoint::multipart_file as file;
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
