//! MiniMax-specific speech controls. No other extension inherits this schema.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Parameters {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub timbre_weights: Option<Vec<TimbreWeight>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub subtitle_enable: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub subtitle_type: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub language_boost: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub text_normalization_mode: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub latex_read: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub pronunciation_dict: Option<Pronunciation>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub audio_setting: Option<Audio>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub voice_modify: Option<Modify>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimbreWeight {
  pub voice_id: String,
  pub weight: u8,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pronunciation {
  pub tone: Vec<String>,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Audio {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub format: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub sample_rate: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub bitrate: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub channel: Option<u8>,
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Modify {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub pitch: Option<i32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub intensity: Option<i32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub timbre: Option<i32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub sound_effects: Option<String>,
}
const LANGUAGES: &[&str] = &[
  "Chinese",
  "Chinese,Yue",
  "English",
  "Arabic",
  "Russian",
  "Spanish",
  "French",
  "Portuguese",
  "German",
  "Turkish",
  "Dutch",
  "Ukrainian",
  "Vietnamese",
  "Indonesian",
  "Japanese",
  "Italian",
  "Korean",
  "Thai",
  "Polish",
  "Romanian",
  "Greek",
  "Czech",
  "Finnish",
  "Hindi",
  "Bulgarian",
  "Danish",
  "Hebrew",
  "Malay",
  "Persian",
  "Slovak",
  "Swedish",
  "Croatian",
  "Filipino",
  "Hungarian",
  "Norwegian",
  "Slovenian",
  "Catalan",
  "Nynorsk",
  "Tamil",
  "Afrikaans",
  "auto",
];

impl Parameters {
  pub fn apply(&self, model: &str, body: &mut Value) -> Result<(), String> {
    if let Some(kind) = self.subtitle_type.as_deref()
      && (!["sentence", "word"].contains(&kind) || self.subtitle_enable != Some(true))
    {
      return Err(
        "MiniMax subtitle_type requires subtitle_enable=true and sentence or word".into(),
      );
    }
    let format = self.audio_setting.as_ref().and_then(|a| a.format.as_deref()).unwrap_or("mp3");
    if !["mp3", "wav", "flac", "pcmu_wav", "opus"].contains(&format) {
      return Err(
        "MiniMax stored audio requires mp3, wav, flac, pcmu_wav or opus; raw PCM is not supported"
          .into(),
      );
    }
    if let Some(a) = &self.audio_setting {
      if a.sample_rate.is_some_and(|v| {
        ![8000, 16000, 22050, 24000, 32000, 44100].contains(&v)
          || (format == "pcmu_wav" && v != 8000)
      }) {
        return Err("invalid MiniMax sample rate (pcmu_wav requires 8000 Hz)".into());
      }
      if a.bitrate.is_some_and(|v| format != "mp3" || ![32000, 64000, 128000, 256000].contains(&v))
      {
        return Err("MiniMax bitrate requires MP3 and 32000, 64000, 128000 or 256000".into());
      }
      if a.channel.is_some_and(|v| ![1, 2].contains(&v)) {
        return Err("MiniMax channel must be 1 or 2".into());
      }
    }
    if let Some(language) = self.language_boost.as_deref() {
      if !LANGUAGES.contains(&language) {
        return Err("unsupported MiniMax language_boost".into());
      }
      if (model.starts_with("speech-01") || model.starts_with("speech-02"))
        && ["Persian", "Filipino", "Tamil"].contains(&language)
      {
        return Err("this MiniMax model does not support the requested language".into());
      }
      if self.latex_read == Some(true) && language != "Chinese" && language != "auto" {
        return Err("MiniMax LaTeX reading requires Chinese".into());
      }
    }
    if self.text_normalization_mode.as_deref().is_some_and(|v| !["basic", "quality"].contains(&v)) {
      return Err("MiniMax text_normalization_mode must be basic or quality".into());
    }
    if let Some(dictionary) = &self.pronunciation_dict {
      // Local resource limits, not upstream capability limits.
      if dictionary.tone.len() > 1000
        || dictionary.tone.iter().any(|s| {
          s.len() > 4096
            || !s.split_once('/').is_some_and(|(a, b)| !a.trim().is_empty() && !b.trim().is_empty())
        })
      {
        return Err("pronunciation_dict accepts up to 1000 original/replacement rules, each at most 4096 bytes".into());
      }
    }
    if let Some(modify) = &self.voice_modify {
      if !["mp3", "wav", "flac"].contains(&format) {
        return Err("MiniMax voice_modify requires mp3, wav or flac".into());
      }
      if [modify.pitch, modify.intensity, modify.timbre]
        .into_iter()
        .flatten()
        .any(|v| !(-100..=100).contains(&v))
      {
        return Err("MiniMax voice effects must be integers from -100 to 100".into());
      }
      if modify.sound_effects.as_deref().is_some_and(|v| {
        !["spacious_echo", "auditorium_echo", "lofi_telephone", "robotic"].contains(&v)
      }) {
        return Err("unsupported MiniMax sound effect".into());
      }
    }
    let mut fields = serde_json::to_value(self).map_err(|e| e.to_string())?;
    let map = fields.as_object_mut().ok_or("invalid MiniMax parameters")?;
    map.remove("latex_read");
    // Audio settings replace the fixed legacy defaults: don't leak MP3 bitrate into WAV.
    for (key, value) in map.iter() {
      body[key] = value.clone();
    }
    if let Some(latex) = self.latex_read {
      body["voice_setting"]["latex_read"] = json!(latex);
      if latex {
        body["language_boost"] = json!("Chinese");
      }
    }
    Ok(())
  }
}
pub fn schema() -> Value {
  json!({"type":"object","additionalProperties":false,"properties":{
    "timbre_weights":{"type":"array","minItems":1,"maxItems":4,"description":"Mix voices using relative integer weights; leave voice.id unset. Overrides the configured single voice, while other voice controls inherit defaults.","items":{"type":"object","additionalProperties":false,"required":["voice_id","weight"],"properties":{"voice_id":{"type":"string","minLength":1,"maxLength":200},"weight":{"type":"integer","minimum":1,"maximum":100}}}},
    "subtitle_enable":{"type":"boolean","description":"Save the returned subtitle JSON alongside audio in Wish blobs."},
    "subtitle_type":{"type":"string","enum":["sentence","word"],"description":"Requires subtitle_enable=true; default sentence."},
    "language_boost":{"type":"string","enum":LANGUAGES},
    "text_normalization_mode":{"type":"string","enum":["basic","quality"],"description":"quality has effect on speech-2.6/2.8; older models fall back to basic."},
    "latex_read":{"type":"boolean","description":"Read $$...$$ formulas in Chinese; true forces Chinese language."},
    "pronunciation_dict":{"type":"object","additionalProperties":false,"properties":{"tone":{"type":"array","maxItems":1000,"items":{"type":"string","description":"original/replacement; Wish limits each rule to 4096 bytes."}}}},
    "audio_setting":{"type":"object","additionalProperties":false,"properties":{
      "format":{"type":"string","enum":["mp3","wav","flac","pcmu_wav","opus"]},
      "sample_rate":{"type":"integer","enum":[8000,16000,22050,24000,32000,44100]},
      "bitrate":{"type":"integer","enum":[32000,64000,128000,256000],"description":"MP3 only."},
      "channel":{"type":"integer","enum":[1,2]}}},
    "voice_modify":{"type":"object","additionalProperties":false,"properties":{
      "pitch":{"type":"integer","minimum":-100,"maximum":100},
      "intensity":{"type":"integer","minimum":-100,"maximum":100},
      "timbre":{"type":"integer","minimum":-100,"maximum":100},
      "sound_effects":{"type":"string","enum":["spacious_echo","auditorium_echo","lofi_telephone","robotic"]}}}
  }})
}
