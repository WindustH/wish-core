//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub struct XaiSpeech;
pub static DRIVER: XaiSpeech = XaiSpeech;
impl MediaDriver for XaiSpeech {
  fn buffered_audio(&self, p: &Parameters) -> bool {
    super::speech_wire::timed_response(p)
  }
  fn decode_audio(&self, bytes: &[u8]) -> Result<AudioCompletion, String> {
    let (audio, metadata) = super::speech_output::decode_timed(bytes)?;
    let mime = crate::storage::media::mime(&audio).ok_or("unrecognized speech audio")?;
    let value: serde_json::Value = serde_json::from_slice(&metadata).map_err(|e| e.to_string())?;
    if !["audio/mpeg", "audio/wav"].contains(&mime) || value["content_type"] != mime {
      return Err("speech audio content type does not match its bytes".into());
    }
    Ok(AudioCompletion::Inline { audio, metadata })
  }

  fn id(&self) -> &'static str {
    "xai_speech"
  }
  fn kind(&self) -> MediaKind {
    MediaKind::Speech
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    super::endpoint::call(base, path, c, d)
  }
  fn submission(
    &self,
    model: &str,
    prompt: &str,
    p: &Parameters,
  ) -> Result<(String, Draft), Error> {
    let (path, body) = super::speech_wire::submission(model, prompt, &decode(p)?)?;
    Ok((path, Draft::post_json(&body)?))
  }
}
