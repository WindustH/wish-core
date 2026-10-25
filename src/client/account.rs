//! An account client carries credentials and renewal policy without a model protocol.
use super::{
  RetryObserver, RetryOperation,
  retry::{classify_read_retry, retry_upstream},
};
use crate::protocol::{
  attempt::Transport,
  endpoint::{CredentialRenewal, Credentials},
  error::Error,
};
use crate::utils::{retry::RetryPolicy, time::unix_seconds};
#[derive(Clone)]
pub struct AccountClient<T> {
  transport: T,
  credentials: Credentials,
  renewal: Option<CredentialRenewal>,
  auxiliary_retry: RetryPolicy,
  retry_observer: Option<RetryObserver>,
}
impl<T: Transport> AccountClient<T> {
  pub fn new(transport: T, credentials: Credentials, renewal: Option<CredentialRenewal>) -> Self {
    Self {
      transport,
      credentials,
      renewal,
      auxiliary_retry: RetryPolicy::auxiliary(),
      retry_observer: None,
    }
  }
  pub fn get_credentials(&self) -> &Credentials {
    &self.credentials
  }
  pub fn get_api_key(&self) -> &str {
    &self.credentials.api_key
  }
  pub fn with_transport(mut self, transport: T) -> Self {
    self.transport = transport;
    self
  }
  pub fn with_retry_observer(mut self, observer: RetryObserver) -> Self {
    self.retry_observer = Some(observer);
    self
  }
  pub async fn search(
    &self,
    protocol: crate::protocol::web_search::SearchProtocol,
    query: &crate::protocol::web_search::SearchQuery,
    base_url: Option<&str>,
    headers: &[(String, String)],
  ) -> Result<crate::protocol::web_search::SearchResults, Error> {
    retry_upstream(
      &self.auxiliary_retry,
      RetryOperation::WebSearch,
      self.retry_observer.clone(),
      |_| async {
        tokio::time::timeout(
          std::time::Duration::from_secs(60),
          crate::protocol::web_search::search(
            &self.transport,
            protocol,
            &self.credentials,
            base_url,
            headers,
            query,
            unix_seconds(),
          ),
        )
        .await
        .unwrap_or_else(|_| {
          Err(Error::Transport(crate::protocol::error::TransportFailure {
            retryable: false,
            message: "web search: no answer within 60 seconds".into(),
          }))
        })
      },
      classify_read_retry,
    )
    .await
  }
  pub async fn fetch(
    &self,
    protocol: crate::protocol::web_fetch::FetchProtocol,
    query: &crate::protocol::web_fetch::FetchRequest,
    base_url: Option<&str>,
    headers: &[(String, String)],
  ) -> Result<crate::protocol::web_fetch::FetchResult, Error> {
    retry_upstream(
      &self.auxiliary_retry,
      RetryOperation::WebFetch,
      self.retry_observer.clone(),
      |_| async {
        tokio::time::timeout(
          std::time::Duration::from_secs(60),
          crate::protocol::web_fetch::fetch(
            &self.transport,
            protocol,
            &self.credentials,
            base_url,
            headers,
            query,
            unix_seconds(),
          ),
        )
        .await
        .unwrap_or_else(|_| {
          Err(Error::Transport(crate::protocol::error::TransportFailure {
            retryable: false,
            message: "web fetch: no answer within 60 seconds".into(),
          }))
        })
      },
      classify_read_retry,
    )
    .await
  }
  pub async fn refresh_credentials(&self) -> Result<Credentials, Error> {
    match self.renewal {
      Some(CredentialRenewal::CodexOAuth) => {
        let tokens =
          crate::protocol::endpoint::codex_oauth::refresh(&self.transport, &self.credentials)
            .await?;
        Ok(self.credentials.renew(&tokens))
      }
      Some(CredentialRenewal::CopilotToken) => {
        let tokens =
          crate::protocol::endpoint::copilot_oauth::exchange(&self.transport, &self.credentials)
            .await?;
        Ok(self.credentials.renew(&tokens))
      }
      None => Err(Error::build_unsupported(
        "credential refresh",
        "account",
        "carries no renewal: a key or a signature neither runs out nor exchanges",
      )),
    }
  }
}
