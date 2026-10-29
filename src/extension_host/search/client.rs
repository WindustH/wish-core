//! Account-independent read execution for this extension.
use super::{SearchProtocol, SearchQuery, SearchResults};
use crate::{
  client::RetryOperation,
  extensions::shared::read::ReadClient,
  protocol::{attempt::Transport, error::Error},
};
impl<T: Transport> ReadClient<T> {
  pub async fn search(
    &self,
    protocol: SearchProtocol,
    request: &SearchQuery,
    base_url: Option<&str>,
    headers: &[(String, String)],
  ) -> Result<SearchResults, Error> {
    self
      .execute(RetryOperation::WebSearch, |transport, credentials| {
        super::search(
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
