//! NovelAI's JSON image endpoint. Structured captions are derived from the tool prompt.
use super::*;

pub(super) fn submission(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
) -> Result<Draft, Error> {
  let mut args = parameters.clone();
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
  // Wish supplies the current prompt on every call; stale configured captions cannot override it.
  args.insert("v4_prompt".into(), json!({"caption":{"base_caption":prompt,"char_captions":[]},"use_coords":false,"use_order":true}));
  args.insert("v4_negative_prompt".into(), json!({"caption":{"base_caption":negative,"char_captions":[]},"use_coords":false,"use_order":true}));
  let mut draft =
    Draft::post_json(&json!({"input":prompt,"model":model,"action":"generate","parameters":args}))?;
  draft.headers.push(("accept".into(), "application/json".into()));
  Ok(draft)
}
