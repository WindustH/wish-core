use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
  pub order: Vec<String>,
  pub providers: BTreeMap<String, MediaProvider>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaProvider {
  pub preset: String,
  pub credential_id: String,
  pub model: String,
  #[serde(default)]
  pub base_url: Option<String>,
  #[serde(default = "crate::application::config::yes")]
  pub enabled: bool,
  #[serde(default = "crate::application::config::yes")]
  pub proxy_enabled: bool,
  #[serde(default)]
  pub parameters: MediaOptions,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MediaPreset {
  pub id: String,
  pub name: String,
  pub brand: String,
  pub protocol: MediaProtocol,
  pub kind: MediaKind,
  pub model: String,
  pub base_url: String,
  pub account_presets: Vec<String>,
  pub docs_url: String,
  #[serde(default)]
  pub defaults: MediaOptions,
  #[serde(default)]
  pub resolutions: Vec<String>,
  #[serde(default)]
  pub aspect_ratios: Vec<String>,
}
pub fn presets() -> &'static Vec<MediaPreset> {
  static PRESETS: std::sync::OnceLock<Vec<MediaPreset>> = std::sync::OnceLock::new();
  PRESETS.get_or_init(|| {
    serde_json::from_str(include_str!("../../../resources/media-presets.json"))
      .expect("valid media presets")
  })
}
pub(super) fn preset(id: &str) -> Result<&'static MediaPreset, ApplicationError> {
  presets()
    .iter()
    .find(|p| p.id == id)
    .ok_or_else(|| ApplicationError::invalid(format!("unknown media preset {id}")))
}
impl MediaConfig {
  pub fn validate(
    &self,
    accounts: &BTreeMap<String, AccountConfig>,
    category: &str,
  ) -> Result<(), ApplicationError> {
    let mut seen = std::collections::HashSet::new();
    for id in &self.order {
      if !self.providers.contains_key(id) || !seen.insert(id) {
        return Err(ApplicationError::invalid("invalid media provider order"));
      }
    }
    for p in self.providers.values() {
      let spec = preset(&p.preset)?;
      if (if spec.kind == MediaKind::Video {
        "video"
      } else if spec.kind == MediaKind::Transcription {
        "transcription"
      } else {
        "audio"
      }) != category
      {
        return Err(ApplicationError::invalid("provider belongs to a different media category"));
      }
      let account = accounts
        .get(&p.credential_id)
        .ok_or_else(|| ApplicationError::invalid("media service requires an account"))?;
      if account
        .preset
        .as_deref()
        .is_some_and(|p| !spec.account_presets.iter().any(|allowed| allowed == p))
      {
        return Err(ApplicationError::invalid(
          "account type is incompatible with this media provider",
        ));
      }
      let base = p.base_url.as_deref().filter(|v| !v.is_empty()).unwrap_or(&spec.base_url);
      crate::protocol::image_generation::validate_base_url(base)?;
      if p.model.is_empty() || p.model.len() > 200 {
        return Err(ApplicationError::invalid("invalid media model ID"));
      }
      // Validate configured defaults without requiring a voice before it is chosen at call time.
      let mut options = p.parameters.clone();
      if spec.kind == MediaKind::Speech && options.voice.is_none() {
        options.voice = Some("validation".into());
      }
      if spec.kind == MediaKind::Transcription {
        if spec.protocol == MediaProtocol::ElevenTranscription
          && !matches!(p.model.as_str(), "scribe_v1" | "scribe_v2")
        {
          return Err(ApplicationError::invalid("unsupported ASR model"));
        }
        if serde_json::to_value(&options)
          .map_err(ApplicationError::internal)?
          .as_object()
          .is_some_and(|o| o.values().any(|v| !v.is_null()))
        {
          return Err(ApplicationError::invalid("ASR defaults are specified per tool call"));
        }
      } else {
        spec.protocol.submission(&p.model, "validation", &options)?;
      }
    }
    Ok(())
  }
}
