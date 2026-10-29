//! An account client carries credentials and renewal policy without a model protocol.
use crate::protocol::{
  attempt::Transport,
  endpoint::{CredentialRenewal, Credentials},
  error::Error,
};
#[derive(Clone)]
pub struct AccountClient<T> {
  transport: T,
  credentials: Credentials,
  renewal: Option<CredentialRenewal>,
}
impl<T: Transport> AccountClient<T> {
  pub fn new(transport: T, credentials: Credentials, renewal: Option<CredentialRenewal>) -> Self {
    Self { transport, credentials, renewal }
  }
  pub fn get_credentials(&self) -> &Credentials {
    &self.credentials
  }
  pub fn get_api_key(&self) -> &str {
    &self.credentials.api_key
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
