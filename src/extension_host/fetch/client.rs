//! Account-independent read execution for this extension.
use super::{FetchProtocol, FetchRequest, FetchResult};
use crate::{
  client::RetryOperation,
  extensions::shared::read::ReadClient,
  protocol::{attempt::Transport, error::Error},
};
impl<T: Transport> ReadClient<T> {
  pub async fn fetch(
    &self,
    protocol: FetchProtocol,
    request: &FetchRequest,
    base_url: Option<&str>,
    headers: &[(String, String)],
  ) -> Result<FetchResult, Error> {
    self
      .execute(RetryOperation::WebFetch, |transport, credentials| {
        super::fetch(
          transport,
          protocol,
          credentials,
          base_url,
          headers,
          request,
          crate::utils::time::unix_seconds(),
        )
      })
      .await
  }
}
