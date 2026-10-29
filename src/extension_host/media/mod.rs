//! File-generating media codecs. Job persistence, credential routing and downloads live above.
use crate::extensions::runninghub::video as runninghub;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub mod transcription;
pub mod video;
use crate::extensions::xiaoyunque::video as xiaoyunque;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaProtocol {
  MinimaxH3,
  XaiVideo,
  XaiSpeech,
  MinimaxSpeech,
  RunninghubVideo,
  XiaoyunqueVideo,
  RunwayVideo,
  LumaVideo,
  SeedanceVideo,
  Seedance25Video,
  OpenaiSpeech,
  OpenaiTranscription,
  XaiTranscription,
  SiliconTranscription,
  ElevenSpeech,
  ElevenMusic,
  ElevenSound,
  ElevenTranscription,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
  Video,
  Speech,
  Music,
  SoundEffect,
  Transcription,
}
impl MediaKind {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Video => "video",
      Self::Speech => "speech",
      Self::Music => "music",
      Self::SoundEffect => "sound_effect",
      Self::Transcription => "transcription",
    }
  }
  pub fn file_kind(self) -> &'static str {
    if self == Self::Video { "video/" } else { "audio/" }
  }
}
pub use crate::extensions::shared::parameters::Parameters;
use crate::extensions::shared::parameters::decode;
impl MediaProtocol {
  pub fn credentials_ready(self, credentials: &Credentials) -> bool {
    match self {
      Self::MinimaxH3 => crate::extensions::minimax::endpoint::ready(credentials),
      Self::XaiVideo => crate::extensions::xai::endpoint::ready(credentials),
      Self::XaiSpeech => crate::extensions::xai::endpoint::ready(credentials),
      Self::MinimaxSpeech => crate::extensions::minimax::endpoint::ready(credentials),
      Self::RunninghubVideo => crate::extensions::runninghub::endpoint::ready(credentials),
      Self::XiaoyunqueVideo => crate::extensions::xiaoyunque::endpoint::ready(credentials),
      Self::RunwayVideo => crate::extensions::runway::endpoint::ready(credentials),
      Self::LumaVideo => crate::extensions::luma::endpoint::ready(credentials),
      Self::SeedanceVideo => crate::extensions::seedance::endpoint::ready(credentials),
      Self::Seedance25Video => crate::extensions::seedance::endpoint::ready(credentials),
      Self::OpenaiSpeech => crate::extensions::openai::endpoint::ready(credentials),
      Self::OpenaiTranscription => crate::extensions::openai::endpoint::ready(credentials),
      Self::XaiTranscription => crate::extensions::xai::endpoint::ready(credentials),
      Self::SiliconTranscription => crate::extensions::siliconflow::endpoint::ready(credentials),
      Self::ElevenSpeech => crate::extensions::elevenlabs::endpoint::ready(credentials),
      Self::ElevenMusic => crate::extensions::elevenlabs::endpoint::ready(credentials),
      Self::ElevenSound => crate::extensions::elevenlabs::endpoint::ready(credentials),
      Self::ElevenTranscription => crate::extensions::elevenlabs::endpoint::ready(credentials),
    }
  }
  pub fn kind(self) -> MediaKind {
    match self {
      Self::XaiVideo
      | Self::XiaoyunqueVideo
      | Self::RunninghubVideo
      | Self::MinimaxH3
      | Self::RunwayVideo
      | Self::LumaVideo
      | Self::SeedanceVideo
      | Self::Seedance25Video => MediaKind::Video,
      Self::ElevenSpeech | Self::OpenaiSpeech | Self::MinimaxSpeech | Self::XaiSpeech => {
        MediaKind::Speech
      }
      Self::ElevenMusic => MediaKind::Music,
      Self::ElevenSound => MediaKind::SoundEffect,
      Self::ElevenTranscription
      | Self::OpenaiTranscription
      | Self::SiliconTranscription
      | Self::XaiTranscription => MediaKind::Transcription,
    }
  }
  pub fn call(
    self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error> {
    match self {
      Self::MinimaxH3 => crate::extensions::minimax::endpoint::call(base, path, credentials, draft),
      Self::XaiVideo => crate::extensions::xai::endpoint::call(base, path, credentials, draft),
      Self::XaiSpeech => crate::extensions::xai::endpoint::call(base, path, credentials, draft),
      Self::MinimaxSpeech => {
        crate::extensions::minimax::endpoint::call(base, path, credentials, draft)
      }
      Self::RunninghubVideo => {
        crate::extensions::runninghub::endpoint::call(base, path, credentials, draft)
      }
      Self::XiaoyunqueVideo => {
        crate::extensions::xiaoyunque::endpoint::call(base, path, credentials, draft)
      }
      Self::RunwayVideo => {
        crate::extensions::runway::endpoint::call(base, path, credentials, draft)
      }
      Self::LumaVideo => crate::extensions::luma::endpoint::call(base, path, credentials, draft),
      Self::SeedanceVideo => {
        crate::extensions::seedance::endpoint::call(base, path, credentials, draft)
      }
      Self::Seedance25Video => {
        crate::extensions::seedance::endpoint::call(base, path, credentials, draft)
      }
      Self::OpenaiSpeech => {
        crate::extensions::openai::endpoint::call(base, path, credentials, draft)
      }
      Self::OpenaiTranscription => {
        crate::extensions::openai::endpoint::call(base, path, credentials, draft)
      }
      Self::XaiTranscription => {
        crate::extensions::xai::endpoint::call(base, path, credentials, draft)
      }
      Self::SiliconTranscription => {
        crate::extensions::siliconflow::endpoint::call(base, path, credentials, draft)
      }
      Self::ElevenSpeech => {
        crate::extensions::elevenlabs::endpoint::call(base, path, credentials, draft)
      }
      Self::ElevenMusic => {
        crate::extensions::elevenlabs::endpoint::call(base, path, credentials, draft)
      }
      Self::ElevenSound => {
        crate::extensions::elevenlabs::endpoint::call(base, path, credentials, draft)
      }
      Self::ElevenTranscription => {
        crate::extensions::elevenlabs::endpoint::call(base, path, credentials, draft)
      }
    }
  }
  pub fn submission(
    self,
    model: &str,
    prompt: &str,
    options: &Parameters,
  ) -> Result<(String, Draft), Error> {
    let invalid = |s: &str| Error::Build(s.into());
    let (path, body) = match self {
      Self::XaiVideo => {
        return crate::extensions::xai::video_wire::submission(
          model,
          prompt,
          video::VideoMode::Text,
          &decode(options)?,
          &[],
        );
      }
      Self::XiaoyunqueVideo => {
        return xiaoyunque::submission(model, prompt, &decode(options)?, &[]);
      }
      Self::RunninghubVideo => {
        return runninghub::submission(
          model,
          prompt,
          video::VideoMode::Text,
          &decode(options)?,
          &[],
        );
      }
      Self::ElevenTranscription
      | Self::OpenaiTranscription
      | Self::SiliconTranscription
      | Self::XaiTranscription => {
        return Err(invalid("use audio_transcribe for ASR"));
      }
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video::submission(
          model,
          prompt,
          &decode(options)?,
          self == Self::Seedance25Video,
        )?
      }
      Self::RunwayVideo => {
        crate::extensions::runway::wire::generation(model, prompt, &decode(options)?)?
      }
      Self::LumaVideo => {
        crate::extensions::luma::video::submission(model, prompt, &decode(options)?)?
      }
      Self::XaiSpeech => {
        crate::extensions::xai::speech_wire::submission(model, prompt, &decode(options)?)?
      }
      Self::MinimaxSpeech => {
        return crate::extensions::minimax::speech::configured_submission(
          model,
          prompt,
          options,
          &Default::default(),
        );
      }
      Self::OpenaiSpeech => {
        crate::extensions::openai::speech::submission(model, prompt, &decode(options)?)?
      }
      Self::MinimaxH3 => {
        crate::extensions::minimax::video::submission(model, prompt, &decode(options)?)?
      }
      Self::ElevenSpeech => {
        crate::extensions::elevenlabs::speech::submission(model, prompt, &decode(options)?)?
      }
      Self::ElevenSound => {
        crate::extensions::elevenlabs::sound::submission(model, prompt, &decode(options)?)?
      }
      Self::ElevenMusic => {
        crate::extensions::elevenlabs::music::submission(model, prompt, &decode(options)?)?
      }
    };
    Ok((path, Draft::post_json(&body)?))
  }
  pub fn task_id(self, value: &Value) -> Result<String, String> {
    match self {
      Self::XaiVideo => crate::extensions::xai::video_status::task_id(value),
      Self::MinimaxH3 => crate::extensions::minimax::video_status::task_id(value),
      Self::RunwayVideo => crate::extensions::runway::video_status::task_id(value),
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video_status::task_id(value)
      }
      Self::LumaVideo => crate::extensions::luma::video_status::task_id(value),
      Self::XiaoyunqueVideo => xiaoyunque::task_id(value),
      Self::RunninghubVideo => runninghub::task_id(value),
      _ => Err("selected extension does not return a video task".into()),
    }
  }
}
pub use crate::extensions::shared::video::VideoStatus;
impl MediaProtocol {
  pub fn status_request(self, task: &str) -> Result<(String, Draft), Error> {
    match self {
      Self::XaiVideo => crate::extensions::xai::video_status::request(task),
      Self::MinimaxH3 => crate::extensions::minimax::video_status::request(task),
      Self::RunwayVideo => crate::extensions::runway::video_status::request(task),
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video_status::request(task)
      }
      Self::LumaVideo => crate::extensions::luma::video_status::request(task),
      Self::XiaoyunqueVideo => xiaoyunque::status_request(task),
      Self::RunninghubVideo => runninghub::status_request(task),
      _ => Err(Error::Build("provider has no video status API".into())),
    }
  }
  pub fn decode_status(self, value: &Value) -> Result<VideoStatus, Error> {
    match self {
      Self::XaiVideo => crate::extensions::xai::video_status::decode(value),
      Self::MinimaxH3 => crate::extensions::minimax::video_status::decode(value),
      Self::RunwayVideo => crate::extensions::runway::video_status::decode(value),
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video_status::decode(value)
      }
      Self::LumaVideo => crate::extensions::luma::video_status::decode(value),
      Self::XiaoyunqueVideo => xiaoyunque::status(value),
      Self::RunninghubVideo => runninghub::status(value),
      _ => Err(Error::Build("provider has no video status API".into())),
    }
  }
}

impl MediaProtocol {
  pub fn parameters(
    self,
    defaults: &Parameters,
    overrides: &Parameters,
    mode: video::VideoMode,
    inputs: &[video::VideoInput],
  ) -> Result<Parameters, Error> {
    match self {
      Self::RunwayVideo => {
        crate::extensions::runway::video_contract::parameters(defaults, overrides, mode)
      }
      Self::LumaVideo => {
        crate::extensions::luma::video_contract::parameters(defaults, overrides, mode)
      }
      Self::XaiVideo => {
        crate::extensions::xai::video_contract::parameters(defaults, overrides, mode, inputs)
      }
      Self::SeedanceVideo | Self::Seedance25Video => {
        crate::extensions::seedance::video_contract::parameters(
          defaults,
          overrides,
          mode,
          self == Self::Seedance25Video,
        )
      }
      Self::RunninghubVideo => {
        crate::extensions::runninghub::video_contract::parameters(defaults, overrides, mode)
      }
      _ => Ok(crate::extensions::shared::parameters::overlay(defaults, overrides)),
    }
  }
}

impl MediaProtocol {
  pub fn validate_configuration(self, model: &str, values: &Parameters) -> Result<(), Error> {
    match self {
      Self::MinimaxSpeech => {
        crate::extensions::minimax::speech::validate_configuration(model, values)
      }
      Self::OpenaiSpeech => {
        crate::extensions::openai::speech::validate_configuration(model, values)
      }
      Self::ElevenSpeech => {
        crate::extensions::elevenlabs::speech::validate_configuration(model, values)
      }
      Self::RunwayVideo => crate::extensions::runway::wire::validate_configuration(model, values),
      Self::OpenaiTranscription => {
        crate::extensions::openai::transcription::validate_configuration(model, values)
      }
      Self::XaiTranscription => {
        crate::extensions::xai::transcription::validate_configuration(model, values)
      }
      Self::ElevenTranscription => {
        crate::extensions::elevenlabs::transcription::validate_configuration(model, values)
      }
      Self::SiliconTranscription => {
        crate::extensions::siliconflow::transcription::validate_configuration(model, values)
      }
      _ => self.submission(model, "validation", values).map(|_| ()),
    }
  }
}
