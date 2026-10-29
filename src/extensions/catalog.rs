//! Extension-owned tool registration; no common business request or result schema.
use crate::protocol::Tool;
use serde_json::Value;

pub const DISCOVERY_TOOL: &str = "extension_tools";
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Permission {
  ImageGeneration,
  VideoGeneration,
  AudioGeneration,
  Transcription,
  WebSearch,
  WebFetch,
}
/// Parsed invocations retain their extension-specific type at the host boundary.
pub enum Invocation {
  /// Validated extension-owned arguments, erased only for host routing.
  Configured(Value),
  JinaFetch(super::jina::fetch_tool::Request),
  DeepseekFetch(super::deepseek::fetch_tool::Request),
  OpenaiFetch(super::openai::fetch_tool::Request),
  ZaiFetch(super::zai::fetch_tool::Request),

  SeedanceVideo(super::seedance::video_tool::Request),
  MinimaxVideo(super::minimax::video_tool::Request),
  LumaVideo(super::luma::video_tool::Request),
  RunninghubVideo(super::runninghub::video_tool::Request),
  XiaoyunqueVideo(super::xiaoyunque::video_tool::Request),

  OpenaiTranscribe(super::openai::transcribe_tool::Request),
  SiliconflowTranscribe(super::siliconflow::transcribe_tool::Request),
  ElevenlabsTranscribe(super::elevenlabs::transcribe_tool::Request),

  OpenaiSpeech(super::openai::speech_tool::Request),
  ElevenlabsSpeech(super::elevenlabs::speech_tool::Request),
  ElevenlabsMusic(super::elevenlabs::music_tool::Request),
  ElevenlabsSound(super::elevenlabs::sound_tool::Request),

  RunwayVideo(super::runway::Request),
  Novelai(super::novelai::Request),
  BflGenerate(super::bfl::Generate),
  BflFill(super::bfl::Fill),
  MinimaxSpeech(super::minimax::Speech),
  XaiTranscribe(super::xai::Transcribe),
  XaiSpeech(super::xai::speech::Request),
  XaiVideo(super::xai::video::Request),
}
pub struct Registration {
  pub extension: &'static str,
  pub name: &'static str,
  pub summary: &'static str,
  pub tags: &'static [&'static str],
  pub service_presets: &'static [&'static str],
  pub accepts_model: fn(&str) -> bool,
  pub permission: Permission,
  pub specification: fn() -> Tool,
  pub examples: fn() -> Value,
  pub parse: fn(Value) -> Result<Invocation, String>,
}
pub fn registrations() -> &'static [Registration] {
  &[
    super::bocha::search_tool::REGISTRATION,
    super::brave::search_tool::REGISTRATION,
    super::deepseek::search_tool::REGISTRATION,
    super::exa::search_tool::REGISTRATION,
    super::jina::search_tool::REGISTRATION,
    super::kimi::search_tool::REGISTRATION,
    super::metaso::search_tool::REGISTRATION,
    super::minimax::search_tool::REGISTRATION,
    super::openai::search_tool::REGISTRATION,
    super::perplexity::search_tool::REGISTRATION,
    super::searxng::search_tool::REGISTRATION,
    super::serper::search_tool::REGISTRATION,
    super::tavily::search_tool::REGISTRATION,
    super::zai::search_tool::REGISTRATION,
    super::deepseek::fetch_tool::REGISTRATION,
    super::jina::fetch_tool::REGISTRATION,
    super::openai::fetch_tool::REGISTRATION,
    super::zai::fetch_tool::REGISTRATION,
    super::openai::image_tool::REGISTRATION,
    super::openai::chatgpt_image_tool::REGISTRATION,
    super::xai::image_tool::REGISTRATION,
    super::google::image_tool::REGISTRATION,
    super::google::interactions_image_tool::REGISTRATION,
    super::together::image_tool::REGISTRATION,
    super::siliconflow::image_tool::REGISTRATION,
    super::seedance::seedream_image_tool::REGISTRATION,
    super::fal::image_tool::REGISTRATION,
    super::replicate::image_tool::REGISTRATION,
    super::alibaba::wan_image_tool::REGISTRATION,
    super::stability::image_tool::REGISTRATION,
    super::jimeng::image_tool::REGISTRATION,
    super::runninghub::image_tool::REGISTRATION,
    super::seedance::video_tool::REGISTRATION,
    super::minimax::video_tool::REGISTRATION,
    super::luma::video_tool::REGISTRATION,
    super::runninghub::video_tool::REGISTRATION,
    super::xiaoyunque::video_tool::REGISTRATION,
    super::openai::speech_tool::REGISTRATION,
    super::elevenlabs::speech_tool::REGISTRATION,
    super::elevenlabs::music_tool::REGISTRATION,
    super::elevenlabs::sound_tool::REGISTRATION,
    super::openai::transcribe_tool::REGISTRATION,
    super::siliconflow::transcribe_tool::REGISTRATION,
    super::elevenlabs::transcribe_tool::REGISTRATION,
    super::novelai::REGISTRATION,
    super::runway::REGISTRATION,
    super::bfl::GENERATE,
    super::bfl::FILL,
    super::minimax::SPEECH,
    super::xai::TRANSCRIBE,
    super::xai::speech::REGISTRATION,
    super::xai::video::REGISTRATION,
  ]
}
pub fn find(name: &str) -> Option<&'static Registration> {
  registrations().iter().find(|registration| registration.name == name)
}
pub fn is_tool(name: &str) -> bool {
  name == DISCOVERY_TOOL || find(name).is_some()
}
