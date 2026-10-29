impl crate::extensions::invocation::Executable for super::Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      host
        .image(crate::extensions::image_runtime::ImageRequest {
          prompt: self.prompt,
          provider: self.service,
          images: self.source_image.into_iter().collect(),
          options: self.parameters,
          mask: None,
          markup: None,
          references: self
            .references
            .into_iter()
            .map(|r| crate::extensions::image_runtime::PreciseReferenceRequest {
              image: r.image,
              kind: r.kind,
              strength: r.strength,
              fidelity: r.fidelity,
            })
            .collect(),
        })
        .await
    })
  }
}
