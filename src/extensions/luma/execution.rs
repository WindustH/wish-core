//! Native tool execution retains supplier request types and owns preparation.
use crate::extensions::media_runtime::MediaRequest;
use serde_json::Value;

impl crate::extensions::invocation::Executable for super::video_tool::Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      let request = *self;

      let options = match serde_json::to_value(request.parameters) {
        Ok(Value::Object(options)) => options,
        Ok(_) => {
          return Err(crate::extensions::invocation::Failure::Rejected(
            "invalid extension parameter object".into(),
          ));
        }
        Err(error) => return Err(error.to_string().into()),
      };
      let request = MediaRequest {
        action: Some(request.action),
        provider: request.service,
        prompt: request.prompt,
        job_id: request.job_id,
        options,
        mode: request.mode,
        inputs: request.inputs,
        modify_mode: request.modify_mode,
        ..Default::default()
      };

      host.media(request).await
    })
  }
}
