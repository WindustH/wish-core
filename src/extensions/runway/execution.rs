//! Native tool execution retains supplier request types and owns preparation.
use super::operation::Operation;
use crate::extensions::media_runtime::MediaRequest;
use serde_json::Value;

impl crate::extensions::invocation::Executable for super::Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      let request = *self;

      let operation = match request.prepare() {
        Ok(operation) => operation,
        Err(error) => return Err(error.into()),
      };
      let request = match operation {
        Operation::Submit(submission) => {
          let options = match serde_json::to_value(submission.options) {
            Ok(Value::Object(options)) => options,
            Ok(_) => {
              return Err(crate::extensions::invocation::Failure::Rejected(
                "invalid Runway options".into(),
              ));
            }
            Err(error) => return Err(error.to_string().into()),
          };
          MediaRequest {
            action: Some("run".into()),
            provider: submission.service,
            prompt: submission.prompt,
            mode: Some(submission.mode),
            inputs: submission.inputs,
            options,
            plan: Some(std::sync::Arc::new(submission.plan)),
            ..Default::default()
          }
        }
        Operation::Status(job_id) => {
          MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
        }
        Operation::ListJobs => {
          MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
        }
      };

      host.media(request).await
    })
  }
}
