//! What one account is reached with.
//!
//! [`Credentials`] is the caller's own half of a call: the key and whatever else an account is
//! addressed by. Where a wire expects the key is not the credentials' business - that is a fact
//! about the wire, stated as an [`AuthScheme`](super::AuthScheme) on the endpoint, and the two are
//! joined when the call is built, which is also the only place that knows both.
//!
//! What a wire asks for is not always the key: a console endpoint reads a workspace from its own
//! header, and one with no key form at all reads the session a browser holds. Each piece has a
//! name ([`CredentialField`]), which is also the name configuration and a path placeholder write
//! it with, so a wire can say what it needs without carrying it.
//!
//! Credentials are also exchanged for credentials: [`codex_oauth`](super::codex_oauth) is the one
//! request that takes a credential and answers with a new [`Tokens`] set, for the subscription
//! that hands out tokens rather than keys. And the AWS credentials are here because they are an
//! account's own: a region, a key pair, a session token, with the mechanism that proves them
//! beside them. `sigv4` signs a request with them at the moment the request is built; Aliyun's POP
//! signature stays absent until one of these endpoints needs it.

/// What one account is reached with.
///
/// Everything past the key is optional because most services address an account by its key alone; a
/// source that does need more says so when its call is built, rather than defaulting to a guess.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credentials {
  /// The API key or OAuth token that authenticates the account.
  pub api_key: String,
  /// The workspace an account's quota is read from, where a service addresses it that way.
  pub workspace_id: Option<String>,
  /// The team an account belongs to, where a service bills or limits per team.
  pub team_id: Option<String>,
  /// The organization inside a team, where a service names both.
  pub organization: Option<String>,
  /// The project inside an organization, where a service names both.
  pub project: Option<String>,
  /// The account a subscription is addressed by, where a service asks for one beside the token
  /// (Codex's `chatgpt-account-id`).
  pub account_id: Option<String>,
  /// The token a subscription's next access token is exchanged for, where an account is served by a
  /// token endpoint rather than a long-lived key. It is rotated by a refresh, so the caller stores
  /// what the exchange handed back.
  pub refresh_token: Option<String>,
  /// When the access token stops being accepted, when the key is one: read off the pair a refresh
  /// handed back, by whoever applied that pair to these credentials. Read by
  /// [`Endpoint::build_call`](super::Endpoint::build_call), which is where spent credentials are
  /// refused before anything is sent.
  pub expires_at: Option<u64>,
  /// The region an AWS account is served from, which its signature's scope names and a read's
  /// path may address.
  pub region: Option<String>,
  /// The key id of an AWS account, which its signature travels inside.
  pub access_key_id: Option<String>,
  /// The secret an AWS account signs with. It signs; it never travels.
  pub secret_access_key: Option<String>,
  /// The session token temporary AWS credentials carry.
  pub session_token: Option<String>,
}

impl Credentials {
  /// Credentials that are only a key, which is what most services need.
  #[allow(dead_code)] // wish-test
  pub fn from_api_key(api_key: impl Into<String>) -> Self {
    Self { api_key: api_key.into(), ..Self::default() }
  }

  /// The credentials with one token exchange applied: the access token in place of the old, the
  /// rotated refresh token in place of the one spent (kept when the endpoint rotated nothing),
  /// and the account id and expiry the exchange named. Everything the exchange did not name
  /// stays as it was - this renews credentials, it does not rebuild an account.
  #[must_use]
  pub fn renew(&self, tokens: &Tokens) -> Self {
    Self {
      api_key: tokens.access_token.clone(),
      refresh_token: tokens.refresh_token.clone().or_else(|| self.refresh_token.clone()),
      account_id: tokens.account_id.clone().or_else(|| self.account_id.clone()),
      expires_at: tokens.expires_at,
      ..self.clone()
    }
  }

  /// One of the account's own fields, absent when it was not given or is empty: an empty value is
  /// a configuration mistake, not a request to send half-addressed.
  pub(crate) fn get_field(&self, field: CredentialField) -> Option<&str> {
    let value = match field {
      CredentialField::ApiKey => Some(self.api_key.as_str()),
      CredentialField::WorkspaceId => self.workspace_id.as_deref(),
      CredentialField::TeamId => self.team_id.as_deref(),
      CredentialField::Organization => self.organization.as_deref(),
      CredentialField::Project => self.project.as_deref(),
      CredentialField::AccountId => self.account_id.as_deref(),
      CredentialField::Region => self.region.as_deref(),
      CredentialField::AccessKeyId => self.access_key_id.as_deref(),
      CredentialField::SecretAccessKey => self.secret_access_key.as_deref(),
      CredentialField::SessionToken => self.session_token.as_deref(),
    };
    value.filter(|value| !value.is_empty())
  }

  /// Sets one of the account's own fields, as configuration names it.
  pub(crate) fn set_field(&mut self, field: CredentialField, value: String) {
    match field {
      CredentialField::ApiKey => self.api_key = value,
      CredentialField::WorkspaceId => self.workspace_id = Some(value),
      CredentialField::TeamId => self.team_id = Some(value),
      CredentialField::Organization => self.organization = Some(value),
      CredentialField::Project => self.project = Some(value),
      CredentialField::AccountId => self.account_id = Some(value),
      CredentialField::Region => self.region = Some(value),
      CredentialField::AccessKeyId => self.access_key_id = Some(value),
      CredentialField::SecretAccessKey => self.secret_access_key = Some(value),
      CredentialField::SessionToken => self.session_token = Some(value),
    }
  }
}

/// What a token endpoint hands back: the credentials a subscription's calls are made with.
///
/// The access token authenticates the calls, the refresh token is what the next exchange takes, and
/// the id token is where a subscription claims who it belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
  /// The token a call is authenticated with.
  pub access_token: String,
  /// The token the next refresh exchanges, when the endpoint rotated it.
  pub refresh_token: Option<String>,
  /// The identity token, kept as it arrived: this crate reads what it claims, and never verifies
  /// it, because the service that signed it is the one that checks it.
  pub id_token: Option<String>,
  /// The account a subscription is addressed by: the endpoint's own field when it names one, the id
  /// token's claim otherwise.
  pub account_id: Option<String>,
  /// When the access token stops being accepted, from its own `exp` claim.
  pub expires_at: Option<u64>,
}

/// One piece of an account's credentials, named the way a wire and a caller write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialField {
  /// The API key or OAuth token.
  ApiKey,
  /// The workspace an account's quota is read from.
  WorkspaceId,
  /// The team an account is billed or limited by.
  TeamId,
  /// The organization inside a team.
  Organization,
  /// The project inside an organization.
  Project,
  /// The account of a subscription that is addressed beside its token.
  AccountId,
  /// The region an AWS account is served from.
  Region,
  /// The key id of an AWS account.
  AccessKeyId,
  /// The secret an AWS account signs with.
  SecretAccessKey,
  /// The session token temporary AWS credentials carry.
  SessionToken,
}

impl CredentialField {
  /// Every field, in declaration order.
  const ALL: [CredentialField; 10] = [
    CredentialField::ApiKey,
    CredentialField::WorkspaceId,
    CredentialField::TeamId,
    CredentialField::Organization,
    CredentialField::Project,
    CredentialField::AccountId,
    CredentialField::Region,
    CredentialField::AccessKeyId,
    CredentialField::SecretAccessKey,
    CredentialField::SessionToken,
  ];

  /// The name this field is written as, in a configuration and in a path placeholder.
  pub(crate) fn get_name(self) -> &'static str {
    match self {
      CredentialField::ApiKey => "api_key",
      CredentialField::WorkspaceId => "workspace_id",
      CredentialField::TeamId => "team_id",
      CredentialField::Organization => "organization",
      CredentialField::Project => "project",
      CredentialField::AccountId => "account_id",
      CredentialField::Region => "region",
      CredentialField::AccessKeyId => "access_key_id",
      CredentialField::SecretAccessKey => "secret_access_key",
      CredentialField::SessionToken => "session_token",
    }
  }

  /// The field a name written by [`CredentialField::get_name`] names.
  pub(crate) fn from_name(name: &str) -> Option<Self> {
    Self::ALL.into_iter().find(|field| field.get_name() == name)
  }

  /// The field a `{placeholder}` in an endpoint's address names.
  ///
  /// A region may address a path, because it is where a service lives rather than something
  /// secret; an AWS key never does, because a credential in a URL is a credential in every log line
  /// that ever touches that URL.
  pub(crate) fn from_placeholder(name: &str) -> Option<Self> {
    Self::from_name(name).filter(|field| {
      !matches!(
        field,
        CredentialField::AccessKeyId
          | CredentialField::SecretAccessKey
          | CredentialField::SessionToken
      )
    })
  }
}
