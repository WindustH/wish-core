//! Jimeng 4.6 on Volcengine Visual OpenAPI, including signed POST polling.
use crate::extensions::shared::images::{ImageReply, ImageSource, ReferenceInput};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};
const MODEL: &str = "jimeng_seedream46_cvtob";
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
fn request(action: &str, body: Value) -> Result<Draft, Error> {
  let mut draft = Draft::post_json(&body)?;
  draft.query = vec![("Action".into(), action.into()), ("Version".into(), "2022-08-31".into())];
  Ok(draft)
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[ReferenceInput],
) -> Result<Draft, Error> {
  if model != MODEL {
    return Err(invalid("unsupported Jimeng model"));
  }
  for key in parameters.keys() {
    if ![
      "width",
      "height",
      "size",
      "min_ratio",
      "max_ratio",
      "scale",
      "force_single",
      "logo_info",
      "aigc_meta",
    ]
    .contains(&key.as_str())
    {
      return Err(invalid("unsupported Jimeng generation parameter"));
    }
  }
  if let Some(size) = parameters.get("size")
    && !size.as_u64().is_some_and(|n| (1_048_576..=16_777_216).contains(&n))
  {
    return Err(invalid("Jimeng size must be an integer in 1048576..16777216"));
  }
  let min = parameters
    .get("min_ratio")
    .map(Value::as_f64)
    .unwrap_or(Some(1.0 / 3.0))
    .ok_or_else(|| invalid("invalid Jimeng min_ratio"))?;
  let max = parameters
    .get("max_ratio")
    .map(Value::as_f64)
    .unwrap_or(Some(3.0))
    .ok_or_else(|| invalid("invalid Jimeng max_ratio"))?;
  if !(1.0 / 16.0..16.0).contains(&min) || max <= 1.0 / 16.0 || max > 16.0 || min > max {
    return Err(invalid("invalid Jimeng ratio bounds"));
  }
  if parameters.contains_key("width") || parameters.contains_key("height") {
    let w = parameters
      .get("width")
      .and_then(Value::as_u64)
      .ok_or_else(|| invalid("Jimeng width and height must be supplied together"))?;
    let h = parameters
      .get("height")
      .and_then(Value::as_u64)
      .ok_or_else(|| invalid("Jimeng width and height must be supplied together"))?;
    if !w.checked_mul(h).is_some_and(|area| (1_048_576..=16_777_216).contains(&area))
      || !(min..=max).contains(&(w as f64 / h as f64))
    {
      return Err(invalid("Jimeng dimensions violate area or ratio bounds"));
    }
  }
  if parameters.get("scale").is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=100).contains(&n))) {
    return Err(invalid("Jimeng scale must be an integer in 1..100"));
  }
  if parameters.get("force_single").is_some_and(|v| !v.is_boolean()) {
    return Err(invalid("Jimeng force_single must be boolean"));
  }
  for key in ["logo_info", "aigc_meta"] {
    if parameters.get(key).is_some_and(|v| !v.is_object()) {
      return Err(invalid("Jimeng logo_info and aigc_meta must be objects"));
    }
  }
  let urls = images
    .iter()
    .map(|i| match i {
      ReferenceInput::Url(url) => Ok(url),
      _ => Err(invalid("Jimeng requires public HTTPS reference URLs")),
    })
    .collect::<Result<Vec<_>, _>>()?;
  let mut values = parameters.clone();
  values.remove("logo_info");
  values.remove("aigc_meta");
  let mut body = Value::Object(values);
  body["req_key"] = json!(model);
  body["prompt"] = json!(prompt);
  if !urls.is_empty() {
    body["image_urls"] = json!(urls);
  }
  request("CVSync2AsyncSubmitTask", body)
}
pub(crate) fn status_request(
  model: &str,
  task: &str,
  parameters: &Map<String, Value>,
) -> Result<Draft, Error> {
  let mut result = json!({"return_url":true});
  for key in ["logo_info", "aigc_meta"] {
    if let Some(value) = parameters.get(key) {
      result[key] = value.clone();
    }
  }
  request(
    "CVSync2AsyncGetResult",
    json!({"req_key":model,"task_id":task,"req_json":result.to_string()}),
  )
}
pub(crate) fn decode(body: &Value) -> Result<ImageReply, Error> {
  if body["code"].as_u64() != Some(10000) {
    return Err(invalid("Jimeng request failed; do not automatically repeat generation"));
  }
  if let Some(task) = body["data"]["task_id"].as_str() {
    if task.is_empty()
      || task.len() > 200
      || !task.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
      return Err(invalid("invalid Jimeng task ID"));
    }
    return Ok(ImageReply::PendingTask { task_id: Some(task.into()) });
  }
  match body["data"]["status"].as_str() {
    Some("in_queue" | "generating") => Ok(ImageReply::PendingTask { task_id: None }),
    Some("done") => {
      let mut images = Vec::new();
      if let Some(urls) = body["data"]["image_urls"].as_array().filter(|a| !a.is_empty()) {
        for url in urls {
          let url = url.as_str().ok_or_else(|| invalid("invalid Jimeng image URL"))?;
          crate::extensions::shared::urls::validate_url(url)?;
          images.push(ImageSource::Url(url.into()));
        }
      } else if let Some(data) = body["data"]["binary_data_base64"].as_array() {
        for image in data {
          images.push(ImageSource::Base64(
            image.as_str().ok_or_else(|| invalid("invalid Jimeng image data"))?.into(),
          ));
        }
      }
      if images.is_empty() || images.len() > 15 {
        return Err(invalid("Jimeng result contains no images or exceeds fifteen images"));
      }
      Ok(ImageReply::Complete(images))
    }
    Some("not_found" | "expired") => {
      Err(invalid("Jimeng task is missing or expired; do not automatically resubmit"))
    }
    _ => Err(invalid("unrecognized Jimeng task response")),
  }
}

pub fn capabilities(_model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: 14,
    local_references: false,
    remote_references: true,
    image_required: false,
    mask: false,
    markup: false,
    options: &[],
  }
}
