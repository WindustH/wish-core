impl crate::extensions::invocation::Executable for super::Generate {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      host
        .image(crate::extensions::image_runtime::ImageRequest {
          prompt: self.prompt,
          provider: self.service,
          images: self.reference_images,
          options: self.parameters,
          mask: None,
          markup: None,
          references: Vec::new(),
        })
        .await
    })
  }
}
impl crate::extensions::invocation::Executable for super::Fill {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      host
        .image(crate::extensions::image_runtime::ImageRequest {
          prompt: self.prompt,
          provider: self.service,
          images: vec![self.source_image],
          options: self.parameters,
          mask: self.mask,
          markup: None,
          references: Vec::new(),
        })
        .await
    })
  }
}
