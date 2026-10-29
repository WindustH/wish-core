//! NovelAI V4.5 structured character captions. Never pass host convenience fields upstream.
use crate::protocol::error::Error;
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Character {
  prompt: String,
  #[serde(default)]
  negative_prompt: String,
  #[serde(default)]
  centers: Vec<Center>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Center {
  x: f64,
  y: f64,
}

pub(crate) fn supported(model: &str) -> bool {
  matches!(model, "nai-diffusion-4-5-full" | "nai-diffusion-4-5-curated")
}

fn parse(model: &str, value: Option<&Value>) -> Result<Vec<Character>, Error> {
  let Some(value) = value else {
    return Ok(Vec::new());
  };
  let characters: Vec<Character> = serde_json::from_value(value.clone())
    .map_err(|e| Error::Build(format!("invalid NovelAI character_prompts: {e}")))?;
  if !characters.is_empty() && !supported(model) {
    return Err(Error::Build(
      "NovelAI character_prompts currently require a V4.5 full/curated model".into(),
    ));
  }
  if characters.len() > 6 {
    return Err(Error::Build("NovelAI V4.5 supports at most six character prompts".into()));
  }
  let positioned = characters.iter().any(|character| !character.centers.is_empty());
  for character in &characters {
    if character.prompt.trim().is_empty()
      || character.prompt.len() > 4096
      || character.negative_prompt.len() > 4096
    {
      return Err(Error::Build("NovelAI character prompt must contain 1..4096 bytes and negative_prompt at most 4096 bytes".into()));
    }
    if character.centers.len() > 6 || (positioned && character.centers.is_empty()) {
      return Err(Error::Build("NovelAI explicit positioning requires 1..6 centers for every character; omit all centers for automatic placement".into()));
    }
    if character
      .centers
      .iter()
      .any(|center| !(0.0..=1.0).contains(&center.x) || !(0.0..=1.0).contains(&center.y))
    {
      return Err(Error::Build("NovelAI character center coordinates must be in 0..1".into()));
    }
  }
  Ok(characters)
}

pub(crate) fn validate(model: &str, parameters: &Map<String, Value>) -> Result<(), Error> {
  parse(model, parameters.get("character_prompts")).map(|_| ())
}

pub(crate) fn apply(
  model: &str,
  prompt: &str,
  negative: &str,
  parameters: &mut Map<String, Value>,
) -> Result<(), Error> {
  let characters = parse(model, parameters.get("character_prompts"))?;
  let positioned = characters.iter().any(|character| !character.centers.is_empty());
  let captions = |negative: bool| {
    characters.iter().map(|character| {
    let centers = if positioned {
      character.centers.iter().map(|center| json!({"x":center.x,"y":center.y})).collect::<Vec<_>>()
    } else {
      vec![json!({"x":0.5,"y":0.5})]
    };
    json!({"char_caption": if negative { &character.negative_prompt } else { &character.prompt }, "centers":centers})
  }).collect::<Vec<_>>()
  };
  parameters.remove("character_prompts");
  parameters.insert("v4_prompt".into(), json!({"caption":{"base_caption":prompt,"char_captions":captions(false)},"use_coords":positioned,"use_order":true}));
  parameters.insert("v4_negative_prompt".into(), json!({"caption":{"base_caption":negative,"char_captions":captions(true)},"use_coords":positioned,"use_order":true}));
  Ok(())
}

pub(crate) fn schema() -> Value {
  json!({"type":"array","maxItems":6,"description":"V4.5 full/curated only. Separate character descriptions in scene order; base prompt describes scene/style and subject count. Empty array clears configured characters. Omit all centers for automatic placement, or provide centers for every character. Explicit centers are normalized 0..1 from left/top; keep their order consistent with descriptions. Local text limit is 4096 bytes per prompt; provider token limits still apply.","items":{"type":"object","additionalProperties":false,"required":["prompt"],"properties":{
    "prompt":{"type":"string","minLength":1,"maxLength":4096},
    "negative_prompt":{"type":"string","maxLength":4096},
    "centers":{"type":"array","minItems":1,"maxItems":6,"items":{"type":"object","additionalProperties":false,"required":["x","y"],"properties":{"x":{"type":"number","minimum":0,"maximum":1},"y":{"type":"number","minimum":0,"maximum":1}}}}
  }}})
}
