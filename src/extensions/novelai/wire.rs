//! NovelAI's JSON image endpoint. Structured captions are derived from the tool prompt.
use super::references::PreciseReference;
use crate::extensions::shared::images::ReferenceImage;
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};

pub(crate) fn submission(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
) -> Result<Draft, Error> {
  render(model, prompt, parameters, None, &[])
}

pub(crate) fn edit(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
  image: &ReferenceImage,
) -> Result<Draft, Error> {
  render(model, prompt, parameters, Some(image), &[])
}

pub(crate) fn with_references(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
  image: Option<&ReferenceImage>,
  references: &[PreciseReference],
) -> Result<Draft, Error> {
  render(model, prompt, parameters, image, references)
}

fn render(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
  image: Option<&ReferenceImage>,
  references: &[PreciseReference],
) -> Result<Draft, Error> {
  let mut args = parameters.clone();
  if !references.is_empty() {
    for reference in references {
      if !(0.0..=1.0).contains(&reference.strength) || !(0.0..=1.0).contains(&reference.fidelity) {
        return Err(Error::Build("reference strength and fidelity must be in 0..1".into()));
      }
    }
    if args.keys().any(|key| {
      key.starts_with("reference_image")
        || key.starts_with("reference_strength")
        || key.starts_with("reference_information")
    }) {
      return Err(Error::Build(
        "NovelAI precise references cannot be combined with Vibe Transfer parameters".into(),
      ));
    }
    args.insert(
      "director_reference_images".into(),
      json!(references.iter().map(|r| &r.image.data).collect::<Vec<_>>()),
    );
    args.insert("director_reference_descriptions".into(), json!(references.iter().map(|r| json!({"caption":{"base_caption":r.kind.caption(),"char_captions":[]},"use_coords":false,"use_order":true})).collect::<Vec<_>>()));
    args.insert(
      "director_reference_strength_values".into(),
      json!(references.iter().map(|r| r.strength).collect::<Vec<_>>()),
    );
    args.insert(
      "director_reference_secondary_strength_values".into(),
      json!(references.iter().map(|r| r.fidelity).collect::<Vec<_>>()),
    );
    args.insert(
      "director_reference_information_extracted".into(),
      json!(vec![1.0; references.len()]),
    );
  }
  // The official API declares provider-native sampler/schedule strings rather than a stable enum.
  // Bound user input here; model-specific supported values remain the provider's contract.
  for key in ["sampler", "noise_schedule"] {
    if args.get(key).is_some_and(|value| {
      !value.as_str().is_some_and(|value| {
        !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
      })
    }) {
      return Err(Error::Build(format!(
        "NovelAI {key} must be nonempty text of at most 128 bytes without control characters"
      )));
    }
  }
  if args.get("dynamic_thresholding").is_some_and(|value| !value.is_boolean()) {
    return Err(Error::Build("NovelAI dynamic_thresholding must be boolean".into()));
  }
  for key in ["strength", "noise", "cfg_rescale"] {
    if args.get(key).is_some_and(|v| !v.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n))) {
      return Err(Error::Build(format!("NovelAI {key} must be a number in 0..1")));
    }
  }
  if let Some(image) = image {
    args.insert("image".into(), json!(image.data));
    args.entry("strength").or_insert(json!(0.7));
    args.entry("noise").or_insert(json!(0.0));
    args.insert("add_original_image".into(), json!(false));
  }
  for (key, default) in [("width", 1024), ("height", 1024), ("steps", 28), ("n_samples", 1)] {
    args.entry(key).or_insert(json!(default));
  }
  for key in ["width", "height"] {
    if !args[key].as_u64().is_some_and(|n| (64..=2048).contains(&n) && n % 64 == 0) {
      return Err(Error::Build("NovelAI dimensions must be multiples of 64 in 64..2048".into()));
    }
  }
  if !args["steps"].as_u64().is_some_and(|n| (1..=50).contains(&n))
    || !args["n_samples"].as_u64().is_some_and(|n| (1..=8).contains(&n))
  {
    return Err(Error::Build("NovelAI steps must be 1..50 and n_samples must be 1..8".into()));
  }
  args.entry("scale").or_insert(json!(5));
  if !args["scale"].as_f64().is_some_and(|n| (0.0..=10.0).contains(&n)) {
    return Err(Error::Build("NovelAI scale must be 0..10".into()));
  }
  args.entry("sampler").or_insert(json!("k_euler_ancestral"));
  args.entry("noise_schedule").or_insert(json!("karras"));
  args.entry("params_version").or_insert(json!(3));
  args.entry("seed").or_insert_with(|| json!(uuid::Uuid::new_v4().as_u128() as u32));
  if !args["seed"].as_u64().is_some_and(|n| u32::try_from(n).is_ok()) {
    return Err(Error::Build("NovelAI seed must be an unsigned 32-bit integer".into()));
  }
  let negative = args
    .entry("negative_prompt")
    .or_insert(json!(""))
    .as_str()
    .ok_or_else(|| Error::Build("NovelAI negative_prompt must be text".into()))?
    .to_owned();
  if negative.len() > 32 * 1024 {
    return Err(Error::Build("NovelAI negative_prompt must not exceed 32768 bytes".into()));
  }
  // Current prompts replace stale configured wire captions.
  super::characters::apply(model, prompt, &negative, &mut args)?;
  let mut draft = Draft::post_json(
    &json!({"input":prompt,"model":model,"action":if image.is_some() {"img2img"} else {"generate"},"parameters":args}),
  )?;
  draft.headers.push(("accept".into(), "application/json".into()));
  Ok(draft)
}

pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  {
    for item in body["images"].as_array().into_iter().flatten() {
      let data = item["image"]
        .as_str()
        .ok_or_else(|| Error::Malformed("NovelAI reply contains an invalid image".into()))?;
      images.push(crate::extensions::shared::images::ImageSource::Base64(data.into()));
    }
  }
  if images.is_empty() || images.len() > 8 {
    return Err(Error::Malformed("no supported images or too many images".into()));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: 1,
    local_references: true,
    remote_references: false,
    image_required: false,
    mask: false,
    markup: false,
    options: super::request_options(model),
  }
}
