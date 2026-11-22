use super::{Error, Result};
use chromiumoxide::{
  Page,
  cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    InsertTextParams,
  },
  layout::Point,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
  args[key].as_str().ok_or_else(|| Error::invalid(format!("expected string field {key}")))
}
pub(super) fn address(args: &Value) -> Result<&str> {
  let url = string(args, "url")?;
  if !reqwest::Url::parse(url)
    .is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.host().is_some())
  {
    return Err(Error::invalid("expected an HTTP or HTTPS URL"));
  }
  Ok(url)
}
pub(super) async fn evaluate(page: &Page, expression: String) -> Result<Value> {
  let result = page.evaluate_expression(expression).await.map_err(Error::internal)?;
  Ok(result.object().value.clone().unwrap_or(Value::Null))
}
pub(super) async fn dom(page: &Page, args: &Value) -> Result<Value> {
  let expression = format!(
    "(()=>{{try{{return {{value:({})({args})}}}}catch(e){{return {{error:String(e?.message ?? e)}}}}}})()",
    include_str!("dom.js")
  );
  let mut result = evaluate(page, expression).await?;
  if let Some(message) = result["error"].as_str() {
    return Err(Error::conflict(message));
  }
  Ok(result["value"].take())
}
fn number(args: &Value, key: &str) -> Result<f64> {
  args[key].as_f64().ok_or_else(|| Error::invalid(format!("expected number field {key}")))
}
async fn press_key(page: &Page, key: &str) -> Result<()> {
  let def = chromiumoxide::keys::get_key_definition(key)
    .ok_or_else(|| Error::invalid(format!("Unknown key: {key}")))?;
  let command = DispatchKeyEventParams::builder()
    .key(def.key)
    .code(def.code)
    .windows_virtual_key_code(def.key_code)
    .native_virtual_key_code(def.key_code);
  let text = def.text.or_else(|| (def.key.len() == 1).then_some(def.key));
  let down = match text {
    Some(text) => command.clone().r#type(DispatchKeyEventType::KeyDown).text(text),
    None => command.clone().r#type(DispatchKeyEventType::RawKeyDown),
  };
  page.execute(down.build().map_err(Error::invalid)?).await.map_err(Error::internal)?;
  page
    .execute(command.r#type(DispatchKeyEventType::KeyUp).build().map_err(Error::invalid)?)
    .await
    .map_err(Error::internal)?;
  Ok(())
}
pub(super) async fn interact(page: &Page, action: &str, a: &Value) -> Result<()> {
  match action {
    "click" => {
      let coords = if a["ref"].is_string() { dom(page, a).await? } else { a.clone() };
      page
        .click(Point::new(number(&coords, "x")?, number(&coords, "y")?))
        .await
        .map_err(Error::internal)?;
    }
    "fill" => {
      let text = string(a, "text")?;
      string(a, "ref")?;
      dom(page, a).await?;
      if text.is_empty() {
        press_key(page, "Backspace").await?;
      } else {
        page.execute(InsertTextParams::new(text)).await.map_err(Error::internal)?;
      }
    }
    "select" => {
      string(a, "ref")?;
      string(a, "value")?;
      dom(page, a).await?;
    }
    "press" => {
      if a["ref"].is_string() {
        dom(page, a).await?;
      }
      press_key(page, string(a, "key")?).await?;
    }
    "scroll" => {
      let event = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MouseWheel)
        .x(a["x"].as_f64().unwrap_or(400.))
        .y(a["y"].as_f64().unwrap_or(300.))
        .delta_x(a["dx"].as_f64().unwrap_or(0.))
        .delta_y(number(a, "dy")?)
        .build()
        .map_err(Error::invalid)?;
      page.execute(event).await.map_err(Error::internal)?;
    }
    "back" => {
      evaluate(page, "history.back(); null".into()).await?;
    }
    _ => return Err(Error::invalid("unknown browser action")),
  }
  Ok(())
}
pub(super) async fn wait(page: &Page, a: &Value) -> Result<Value> {
  if a["selector"].as_str().or(a["text"].as_str()).is_none_or(str::is_empty) {
    return Err(Error::invalid("wait needs text or selector"));
  }
  let deadline = Instant::now() + Duration::from_secs(10);
  while Instant::now() < deadline {
    if dom(page, a).await? == true {
      return Ok(json!({"matched":true}));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
  }
  Err(Error::conflict("timed out waiting for page condition"))
}
