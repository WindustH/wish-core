//! Video input roles and mode-specific wire contracts. No file access or uploads here.
use super::{MediaProtocol, Parameters};
use crate::extensions::shared::parameters::decode;
use crate::protocol::{endpoint::Draft, error::Error};

pub use crate::extensions::shared::video::*;
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
impl MediaProtocol {
  pub fn video_capabilities(self, model: &str) -> Option<VideoCapabilities> {
    match self {
      Self::XaiVideo => crate::extensions::xai::video_wire::capabilities(model),
      Self::XiaoyunqueVideo => crate::extensions::xiaoyunque::video_contract::capabilities(model),
      Self::RunninghubVideo => crate::extensions::runninghub::video_contract::capabilities(model),
      Self::MinimaxH3 => crate::extensions::minimax::video_contract::capabilities(model),
      Self::RunwayVideo => crate::extensions::runway::video_contract::capabilities(model),
      Self::LumaVideo => crate::extensions::luma::video_contract::capabilities(model),
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video_contract::capabilities(
          model,
          self == Self::Seedance25Video,
        )
      }
      _ => None,
    }
  }
  pub fn validate_video_inputs(
    self,
    model: &str,
    mode: VideoMode,
    inputs: &[VideoInput],
  ) -> Result<(), Error> {
    match self {
      Self::XiaoyunqueVideo => {
        crate::extensions::xiaoyunque::video_contract::validate(model, mode, inputs)
      }
      Self::RunninghubVideo => {
        crate::extensions::runninghub::video_contract::validate(model, mode, inputs)
      }
      Self::MinimaxH3 => crate::extensions::minimax::video_contract::validate(model, mode, inputs),
      Self::RunwayVideo => crate::extensions::runway::video_contract::validate(model, mode, inputs),
      Self::LumaVideo => crate::extensions::luma::video_contract::validate(model, mode, inputs),
      Self::XaiVideo => crate::extensions::xai::video_contract::validate(model, mode, inputs),
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video_contract::validate(
          model,
          mode,
          inputs,
          self == Self::Seedance25Video,
        )
      }
      _ => Err(invalid("selected extension does not support video")),
    }
  }
  pub fn video_submission(
    self,
    model: &str,
    prompt: &str,
    mode: VideoMode,
    options: &Parameters,
    inputs: &[ResolvedInput],
    modify_mode: Option<&str>,
  ) -> Result<(String, Draft), Error> {
    use VideoMode::*;
    if modify_mode.is_some() && !(self == Self::LumaVideo && mode == Edit) {
      return Err(invalid("modify_mode requires Luma editing"));
    }
    if self == Self::XaiVideo {
      return crate::extensions::xai::video_wire::submission(
        model,
        prompt,
        mode,
        &decode(options)?,
        inputs,
      );
    }
    if self == Self::RunninghubVideo {
      return super::runninghub::submission(model, prompt, mode, &decode(options)?, inputs);
    }
    if self == Self::XiaoyunqueVideo {
      return super::xiaoyunque::submission(model, prompt, &decode(options)?, inputs);
    }
    match self {
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video::with_inputs(
          model,
          prompt,
          mode,
          &decode(options)?,
          inputs,
          self == Self::Seedance25Video,
        )
      }
      Self::MinimaxH3 => crate::extensions::minimax::video::with_inputs(
        model,
        prompt,
        mode,
        &decode(options)?,
        inputs,
      ),
      Self::RunwayVideo => {
        crate::extensions::runway::wire::with_inputs(model, prompt, mode, &decode(options)?, inputs)
      }
      Self::LumaVideo => crate::extensions::luma::video::with_inputs(
        model,
        prompt,
        mode,
        &decode(options)?,
        inputs,
        modify_mode,
      ),
      _ => Err(invalid("selected extension does not support video")),
    }
  }
}
