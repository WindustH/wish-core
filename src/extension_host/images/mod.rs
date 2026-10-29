//! Image generation contracts, provider adapters and bounded reference/mask preparation.
pub(crate) use crate::extensions::bfl::image as bfl;
use crate::extensions::shared::urls::validate_url;
use crate::protocol::{
  attempt::{Call, Method},
  endpoint::{Credentials, Draft},
  error::Error,
};
pub(crate) mod execution;
pub(crate) use crate::extensions::google::image as gemini;
mod inputs;
pub(crate) use crate::extensions::google::interactions;
pub(crate) use crate::extensions::jimeng::image as jimeng;
pub(crate) use crate::extensions::openai::image as openai;
mod references;
pub(crate) use crate::extensions::alibaba::image as wan;
pub(crate) use crate::extensions::runninghub::image as runninghub;
pub(crate) use crate::extensions::xai::image as xai;
pub use inputs::{EditOverlay, ReferenceImage, ReferenceInput};
pub use references::{PreciseReference, ReferenceKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageProtocol {
  Openai,
  Xai,
  Codex,
  Gemini,
  GeminiInteractions,
  Together,
  Siliconflow,
  Seedream,
  Bfl,
  Fal,
  Replicate,
  Wan,
  Stability,
  Novelai,
  Jimeng,
  Runninghub,
}

pub use crate::extensions::shared::images::{ImageReply, ImageSource};

impl ImageProtocol {
  pub fn max_output_images(self) -> u64 {
    match self {
      Self::Wan => 12,
      Self::Xai => 10,
      _ => 8,
    }
  }

  pub fn credentials_ready(self, credentials: &Credentials) -> bool {
    match self {
      Self::Openai => crate::extensions::openai::endpoint::ready(credentials),
      Self::Codex => crate::extensions::openai::endpoint::ready(credentials),
      Self::Xai => crate::extensions::xai::endpoint::ready(credentials),
      Self::Gemini => crate::extensions::google::endpoint::ready(credentials),
      Self::GeminiInteractions => crate::extensions::google::endpoint::ready(credentials),
      Self::Together => crate::extensions::together::endpoint::ready(credentials),
      Self::Siliconflow => crate::extensions::siliconflow::endpoint::ready(credentials),
      Self::Seedream => crate::extensions::seedance::endpoint::ready(credentials),
      Self::Bfl => crate::extensions::bfl::endpoint::ready(credentials),
      Self::Fal => crate::extensions::fal::endpoint::ready(credentials),
      Self::Replicate => crate::extensions::replicate::endpoint::ready(credentials),
      Self::Wan => crate::extensions::alibaba::endpoint::ready(credentials),
      Self::Stability => crate::extensions::stability::endpoint::ready(credentials),
      Self::Novelai => crate::extensions::novelai::endpoint::ready(credentials),
      Self::Jimeng => crate::extensions::jimeng::endpoint::ready(credentials),
      Self::Runninghub => crate::extensions::runninghub::endpoint::ready(credentials),
    }
  }
  pub fn task_status(
    self,
    model: &str,
    task: &str,
    parameters: &serde_json::Map<String, Value>,
  ) -> Result<Draft, Error> {
    if self == Self::Runninghub {
      return runninghub::status(task);
    }
    if self != Self::Jimeng {
      return Err(Error::Build("image provider has no task status endpoint".into()));
    }
    jimeng::status_request(model, task, parameters)
  }
  pub fn call(
    self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error> {
    match self {
      Self::Openai => crate::extensions::openai::endpoint::call(base, path, credentials, draft),
      Self::Codex => {
        crate::extensions::openai::endpoint::subscription_image(base, path, credentials, draft)
      }
      Self::Xai => crate::extensions::xai::endpoint::call(base, path, credentials, draft),
      Self::Gemini => crate::extensions::google::endpoint::call(base, path, credentials, draft),
      Self::GeminiInteractions => {
        crate::extensions::google::endpoint::call(base, path, credentials, draft)
      }
      Self::Together => crate::extensions::together::endpoint::call(base, path, credentials, draft),
      Self::Siliconflow => {
        crate::extensions::siliconflow::endpoint::call(base, path, credentials, draft)
      }
      Self::Seedream => crate::extensions::seedance::endpoint::call(base, path, credentials, draft),
      Self::Bfl => crate::extensions::bfl::endpoint::call(base, path, credentials, draft),
      Self::Fal => crate::extensions::fal::endpoint::call(base, path, credentials, draft),
      Self::Replicate => {
        crate::extensions::replicate::endpoint::call(base, path, credentials, draft)
      }
      Self::Wan => crate::extensions::alibaba::endpoint::call(base, path, credentials, draft),
      Self::Stability => {
        crate::extensions::stability::endpoint::call(base, path, credentials, draft)
      }
      Self::Novelai => crate::extensions::novelai::endpoint::call(base, path, credentials, draft),
      Self::Jimeng => crate::extensions::jimeng::endpoint::call(base, path, credentials, draft),
      Self::Runninghub => {
        crate::extensions::runninghub::endpoint::call(base, path, credentials, draft)
      }
    }
  }
  pub fn submission(
    self,
    model: &str,
    prompt: &str,
    parameters: &serde_json::Map<String, Value>,
  ) -> Result<Draft, Error> {
    match self {
      Self::Openai => crate::extensions::openai::image::generation(model, prompt, parameters),
      Self::Codex => {
        crate::extensions::openai::subscription_image::generation(model, prompt, parameters)
      }
      Self::Together => crate::extensions::together::image::generation(model, prompt, parameters),
      Self::Seedream => crate::extensions::seedance::image::generation(model, prompt, parameters),
      Self::Siliconflow => {
        crate::extensions::siliconflow::image::generation(model, prompt, parameters)
      }
      Self::Fal => crate::extensions::fal::image::generation(model, prompt, parameters),
      Self::Replicate => crate::extensions::replicate::image::generation(model, prompt, parameters),
      Self::Bfl => crate::extensions::bfl::image::generation(model, prompt, parameters),
      Self::Gemini => crate::extensions::google::image::submission(model, prompt, parameters, &[]),
      Self::GeminiInteractions => {
        crate::extensions::google::interactions::with_images(model, prompt, parameters, &[])
      }
      Self::Stability => crate::extensions::stability::image::submission(prompt, parameters),
      Self::Novelai => crate::extensions::novelai::wire::submission(model, prompt, parameters),
      Self::Xai => crate::extensions::xai::image::submission(model, prompt, parameters, &[]),
      Self::Jimeng => crate::extensions::jimeng::image::submission(model, prompt, parameters, &[]),
      Self::Runninghub => {
        crate::extensions::runninghub::image::submission(model, prompt, parameters, &[])
      }
      Self::Wan => crate::extensions::alibaba::image::submission(model, prompt, parameters, &[]),
    }
  }
  pub fn decode(
    self,
    body: &Value,
    current_url: &str,
    result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    match self {
      Self::Openai => crate::extensions::openai::image::decode(body),
      Self::Codex => crate::extensions::openai::subscription_image::decode(body),
      Self::Together => crate::extensions::together::image::decode(body),
      Self::Seedream => crate::extensions::seedance::image::decode(body),
      Self::Siliconflow => crate::extensions::siliconflow::image::decode(body),
      Self::Fal => crate::extensions::fal::image::decode(body, current_url, result_url),
      Self::Replicate => crate::extensions::replicate::image::decode(body, current_url, result_url),
      Self::Bfl => crate::extensions::bfl::image::decode(body, current_url, result_url),
      Self::Gemini => crate::extensions::google::image::decode(body),
      Self::GeminiInteractions => crate::extensions::google::interactions::decode(body),
      Self::Stability => crate::extensions::stability::image::decode(body),
      Self::Novelai => crate::extensions::novelai::wire::decode(body),
      Self::Xai => crate::extensions::xai::image::decode(body),
      Self::Jimeng => crate::extensions::jimeng::image::decode(body),
      Self::Runninghub => crate::extensions::runninghub::image::decode(body),
      Self::Wan => crate::extensions::alibaba::image::decode(body),
    }
  }
}
/// Polling keeps credentials on the configured origin (BFL also returns regional origins).
pub fn poll_call(protocol: ImageProtocol, initial: &Call, url: &str) -> Result<Call, Error> {
  let origin = validate_url(&initial.url)?;
  let target = validate_url(url)?;
  let bfl = protocol == ImageProtocol::Bfl
    && crate::extensions::bfl::endpoint::permits_poll_origin(&origin, &target);
  if origin.origin() != target.origin() && !bfl {
    return Err(Error::Build("image polling URL has an untrusted origin".into()));
  }
  Ok(Call {
    method: Method::Get,
    url: target.into(),
    headers: initial
      .headers
      .iter()
      .filter(|(key, _)| !key.eq_ignore_ascii_case("content-type"))
      .cloned()
      .collect(),
    body: Vec::new(),
  })
}
