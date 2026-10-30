//! One paid submission followed by read-only queries. No account registry or tool dependency.
use super::{ImageProtocol, ImageReply, ImageSource};
use crate::{
  extensions::shared::polling,
  protocol::{
    attempt::Transport,
    endpoint::{Credentials, Draft},
  },
  transport::ReqwestTransport,
};
use serde_json::{Map, Value};
use std::{
  sync::atomic::{AtomicBool, Ordering},
  time::Duration,
};

pub(crate) struct Execution<'a> {
  pub protocol: ImageProtocol,
  pub model: &'a str,
  pub base_url: &'a str,
  pub path: &'a str,
  pub credentials: &'a Credentials,
  pub parameters: &'a Map<String, Value>,
  pub user_agent: &'a str,
}
impl Execution<'_> {
  pub async fn run(
    &self,
    transport: &ReqwestTransport,
    draft: Draft,
    submitted: &AtomicBool,
  ) -> Result<Vec<ImageSource>, String> {
    let mut initial = self
      .protocol
      .call(self.base_url, self.path, self.credentials, draft)
      .map_err(|e| e.to_string())?;
    // Protocol identities can be required upstream; the application identity is only a fallback.
    if !initial.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("user-agent")) {
      initial.headers.push(("user-agent".into(), self.user_agent.into()));
    }
    let mut call = initial.clone();
    let mut result_url = None;
    let mut task_id = None;
    let mut querying = false;
    let mut poll_delay = 2u64;
    loop {
      // Mark before awaiting: cancellation cannot misclassify a potentially paid attempt.
      submitted.store(true, Ordering::Relaxed);
      let reply = if querying {
        polling::execute(transport, &call).await?
      } else {
        transport.execute(&call).await.map_err(|_| "Image request interrupted; upstream outcome may be unknown. Do not automatically submit it again.".to_string())?
      };
      let body = crate::protocol::http_error::read_provider_json(reply, "image_generation")
        .map_err(|e| e.to_string())?;
      match self
        .protocol
        .decode(&body, &call.url, result_url.as_deref())
        .map_err(|e| e.to_string())?
      {
        ImageReply::Complete(images) => return Ok(images),
        ImageReply::PendingTask { task_id: next } => {
          querying = true;
          if next.is_some() {
            task_id = next;
          }
          let task = task_id.as_deref().ok_or("image response did not identify a task")?;
          let draft = self
            .protocol
            .task_status(self.model, task, self.parameters)
            .map_err(|e| e.to_string())?;
          tokio::time::sleep(Duration::from_secs(poll_delay)).await;
          poll_delay = (poll_delay * 2).min(15);
          call = self
            .protocol
            .call(self.base_url, self.path, self.credentials, draft)
            .map_err(|e| e.to_string())?;
        }
        ImageReply::Pending { url, result_url: next } => {
          querying = true;
          call = super::poll_call(self.protocol, &initial, &url).map_err(|e| e.to_string())?;
          result_url = next;
          tokio::time::sleep(Duration::from_secs(poll_delay)).await;
          poll_delay = (poll_delay * 2).min(15);
        }
      }
    }
  }
}
