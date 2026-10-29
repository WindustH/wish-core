//! Upload session-owned references only after local validation, before paid generation.
use crate::extensions::shared::images::ReferenceInput;
use crate::protocol::{attempt::Transport, endpoint::Credentials};
use base64::{Engine, engine::general_purpose::STANDARD};

pub(crate) async fn references(
  images: Vec<ReferenceInput>,
  transport: &(impl Transport + Sync),
  base: &str,
  credentials: &Credentials,
) -> Result<Vec<ReferenceInput>, String> {
  let prepared = crate::utils::blocking::cpu(move || {
    let mut total = 0usize;
    images
      .into_iter()
      .map(|image| match image {
        ReferenceInput::Url(url) => Ok((Some(url), Vec::new())),
        ReferenceInput::Inline(image) => {
          let normalized = crate::extensions::shared::image_pixels::encode(
            crate::extensions::shared::image_pixels::decode(&image, true)?,
            12 * 1024 * 1024,
          )?;
          let bytes = STANDARD.decode(normalized.data).map_err(|e| e.to_string())?;
          total += bytes.len();
          if total > 12 * 1024 * 1024 {
            return Err("prepared RunningHub references exceed 12 MiB".into());
          }
          Ok((None, bytes))
        }
      })
      .collect::<Result<Vec<_>, String>>()
  })
  .await
  .map_err(|e| e.to_string())??;
  let mut images = Vec::new();
  for (url, bytes) in prepared {
    let url = if let Some(url) = url {
      url
    } else {
      super::upload::send(bytes, "image/png", base, credentials, transport).await?
    };
    images.push(ReferenceInput::Url(url));
  }
  Ok(images)
}
