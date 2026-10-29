//! Native tool execution retains supplier request types and owns preparation.
use crate::extensions::media_runtime::{MediaKind, MediaRequest};
use serde_json::Value;
fn parameters(value: impl serde::Serialize) -> Result<serde_json::Map<String, Value>, String> {
  match serde_json::to_value(value) {
    Ok(Value::Object(v)) => Ok(v),
    Ok(_) => Err("expected parameter object".into()),
    Err(e) => Err(e.to_string()),
  }
}
impl crate::extensions::invocation::Executable for super::speech_tool::Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      let request = *self;

      use crate::extensions::openai::speech_tool::Request;
      let request = match request {
        Request::Run { text, service, parameters: options } => {
          let options = match parameters(options) {
            Ok(options) => options,
            Err(error) => return Err(error.into()),
          };
          MediaRequest {
            action: Some("run".into()),
            kind: Some(MediaKind::Speech),
            prompt: Some(text),
            provider: service,
            options,
            ..Default::default()
          }
        }
        Request::Status { job_id } => {
          MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
        }
        Request::ListJobs {} => {
          MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
        }
      };

      host.media(request).await
    })
  }
}
