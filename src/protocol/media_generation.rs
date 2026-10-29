//! File-generating media codecs. Job persistence, credential routing and downloads live above.
use super::{
  attempt::Call,
  endpoint::{AuthScheme, Credentials, Draft, Endpoint},
  error::Error,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaProtocol {
  MinimaxH3,
  RunwayVideo,
  LumaVideo,
  OpenaiSpeech,
  OpenaiTranscription,
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
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaOptions {
  pub duration: Option<f64>,
  pub voice: Option<String>,
  pub aspect_ratio: Option<String>,
  pub resolution: Option<String>,
  pub instrumental: Option<bool>,
}
impl MediaOptions {
  pub fn overlay(&self, defaults: &Self) -> Self {
    Self {
      duration: self.duration.or(defaults.duration),
      voice: self.voice.clone().or_else(|| defaults.voice.clone()),
      aspect_ratio: self.aspect_ratio.clone().or_else(|| defaults.aspect_ratio.clone()),
      resolution: self.resolution.clone().or_else(|| defaults.resolution.clone()),
      instrumental: self.instrumental.or(defaults.instrumental),
    }
  }
}
impl MediaProtocol {
  pub fn kind(self) -> MediaKind {
    match self {
      Self::MinimaxH3 | Self::RunwayVideo | Self::LumaVideo => MediaKind::Video,
      Self::ElevenSpeech | Self::OpenaiSpeech => MediaKind::Speech,
      Self::ElevenMusic => MediaKind::Music,
      Self::ElevenSound => MediaKind::SoundEffect,
      Self::ElevenTranscription | Self::OpenaiTranscription | Self::SiliconTranscription => {
        MediaKind::Transcription
      }
    }
  }
  pub fn call(
    self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    mut draft: Draft,
  ) -> Result<Call, Error> {
    let auth = match self {
      Self::ElevenSpeech | Self::ElevenMusic | Self::ElevenSound | Self::ElevenTranscription => {
        AuthScheme::Header("xi-api-key")
      }
      _ => AuthScheme::Bearer(None),
    };
    if self == Self::RunwayVideo {
      draft.headers.push(("X-Runway-Version".into(), "2024-11-06".into()));
    }
    Endpoint::new(base, path, auth)?.build_call(
      draft,
      credentials,
      crate::utils::time::unix_seconds(),
    )
  }
  pub fn submission(
    self,
    model: &str,
    prompt: &str,
    options: &MediaOptions,
  ) -> Result<(String, Draft), Error> {
    let invalid = |s: &str| Error::Build(s.into());
    if prompt.trim().is_empty() || prompt.chars().count() > 4100 {
      return Err(invalid("prompt must contain 1..4100 characters"));
    }
    let kind = self.kind();
    if kind != MediaKind::Speech && options.voice.is_some() {
      return Err(invalid("voice is only supported for speech"));
    }
    if kind != MediaKind::Video && (options.resolution.is_some() || options.aspect_ratio.is_some())
    {
      return Err(invalid("resolution and aspect_ratio are only supported for video"));
    }
    if kind != MediaKind::Music && options.instrumental.is_some() {
      return Err(invalid("instrumental is only supported for music"));
    }
    let (path, body) = match self {
      Self::ElevenTranscription | Self::OpenaiTranscription | Self::SiliconTranscription => {
        return Err(invalid("use audio_transcribe for ASR"));
      }
      Self::RunwayVideo => {
        let duration = options.duration.unwrap_or(5.0);
        if model != "gen4.5" || !(2.0..=10.0).contains(&duration) || duration.fract() != 0.0 {
          return Err(invalid("Runway gen4.5 requires an integer duration of 2..10 seconds"));
        }
        if prompt.encode_utf16().count() > 1000 {
          return Err(invalid("Runway prompt exceeds 1000 UTF-16 characters"));
        }
        if options.resolution.as_deref().is_some_and(|v| !v.eq_ignore_ascii_case("720p")) {
          return Err(invalid("Runway gen4.5 supports 720p output"));
        }
        let ratio = match options.aspect_ratio.as_deref().unwrap_or("16:9") {
          "16:9" => "1280:720",
          "9:16" => "720:1280",
          _ => return Err(invalid("Runway gen4.5 supports 16:9 or 9:16")),
        };
        (
          "/v1/text_to_video".into(),
          json!({"model":model,"promptText":prompt,"duration":duration as u64,"ratio":ratio}),
        )
      }
      Self::LumaVideo => {
        if !["ray-2", "ray-flash-2"].contains(&model) {
          return Err(invalid("unsupported Luma Dream Machine model"));
        }
        let duration = options.duration.unwrap_or(5.0);
        if ![5.0, 9.0].contains(&duration) {
          return Err(invalid("Luma duration must be 5 or 9 seconds"));
        }
        let resolution = options.resolution.as_deref().unwrap_or("720p").to_ascii_lowercase();
        if !["540p", "720p", "1080p", "4k"].contains(&resolution.as_str()) {
          return Err(invalid("unsupported Luma resolution"));
        }
        let ratio = options.aspect_ratio.as_deref().unwrap_or("16:9");
        if !["21:9", "16:9", "4:3", "1:1", "3:4", "9:16", "9:21"].contains(&ratio) {
          return Err(invalid("unsupported Luma aspect ratio"));
        }
        (
          "/dream-machine/v1/generations/video".into(),
          json!({"model":model,"prompt":prompt,"duration":format!("{}s",duration as u64),"resolution":resolution,"aspect_ratio":ratio}),
        )
      }
      Self::OpenaiSpeech => {
        if options.duration.is_some() {
          return Err(invalid("speech duration is determined by the text"));
        }
        if prompt.chars().count() > 4096 {
          return Err(invalid("speech input exceeds 4096 characters"));
        }
        let voice = options
          .voice
          .as_deref()
          .filter(|v| !v.trim().is_empty() && v.len() <= 200)
          .ok_or_else(|| invalid("speech requires a configured or requested voice ID"))?;
        (
          "/v1/audio/speech".into(),
          json!({"model":model,"input":prompt,"voice":voice,"response_format":"mp3"}),
        )
      }
      Self::MinimaxH3 => {
        let min = if model == "MiniMax-H3-Max" { 5.0 } else { 4.0 };
        let duration = options.duration.unwrap_or(5.0);
        if !(min..=15.0).contains(&duration) || duration.fract() != 0.0 {
          return Err(invalid("H3 duration must be an integer from 4 to 15 (Max: 5 to 15)"));
        }
        let resolution = options.resolution.as_deref().unwrap_or("768P");
        let resolutions: &[&str] = match model {
          "MiniMax-H3" => &["768P", "2K"],
          "MiniMax-H3-Highspeed" => &["768P"],
          "MiniMax-H3-Lite" | "MiniMax-H3-Max" => &["480P", "768P"],
          _ => return Err(invalid("unsupported H3 model")),
        };
        if !resolutions.contains(&resolution) {
          return Err(invalid("resolution is not supported by this H3 model"));
        }
        let ratio = options.aspect_ratio.as_deref().unwrap_or("16:9");
        if !["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"].contains(&ratio) {
          return Err(invalid("unsupported video aspect_ratio"));
        }
        (
          "/v2/video_generation".into(),
          json!({"model":model,"content":[{"type":"text","text":prompt}],"duration":duration as u64,"resolution":resolution,"ratio":ratio}),
        )
      }
      Self::ElevenSpeech => {
        if options.duration.is_some() {
          return Err(invalid("speech duration is determined by the text"));
        }
        let voice = options
          .voice
          .as_deref()
          .ok_or_else(|| invalid("speech requires a configured or requested voice ID"))?;
        if voice.is_empty()
          || voice.len() > 100
          || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
          return Err(invalid("invalid voice ID"));
        }
        (
          format!("/v1/text-to-speech/{voice}?output_format=mp3_44100_128"),
          json!({"text":prompt,"model_id":model}),
        )
      }
      Self::ElevenSound => {
        let mut body = json!({"text":prompt,"model_id":model});
        if let Some(duration) = options.duration {
          if !(0.5..=30.0).contains(&duration) {
            return Err(invalid("sound effect duration must be 0.5..30 seconds"));
          }
          body["duration_seconds"] = json!(duration);
        }
        ("/v1/sound-generation?output_format=mp3_44100_128".into(), body)
      }
      Self::ElevenMusic => {
        let mut body = json!({"prompt":prompt,"model_id":model,"force_instrumental":options.instrumental.unwrap_or(false)});
        if let Some(duration) = options.duration {
          if !(3.0..=600.0).contains(&duration) {
            return Err(invalid("music duration must be 3..600 seconds"));
          }
          body["music_length_ms"] = json!((duration * 1000.0).round() as u64);
        }
        ("/v1/music?output_format=mp3_44100_128".into(), body)
      }
    };
    Ok((path, Draft::post_json(&body)?))
  }
  pub fn task_id(self, value: &Value) -> Result<String, String> {
    let key = if self == Self::MinimaxH3 { "task_id" } else { "id" };
    let id = value[key].as_str().filter(|s| {
      !s.is_empty()
        && s.len() <= 200
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    id.map(str::to_owned).ok_or_else(|| {
      "submission returned no valid task ID; outcome unknown, do not resubmit".into()
    })
  }
}

/// Provider-specific status fields stay in the wire codec, outside job persistence.
pub enum VideoStatus {
  Pending,
  Complete(String),
  Failed,
}
impl MediaProtocol {
  pub fn status_request(self, task: &str) -> Result<(String, Draft), Error> {
    let path = match self {
      Self::MinimaxH3 => format!("/v2/query/video_generation/{task}"),
      Self::RunwayVideo => format!("/v1/tasks/{task}"),
      Self::LumaVideo => format!("/dream-machine/v1/generations/{task}"),
      _ => return Err(Error::Build("provider has no video status API".into())),
    };
    Ok((path, Draft::get(Vec::new())))
  }
  pub fn decode_status(self, value: &Value) -> Result<VideoStatus, Error> {
    let (status, url) = match self {
      Self::MinimaxH3 => (&value["task"]["status"], &value["task"]["content"]["url"]),
      Self::RunwayVideo => (&value["status"], &value["output"][0]),
      Self::LumaVideo => (&value["state"], &value["assets"]["video"]),
      _ => return Err(Error::Build("provider has no video status API".into())),
    };
    match status.as_str().unwrap_or("").to_ascii_lowercase().as_str() {
      "succeeded" | "completed" => {
        let url =
          url.as_str().ok_or_else(|| Error::Build("completed task has no artifact URL".into()))?;
        super::image_generation::validate_url(url)?;
        Ok(VideoStatus::Complete(url.into()))
      }
      "failed" | "cancelled" | "canceled" => Ok(VideoStatus::Failed),
      "queued" | "pending" | "running" | "processing" | "throttled" | "dreaming" => {
        Ok(VideoStatus::Pending)
      }
      _ => {
        Err(Error::Build("unrecognized upstream task status; existing task was retained".into()))
      }
    }
  }
  pub fn transcription(
    self,
    model: &str,
    bytes: &[u8],
    mime: &str,
    language: Option<&str>,
    diarize: bool,
  ) -> Result<(String, Draft), Error> {
    if self.kind() != MediaKind::Transcription {
      return Err(Error::Build("provider has no transcription API".into()));
    }
    let boundary = format!("wish-{}", uuid::Uuid::new_v4());
    let mut body = Vec::new();
    let mut fields = if self == Self::ElevenTranscription {
      vec![
        ("model_id", model.to_owned()),
        ("diarize", diarize.to_string()),
        ("timestamps_granularity", "word".into()),
      ]
    } else {
      if diarize {
        return Err(Error::Build(
          "this transcription protocol does not support speaker diarization".into(),
        ));
      }
      if self == Self::OpenaiTranscription && bytes.len() > 25_000_000 {
        return Err(Error::Build("this transcription provider accepts at most 25 MB".into()));
      }
      vec![("model", model.to_owned())]
    };
    if let Some(language) = language {
      if self == Self::SiliconTranscription {
        return Err(Error::Build("SiliconFlow ASR does not support language hints".into()));
      }
      if self == Self::OpenaiTranscription && language.len() != 2 {
        return Err(Error::Build("this provider requires a two-letter ISO language code".into()));
      }
      fields.push((
        if self == Self::ElevenTranscription { "language_code" } else { "language" },
        language.into(),
      ));
    }
    if self == Self::OpenaiTranscription {
      fields.push(("response_format", "json".into()));
    }
    let extension = match mime {
      "audio/mpeg" => "mp3",
      "audio/wav" => "wav",
      "audio/flac" => "flac",
      "audio/ogg" => "ogg",
      "video/mp4" => "mp4",
      "video/webm" => "webm",
      _ => return Err(Error::Build("unsupported audio container".into())),
    };
    for (name, value) in fields {
      body.extend_from_slice(
        format!(
          "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
      );
    }
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.{extension}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((
      if self == Self::ElevenTranscription {
        "/v1/speech-to-text"
      } else {
        "/v1/audio/transcriptions"
      }
      .into(),
      Draft::post(
        None,
        vec![("content-type".into(), format!("multipart/form-data; boundary={boundary}"))],
        body,
      ),
    ))
  }
}
