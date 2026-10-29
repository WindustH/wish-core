//! Explicit input-image codecs; never silently discard a reference image.
use super::ImageProtocol;
pub use crate::extensions::shared::images::{EditOverlay, ReferenceImage, ReferenceInput};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::Value;
impl ImageProtocol {
  fn capabilities(self, model: &str) -> crate::extensions::shared::images::ImageCapabilities {
    match self {
      Self::Openai => crate::extensions::openai::image::capabilities(model),
      Self::Xai => crate::extensions::xai::image::capabilities(model),
      Self::Bfl => crate::extensions::bfl::image::capabilities(model),
      Self::Wan => crate::extensions::alibaba::image::capabilities(model),
      Self::Runninghub => crate::extensions::runninghub::image::capabilities(model),
      Self::Novelai => crate::extensions::novelai::wire::capabilities(model),
      Self::Jimeng => crate::extensions::jimeng::image::capabilities(model),
      Self::Seedream => crate::extensions::seedance::image::capabilities(model),
      Self::Gemini => crate::extensions::google::image::capabilities(model),
      Self::GeminiInteractions => crate::extensions::google::interactions::capabilities(model),
      _ => Default::default(),
    }
  }
  pub fn request_options(self, model: &str) -> &'static [&'static str] {
    self.capabilities(model).options
  }
  pub fn accepts_options(self, model: &str, options: &serde_json::Map<String, Value>) -> bool {
    options.keys().all(|key| self.request_options(model).contains(&key.as_str()))
  }

  pub fn merge_options(
    self,
    model: &str,
    mut defaults: serde_json::Map<String, Value>,
    options: &serde_json::Map<String, Value>,
  ) -> Result<serde_json::Map<String, Value>, Error> {
    if !self.accepts_options(model, options) {
      return Err(Error::Build("selected image provider does not support these options".into()));
    }
    defaults = match self {
      Self::Bfl => super::bfl::merge_options(model, defaults, options)?,
      Self::Xai => super::xai::merge_options(defaults, options)?,
      Self::Gemini => super::gemini::merge_options(defaults, options)?,
      Self::Runninghub => super::runninghub::merge_options(defaults, options)?,
      _ => {
        defaults.extend(options.clone());
        defaults
      }
    };
    self.validate_edit_parameters(model, &defaults)?;
    Ok(defaults)
  }
  pub fn accepts_input(self, model: &str, value: &str) -> bool {
    let caps = self.capabilities(model);
    caps.reference_limit > 0
      && if value.starts_with("https://") { caps.remote_references } else { caps.local_references }
  }
  pub fn reference_images(self, model: &str) -> bool {
    self.reference_image_limit(model) > 0
  }
  pub fn reference_image_limit(self, model: &str) -> usize {
    self.capabilities(model).reference_limit
  }
  pub fn accepts_reference_count(self, model: &str, count: usize) -> bool {
    count <= self.reference_image_limit(model) && (!self.image_required(model) || count == 1)
  }
  pub fn validate_edit_parameters(
    self,
    model: &str,
    parameters: &serde_json::Map<String, Value>,
  ) -> Result<(), Error> {
    if self == Self::Novelai {
      crate::extensions::novelai::characters::validate(model, parameters)?;
    }
    if self == Self::Gemini {
      super::gemini::validate(model, parameters)?;
    }
    if self == Self::Xai {
      super::xai::validate(model, parameters)?;
    }
    if self == Self::Wan {
      super::wan::validate(model, parameters, false)?;
    }
    if self == Self::Bfl {
      super::bfl::validate(model, parameters)?;
    }
    if self == Self::Runninghub {
      super::runninghub::validate(model, parameters)?;
    }
    if self == Self::Openai {
      super::openai::validate_parameters(model, parameters)?;
    }
    Ok(())
  }
  pub fn mask_editing(self, model: &str) -> bool {
    let caps = self.capabilities(model);
    caps.reference_limit > 0 && caps.mask
  }
  pub fn markup_editing(self, model: &str) -> bool {
    self.capabilities(model).markup
  }
  pub fn image_required(self, model: &str) -> bool {
    self.capabilities(model).image_required
  }
  pub fn submission_with_images(
    self,
    model: &str,
    prompt: &str,
    parameters: &serde_json::Map<String, Value>,
    images: &[ReferenceInput],
    references: &[super::PreciseReference],
    overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    if overlay.is_some_and(|v| match v {
      EditOverlay::Mask(_) => !self.mask_editing(model),
      EditOverlay::Markup(_) => !self.markup_editing(model),
    }) {
      return Err(Error::Build("selected image model does not support a mask".into()));
    }
    if self.image_required(model) {
      if !references.is_empty() {
        return Err(Error::Build("Fill does not accept precise references".into()));
      }
      return super::bfl::fill(prompt, parameters, images, overlay.map(EditOverlay::image));
    }
    if !references.is_empty() {
      if !self.precise_references(model) || images.len() > 1 {
        return Err(Error::Build(
          "precise references require a NovelAI V4.5 model and at most one source image".into(),
        ));
      }
      let source = match images.first() {
        Some(ReferenceInput::Inline(image)) => Some(image),
        Some(_) => return Err(Error::Build("NovelAI requires a session image blob".into())),
        None => None,
      };
      return crate::extensions::novelai::wire::with_references(
        model, prompt, parameters, source, references,
      );
    }
    if images.is_empty() {
      return self.submission(model, prompt, parameters);
    }
    if !self.accepts_reference_count(model, images.len()) {
      return Err(Error::Build("image count is unsupported by this adapter; NovelAI image-to-image accepts exactly one source image".into()));
    }
    match self {
      Self::Wan => super::wan::submission(model, prompt, parameters, images),
      Self::Runninghub => super::runninghub::submission(model,prompt,parameters,images),
      Self::Xai => super::xai::submission(model, prompt, parameters, images),
      Self::Openai => super::openai::edit(model, prompt, parameters, images, overlay),
      Self::Jimeng => super::jimeng::submission(model, prompt, parameters, images),
      Self::Bfl => super::bfl::edit(model, prompt, parameters, images),
      Self::Novelai => match &images[0] {
        ReferenceInput::Inline(image) => crate::extensions::novelai::wire::edit(model, prompt, parameters, image),
        _ => Err(Error::Build("NovelAI requires a session image blob".into()))
      },
      Self::Seedream => crate::extensions::seedance::image::with_images(model,prompt,parameters,images),
      Self::GeminiInteractions => super::interactions::with_images(model,prompt,parameters,images),
      Self::Gemini => super::gemini::submission(model,prompt,parameters,images),
      _ => Err(Error::Build("this image adapter does not support reference images; select a provider with reference_images capability".into())),
    }
  }
}
