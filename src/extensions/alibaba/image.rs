//! Wan 2.7 ordered references and pixel-coordinate editing regions.
use crate::extensions::shared::images::ReferenceInput;
use crate::extensions::shared::urls::validate_url;
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
pub fn reference_limit(model: &str) -> usize {
  if matches!(model, "wan2.7-image" | "wan2.7-image-pro") { 9 } else { 0 }
}
pub fn boxes(parameters: &Map<String, Value>) -> Result<Option<Vec<Vec<[u32; 4]>>>, Error> {
  let Some(value) = parameters.get("bbox_list") else {
    return Ok(None);
  };
  let boxes: Vec<Vec<[u32; 4]>> = serde_json::from_value(value.clone())
    .map_err(|_| invalid("bbox_list must contain per-image arrays of four integer coordinates"))?;
  if boxes.is_empty()
    || boxes.len() > 9
    || boxes.iter().any(|list| {
      list.len() > 2
        || list.iter().any(|[x1, y1, x2, y2]| x1 >= x2 || y1 >= y2 || *x2 > 8000 || *y2 > 8000)
    })
  {
    return Err(invalid(
      "Wan bbox_list supports 1..9 images and up to two nonempty boxes per image",
    ));
  }
  Ok(Some(boxes))
}
pub fn validate(model: &str, parameters: &Map<String, Value>, editing: bool) -> Result<(), Error> {
  boxes(parameters)?;
  for key in parameters.keys() {
    if ![
      "bbox_list",
      "size",
      "n",
      "seed",
      "watermark",
      "enable_sequential",
      "thinking_mode",
      "color_palette",
    ]
    .contains(&key.as_str())
    {
      return Err(invalid("unsupported Wan 2.7 parameter"));
    }
  }
  for key in ["watermark", "enable_sequential", "thinking_mode"] {
    if parameters.get(key).is_some_and(|v| !v.is_boolean()) {
      return Err(invalid("Wan mode/watermark options must be booleans"));
    }
  }
  let sequential = parameters.get("enable_sequential").and_then(Value::as_bool).unwrap_or(false);
  let max_count = if sequential { 12 } else { 4 };
  if parameters.get("n").is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=max_count).contains(&n)))
  {
    return Err(invalid("Wan n must be 1..4, or 1..12 with enable_sequential"));
  }
  if parameters.get("seed").is_some_and(|v| !v.as_u64().is_some_and(|v| v <= 2147483647)) {
    return Err(invalid("Wan seed must be an integer from 0 to 2147483647"));
  }
  if let Some(size) = parameters.get("size") {
    let size = size.as_str().ok_or_else(|| invalid("Wan size must be a string"))?;
    let max_pixels: u64 = if model == "wan2.7-image-pro" && !editing && !sequential {
      4096 * 4096
    } else {
      2048 * 2048
    };
    let pixels = match size {
      "1K" => 1024 * 1024,
      "2K" => 2048 * 2048,
      "4K" => 4096 * 4096,
      _ => {
        let (w, h) = size
          .split_once('*')
          .ok_or_else(|| invalid("Wan size requires 1K, 2K, 4K or WIDTH*HEIGHT"))?;
        let (w, h) = (
          w.parse::<u64>().map_err(|_| invalid("invalid Wan width"))?,
          h.parse::<u64>().map_err(|_| invalid("invalid Wan height"))?,
        );
        if w == 0 || h == 0 || w > 32768 || h > 32768 || w > h * 8 || h > w * 8 {
          return Err(invalid("Wan size aspect ratio must be 1:8..8:1"));
        }
        w * h
      }
    };
    if pixels < 768 * 768 || pixels > max_pixels {
      return Err(invalid("Wan output size exceeds this model or generation mode's pixel range"));
    }
  }
  if let Some(palette) = parameters.get("color_palette") {
    if sequential {
      return Err(invalid("Wan color_palette cannot be combined with image sets"));
    }
    let palette = palette
      .as_array()
      .filter(|p| (3..=10).contains(&p.len()))
      .ok_or_else(|| invalid("Wan color_palette requires 3..10 colors"))?;
    let mut total = 0u32;
    for color in palette {
      let object = color
        .as_object()
        .filter(|v| v.len() == 2)
        .ok_or_else(|| invalid("each palette color requires hex and ratio"))?;
      let hex = object.get("hex").and_then(Value::as_str).unwrap_or("");
      if hex.len() != 7 || !hex.starts_with('#') || !hex[1..].bytes().all(|c| c.is_ascii_hexdigit())
      {
        return Err(invalid("palette hex must be #RRGGBB"));
      }
      let ratio = object
        .get("ratio")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_suffix('%'))
        .ok_or_else(|| invalid("palette ratio requires a percentage with two decimal places"))?;
      let (whole, fraction) = ratio
        .split_once('.')
        .ok_or_else(|| invalid("palette ratio requires two decimal places"))?;
      if whole.is_empty()
        || whole.len() > 3
        || fraction.len() != 2
        || !whole.bytes().chain(fraction.bytes()).all(|c| c.is_ascii_digit())
      {
        return Err(invalid("invalid palette ratio"));
      }
      let value = whole.parse::<u32>().map_err(|_| invalid("invalid palette ratio"))? * 100
        + fraction.parse::<u32>().map_err(|_| invalid("invalid palette ratio"))?;
      if value > 10000 {
        return Err(invalid("palette ratio exceeds 100 percent"));
      }
      total += value;
    }
    if total != 10000 {
      return Err(invalid("palette ratios must total exactly 100.00%"));
    }
  }
  Ok(())
}
pub fn submission(
  model: &str,
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[ReferenceInput],
) -> Result<Draft, Error> {
  if prompt.trim().is_empty() || prompt.chars().count() > 5000 {
    return Err(invalid("Wan prompt must contain 1..5000 characters"));
  }
  if images.len() > reference_limit(model) {
    return Err(invalid("Wan model does not support this reference count"));
  }
  if let Some(boxes) = boxes(parameters)?
    && boxes.len() != images.len()
  {
    return Err(invalid("bbox_list must have one item per input image"));
  }
  validate(model, parameters, !images.is_empty())?;
  let mut content = Vec::new();
  for image in images {
    let url = match image {
      ReferenceInput::Inline(image) => format!("data:{};base64,{}", image.mime, image.data),
      ReferenceInput::Url(url) => {
        validate_url(url)?;
        url.clone()
      }
    };
    content.push(json!({"image":url}));
  }
  content.push(json!({"text":prompt}));
  Draft::post_json(
    &json!({"model":model,"input":{"messages":[{"role":"user","content":content}]},"parameters":parameters}),
  )
}

pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  {
    for part in body["output"]["choices"]
      .as_array()
      .into_iter()
      .flatten()
      .flat_map(|v| v["message"]["content"].as_array().into_iter().flatten())
    {
      if let Some(url) = part["image"].as_str() {
        images.push(crate::extensions::shared::images::ImageSource::Url(url.into()));
      }
    }
  }
  if images.is_empty() || images.len() > 12 {
    return Err(Error::Malformed("no supported images or too many images".into()));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: reference_limit(model),
    local_references: true,
    remote_references: true,
    image_required: false,
    mask: false,
    markup: false,
    options: if reference_limit(model) > 0 {
      &[
        "bbox_list",
        "size",
        "n",
        "seed",
        "watermark",
        "enable_sequential",
        "thinking_mode",
        "color_palette",
      ]
    } else {
      &[]
    },
  }
}
