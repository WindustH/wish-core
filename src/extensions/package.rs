//! Bundled package composition. Provider behavior is implemented by each package.
use super::{
  catalog::Registration,
  image_runtime::ImageDriver,
  media_runtime::MediaDriver,
  shared::read_codecs::{FetchCodec, SearchCodec},
};
pub struct Package {
  pub id: &'static str,
  pub tools: &'static [&'static Registration],
  pub images: &'static [&'static dyn ImageDriver],
  pub media: &'static [&'static dyn MediaDriver],
  pub search: &'static [&'static SearchCodec],
  pub fetch: &'static [&'static FetchCodec],
  pub presets: &'static str,
}
pub fn packages() -> &'static [Package] {
  &[
    super::alibaba::package::PACKAGE,
    super::bfl::package::PACKAGE,
    super::bocha::package::PACKAGE,
    super::brave::package::PACKAGE,
    super::deepseek::package::PACKAGE,
    super::elevenlabs::package::PACKAGE,
    super::exa::package::PACKAGE,
    super::fal::package::PACKAGE,
    super::google::package::PACKAGE,
    super::jimeng::package::PACKAGE,
    super::jina::package::PACKAGE,
    super::kimi::package::PACKAGE,
    super::luma::package::PACKAGE,
    super::metaso::package::PACKAGE,
    super::minimax::package::PACKAGE,
    super::novelai::package::PACKAGE,
    super::openai::package::PACKAGE,
    super::perplexity::package::PACKAGE,
    super::replicate::package::PACKAGE,
    super::runninghub::package::PACKAGE,
    super::runway::package::PACKAGE,
    super::searxng::package::PACKAGE,
    super::seedance::package::PACKAGE,
    super::serper::package::PACKAGE,
    super::siliconflow::package::PACKAGE,
    super::stability::package::PACKAGE,
    super::tavily::package::PACKAGE,
    super::together::package::PACKAGE,
    super::xai::package::PACKAGE,
    super::xiaoyunque::package::PACKAGE,
    super::zai::package::PACKAGE,
  ]
}
pub fn presets(category: &str) -> Vec<serde_json::Value> {
  packages()
    .iter()
    .flat_map(|package| {
      let value: serde_json::Value =
        serde_json::from_str(package.presets).expect("valid extension presets");
      value[category].as_array().cloned().unwrap_or_default()
    })
    .collect()
}
