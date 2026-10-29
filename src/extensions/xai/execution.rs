//! Native tool execution retains supplier request types and owns preparation.
use super::video_operation::Operation;
use crate::extensions::media_runtime::{MediaKind, MediaRequest};
use serde_json::Value;

impl crate::extensions::invocation::Executable for super::speech::Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      let request = *self;

      use crate::extensions::xai::speech::Request;
      let request = match request {
        Request::Run(args) => {
          let options = args.parameters();
          MediaRequest {
            action: Some("run".into()),
            kind: Some(MediaKind::Speech),
            prompt: Some(args.text),
            provider: args.service,
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

impl crate::extensions::invocation::Executable for super::video::Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      let request = *self;

      let request = match request.prepare() {
        Operation::Submit(submission) => {
          let options = match serde_json::to_value(submission.options) {
            Ok(Value::Object(options)) => options,
            Ok(_) => {
              return Err(crate::extensions::invocation::Failure::Rejected(
                "invalid xAI video options".into(),
              ));
            }
            Err(error) => return Err(error.to_string().into()),
          };
          MediaRequest {
            action: Some("run".into()),
            prompt: Some(submission.prompt),
            provider: submission.service,
            mode: Some(submission.mode),
            inputs: submission.inputs,
            options,
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
