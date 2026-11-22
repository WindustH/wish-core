//! Reusable remote image and prefix contracts selected by a provider.
use super::ModelUseProtocol;
use crate::protocol::error::Error;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileProtocol {
  Openai,
  Deepseek,
  Anthropic,
  Kimi,
  Gemini,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrefixProtocol {
  Gemini,
  Vertex,
  Ark,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource<P> {
  pub protocol: P,
  pub path: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceConfig {
  #[serde(
    default,
    deserialize_with = "image_locations",
    skip_serializing_if = "std::collections::BTreeMap::is_empty"
  )]
  pub image_locations: std::collections::BTreeMap<String, String>,
  pub files: Option<Resource<FileProtocol>>,
  pub prefix: Option<Resource<PrefixProtocol>>,
}
fn image_locations<'de, D: serde::Deserializer<'de>>(
  input: D,
) -> Result<std::collections::BTreeMap<String, String>, D::Error> {
  let mut locations = std::collections::BTreeMap::new();
  for (digest, uri) in std::collections::BTreeMap::<String, String>::deserialize(input)? {
    if locations.insert(digest.to_ascii_lowercase(), uri).is_some() {
      return Err(serde::de::Error::custom("duplicate image digest"));
    }
  }
  Ok(locations)
}
impl ResourceConfig {
  pub fn validate(
    &self,
    model: ModelUseProtocol,
    incremental: Option<super::incremental::IncrementalProtocol>,
  ) -> Result<(), Error> {
    use crate::protocol::model_use::mode::{
      ChatCompletionApiCompatMode as C, MessagesApiCompatMode as A, ResponsesDeployment,
    };
    use ModelUseProtocol::*;
    let platform =
      matches!(model,OpenAiResponses(m) if m.deployment==ResponsesDeployment::Platform);
    let path_ok = |s: &str| {
      s.starts_with('/')
        && !s.starts_with("//")
        && !s.contains(['#', '?'])
        && !s.split('/').any(|p| p == "..")
    };
    for (digest, location) in &self.image_locations {
      let scheme = match model {
        BedrockConverse => "s3",
        GoogleVertexGenerateContent => "gs",
        OpenAiChat(C::Qwen) => "oss",
        _ => {
          return Err(Error::Build(
            "model wire does not accept object storage image locations".into(),
          ));
        }
      };
      if digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || reqwest::Url::parse(location).ok().is_none_or(|u| {
          u.scheme() != scheme
            || u.host_str().is_none()
            || u.path().len() < 2
            || !u.username().is_empty()
            || u.password().is_some()
        })
      {
        return Err(Error::Build("invalid image digest or object storage URI".into()));
      }
    }
    if let Some(f) = &self.files {
      let valid = match f.protocol {
        FileProtocol::Openai => platform,
        FileProtocol::Deepseek => {
          platform || matches!(model, OpenAiChat(C::DeepSeek) | AnthropicMessages(A::DeepSeek))
        }
        FileProtocol::Anthropic => matches!(model, AnthropicMessages(A::Official)),
        FileProtocol::Kimi => matches!(model, OpenAiChat(C::KimiK2 | C::KimiK3)),
        FileProtocol::Gemini => matches!(model, GoogleGenerateContent | GoogleInteractions),
      };
      if !valid || !path_ok(&f.path) {
        return Err(Error::Build("invalid image file protocol or upload path".into()));
      }
    }
    if let Some(p) = &self.prefix {
      let valid = match p.protocol {
        PrefixProtocol::Gemini => model == GoogleGenerateContent,
        PrefixProtocol::Vertex => model == GoogleVertexGenerateContent,
        PrefixProtocol::Ark => {
          platform
            && matches!(
              incremental,
              None
                | Some(
                  super::incremental::IncrementalProtocol::Disabled
                    | super::incremental::IncrementalProtocol::ArkResponses
                )
            )
        }
      };
      if !valid || !path_ok(&p.path) {
        return Err(Error::Build("invalid prefix cache protocol or creation path".into()));
      }
    }
    Ok(())
  }
}
