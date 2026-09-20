use crate::protocol::{Request, error::Error, model_use::request::google_generate_content};
use serde_json::{Value, json};

pub fn render(request: &Request) -> Result<Value, Error> {
  let mut input = google_generate_content::render(request)?;
  let model = if request.model.starts_with("models/") {
    request.model.clone()
  } else {
    format!("models/{}", request.model)
  };
  input["model"] = json!(model);
  // Counting just contents would omit the system instructions and tool declarations.
  Ok(json!({"generateContentRequest": input}))
}
