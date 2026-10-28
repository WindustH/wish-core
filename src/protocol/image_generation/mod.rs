//! Image wire codecs. Submission, polling and storage policy belong to application.
use crate::protocol::{
  attempt::{Call, Method},
  endpoint::{AuthScheme, Credentials, Draft, Endpoint},
  error::Error,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageProtocol {
  Openai,
  Codex,
  Gemini,
  GeminiInteractions,
  Together,
  Siliconflow,
  Seedream,
  Bfl,
  Fal,
  Replicate,
  Wan,
  Stability,
}

pub enum ImageSource {
  Base64(String),
  Url(String),
}
pub enum ImageReply {
  Complete(Vec<ImageSource>),
  Pending { url: String, result_url: Option<String> },
}

pub fn validate_url(url: &str) -> Result<reqwest::Url, Error> {
  let parsed =
    reqwest::Url::parse(url).map_err(|_| Error::Build("invalid image service URL".into()))?;
  if !matches!(parsed.scheme(), "https" | "http")
    || parsed.host_str().is_none()
    || !parsed.username().is_empty()
    || parsed.password().is_some()
  {
    return Err(Error::Build("image URL must be HTTP(S) without credentials".into()));
  }
  Ok(parsed)
}
impl ImageProtocol {
  pub fn call(
    self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error> {
    let auth = match self {
      Self::Gemini | Self::GeminiInteractions => AuthScheme::Header("x-goog-api-key"),
      Self::Bfl => AuthScheme::Header("x-key"),
      Self::Fal => AuthScheme::None,
      _ => AuthScheme::Bearer(None),
    };
    let mut endpoint = Endpoint::new(base, path, auth)?;
    if self == Self::Fal {
      endpoint = endpoint.with_header("authorization", &format!("Key {}", credentials.api_key));
    }
    if self == Self::Codex {
      endpoint = endpoint
        .with_header("originator", "codex_cli_rs")
        .with_header("x-codex-image-turn-id", &uuid::Uuid::new_v4().to_string());
      if let Some(id) = &credentials.account_id {
        endpoint = endpoint.with_header("chatgpt-account-id", id);
      }
    }
    endpoint.build_call(draft, credentials, crate::utils::time::unix_seconds())
  }
  pub fn submission(
    self,
    model: &str,
    prompt: &str,
    parameters: &serde_json::Map<String, Value>,
  ) -> Result<Draft, Error> {
    let mut args = parameters.clone();
    args.insert("prompt".into(), json!(prompt));
    let body = match self {
      Self::Gemini => {
        json!({"contents":[{"role":"user","parts":[{"text":prompt}]}],"generationConfig":parameters})
      }
      Self::GeminiInteractions => {
        json!({"model":model,"input":[{"type":"text","text":prompt}],"generation_config":parameters})
      }
      Self::Replicate => json!({"input":args}),
      Self::Wan => {
        json!({"model":model,"input":{"messages":[{"role":"user","content":[{"text":prompt}]}]},"parameters":parameters})
      }
      Self::Bfl | Self::Fal => Value::Object(args),
      Self::Stability => {
        let boundary = format!("wish-{}", uuid::Uuid::new_v4());
        let mut body = String::new();
        for (key, value) in args {
          if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Build("invalid image parameter name".into()));
          }
          let value = value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
          body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
          ));
        }
        body.push_str(&format!("--{boundary}--\r\n"));
        return Ok(Draft::post(
          None,
          vec![
            ("content-type".into(), format!("multipart/form-data; boundary={boundary}")),
            ("accept".into(), "application/json".into()),
          ],
          body.into_bytes(),
        ));
      }
      _ => {
        args.insert("model".into(), json!(model));
        Value::Object(args)
      }
    };
    Draft::post_json(&body)
  }
  pub fn decode(
    self,
    body: &Value,
    current_url: &str,
    result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    let mut images = Vec::new();
    let mut add = |item: &Value| {
      if let Some(data) =
        item.get("b64_json").or_else(|| item.get("base64")).and_then(Value::as_str)
      {
        images.push(ImageSource::Base64(data.into()));
      } else if let Some(url) = item.get("url").and_then(Value::as_str).or_else(|| item.as_str()) {
        images.push(ImageSource::Url(url.into()));
      }
    };
    match self {
      Self::Gemini => {
        for part in body["candidates"]
          .as_array()
          .into_iter()
          .flatten()
          .flat_map(|v| v["content"]["parts"].as_array().into_iter().flatten())
        {
          if let Some(data) =
            part["inlineData"]["data"].as_str().or_else(|| part["inline_data"]["data"].as_str())
          {
            images.push(ImageSource::Base64(data.into()));
          }
        }
      }
      Self::GeminiInteractions => collect_interaction_images(body, &mut images),
      Self::Bfl => {
        if let Some(url) = body["result"]["sample"].as_str() {
          images.push(ImageSource::Url(url.into()));
        } else if matches!(
          body["status"].as_str(),
          Some("Error" | "Failed" | "Request Moderated" | "Content Moderated")
        ) {
          return Err(Error::Malformed("image generation failed or was moderated".into()));
        } else if body["id"].is_string()
          || matches!(body["status"].as_str(), Some("Pending" | "Processing"))
        {
          return Ok(ImageReply::Pending {
            url: body["polling_url"].as_str().unwrap_or(current_url).into(),
            result_url: None,
          });
        }
      }
      Self::Fal => {
        if body.get("error").is_some_and(|v| !v.is_null()) {
          return Err(Error::Malformed("image generation failed".into()));
        }
        for image in body["images"].as_array().into_iter().flatten() {
          add(image);
        }
        if images.is_empty() {
          let result = body["response_url"].as_str().or(result_url);
          let url = if body["status"] == "COMPLETED" {
            result
          } else {
            body["status_url"].as_str().or_else(|| {
              matches!(body["status"].as_str(), Some("IN_QUEUE" | "IN_PROGRESS"))
                .then_some(current_url)
            })
          };
          if let Some(url) = url {
            return Ok(ImageReply::Pending {
              url: url.into(),
              result_url: result.map(str::to_owned),
            });
          }
        }
      }
      Self::Replicate => match body["status"].as_str() {
        Some("failed" | "canceled") => {
          return Err(Error::Malformed("image prediction failed or was canceled".into()));
        }
        Some("succeeded") => {
          if let Some(output) = body["output"].as_array() {
            for image in output {
              add(image);
            }
          } else {
            add(&body["output"]);
          }
        }
        Some("starting" | "processing") => {
          return Ok(ImageReply::Pending {
            url: body["urls"]["get"].as_str().unwrap_or(current_url).into(),
            result_url: None,
          });
        }
        _ => {}
      },
      Self::Wan => {
        for part in body["output"]["choices"]
          .as_array()
          .into_iter()
          .flatten()
          .flat_map(|v| v["message"]["content"].as_array().into_iter().flatten())
        {
          if let Some(url) = part["image"].as_str() {
            images.push(ImageSource::Url(url.into()));
          }
        }
      }
      Self::Stability => {
        if body["finish_reason"] == "CONTENT_FILTERED" {
          return Err(Error::Malformed("image generation was moderated".into()));
        }
        if let Some(data) = body["image"].as_str() {
          images.push(ImageSource::Base64(data.into()));
        }
      }
      Self::Siliconflow => {
        for image in body["images"].as_array().into_iter().flatten() {
          add(image);
        }
      }
      _ => {
        for image in body["data"].as_array().into_iter().flatten() {
          add(image);
        }
      }
    }
    if images.is_empty() || images.len() > 8 {
      return Err(Error::Malformed(
        "image response contains no supported images or exceeds eight images".into(),
      ));
    }
    Ok(ImageReply::Complete(images))
  }
}
fn collect_interaction_images(value: &Value, images: &mut Vec<ImageSource>) {
  match value {
    Value::Object(fields) => {
      if value["type"] == "image"
        && let Some(data) = value["data"].as_str()
      {
        images.push(ImageSource::Base64(data.into()));
        return;
      }
      for key in ["outputs", "steps", "content", "output", "output_image"] {
        if let Some(v) = fields.get(key) {
          collect_interaction_images(v, images);
        }
      }
    }
    Value::Array(items) => {
      for item in items {
        collect_interaction_images(item, images);
      }
    }
    _ => {}
  }
}
/// Polling keeps credentials on the configured origin (BFL also returns regional origins).
pub fn poll_call(protocol: ImageProtocol, initial: &Call, url: &str) -> Result<Call, Error> {
  let origin = validate_url(&initial.url)?;
  let target = validate_url(url)?;
  let bfl = protocol == ImageProtocol::Bfl
    && origin.scheme() == "https"
    && target.scheme() == "https"
    && origin.host_str().is_some_and(|h| h == "api.bfl.ai" || h.ends_with(".bfl.ai"))
    && target.host_str().is_some_and(|h| h.ends_with(".bfl.ai"))
    && target.port_or_known_default() == Some(443);
  if origin.origin() != target.origin() && !bfl {
    return Err(Error::Build("image polling URL has an untrusted origin".into()));
  }
  Ok(Call {
    method: Method::Get,
    url: target.into(),
    headers: initial
      .headers
      .iter()
      .filter(|(key, _)| !key.eq_ignore_ascii_case("content-type"))
      .cloned()
      .collect(),
    body: Vec::new(),
  })
}
