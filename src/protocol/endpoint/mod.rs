//! The one place a payload becomes a call: what a tree says, aimed and proven.
//!
//! Every request this crate makes is the same product in two halves. The payload trees -
//! [`model_use`](crate::protocol::model_use), [`token_count`](crate::protocol::token_count),
//! [`upstream_compaction`](crate::protocol::upstream_compaction),
//! [`account_state`](crate::protocol::account_state), [`model_list`](crate::protocol::model_list)
//! and [`web_search`](crate::protocol::web_search) - render what the wire says and hand it over as a
//! [`Draft`]: method, path, query, headers, body, everything the wire sees in the request and
//! nothing the caller does. This module owns the other half, which comes from configuration rather
//! than from a protocol: an [`Endpoint`] states where a draft goes and how its calls are proven,
//! [`Credentials`] carry what the account is reached with, and [`Endpoint::build_call`] joins the
//! two into the one [`Call`] a transport performs. It builds the call and sends nothing.
//!
//! The join has its order, and the order is the module: a path's placeholders are filled from the
//! account's own credentials (`{workspace_id}`, `{region}`), headers merge static-then-auth-then-wire
//! so a protocol can always override what configuration set, and AWS credentials sign the call
//! last - a signature covers the request exactly as it stands on the wire, so it is computed after
//! everything else is in place, and the secret never travels: it only derives the signature.
//!
//! Deliberately absent: credentials inside a URL, because a credential in a URL is a credential in
//! every log line that ever touches it. Credentials may be closer to their end than the call they
//! are asked for: [`Endpoint::build_call`] refuses expired ones before anything is sent, and
//! [`codex_oauth`] beside it is the exchange the auth scheme's renewal option names - deciding when
//! to refresh, and storing what the exchange rotated, stays with the caller, because this crate
//! holds no account state. The read-side trees state their endpoints as static rows instead of
//! configuration; `source.rs` is the shape of one row, and what turns it into an [`Endpoint`].

mod credentials;

pub mod codex_oauth;
pub use credentials::{CredentialField, Credentials, Tokens};
pub(crate) mod sigv4;
mod source;

use serde_json::Value;

use crate::protocol::attempt::{Call, Method};
use crate::protocol::error::Error;
use sigv4::SigV4Credentials;
pub(crate) use source::{Source, SourceHeader};

/// How a target's calls are proven: where the credential sits, and - when it is the kind that
/// runs out - how it renews.
///
/// Placement is stated, never guessed from the protocol: whoever builds the target (a preset, a
/// config file) decides whether the credential is a bearer token, one named header, or an AWS
/// signature. That keeps protocol quirks such as `x-api-key` versus `x-goog-api-key` out of the
/// assembly code, and keeps every secret out of configuration - the value arrives with
/// [`Credentials`] when the call is built. The renewal, where the placed credential is the renewing
/// kind, rides with the bearer placement as the option it carries: every credential that renews is
/// an access token read from `Authorization`, which is the one placement that takes the option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthScheme {
  /// No credential: the target answers anyone.
  None,
  /// `Authorization: Bearer <token>`, with the account's key as the token. The option names how
  /// that token renews when it is a subscription's rather than a key: `None` is a credential that
  /// never runs out.
  Bearer(Option<CredentialRenewal>),
  /// The account's key in one named header (`x-api-key`, `x-goog-api-key`). No renewal option:
  /// every credential read from a header is a key, and a key never runs out.
  Header(&'static str),
  /// AWS SigV4: the call is signed with the account's own AWS credentials rather than carrying a
  /// key. The region, key id and secret arrive with [`Credentials`], and the signature is
  /// computed at the moment the call is built, over the request exactly as it stands. A
  /// signature is derived per call and never renews, so there is no option to carry.
  SigV4,
}

/// How a renewing credential renews, as the option its [`AuthScheme::Bearer`] placement carries.
///
/// A fact of where the credential came from, paired with where it sits rather than an axis named on
/// its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialRenewal {
  /// A Codex subscription's refresh token, exchanged at its issuer's token endpoint
  /// ([`codex_oauth`]): everything the exchange needs is in the credentials themselves.
  CodexOAuth,
}

impl AuthScheme {
  /// The name this scheme is written as in an error, when an ask cannot be served of it.
  pub(crate) fn get_name(self) -> &'static str {
    match self {
      AuthScheme::None => "none",
      AuthScheme::Bearer(_) => "bearer",
      AuthScheme::Header(..) => "header",
      AuthScheme::SigV4 => "sigv4",
    }
  }
}

/// What one of the payload trees says: a request already rendered, aimed at nothing yet.
///
/// This is the intermediate form between a tree and this module. The path a tree resolved itself -
/// a conversation path names its model - overrides the endpoint's own; `None` uses the endpoint's
/// path. Query parameters travel in the order the wire wants them, because a cursor is an opaque
/// token that may carry anything, and are percent-encoded on the way out.
#[derive(Clone, Debug)]
pub struct Draft {
  /// The method the wire is asked with; a read is a `GET`, a call is a `POST`.
  pub method: Method,
  /// The request path, when the tree resolved it; `None` uses the endpoint's own.
  pub path: Option<String>,
  /// Query parameters in wire order.
  pub query: Vec<(String, String)>,
  /// The headers this wire's requests carry, besides what configuration and the auth scheme place.
  pub headers: Vec<(String, String)>,
  /// The body as rendered; empty for a read.
  pub body: Vec<u8>,
}

impl Draft {
  /// A read: a `GET` on the endpoint's own path, its parameters in the query string, no body.
  pub fn get(query: Vec<(String, String)>) -> Self {
    Self { method: Method::Get, path: None, query, headers: Vec::new(), body: Vec::new() }
  }

  /// A `POST` of a body already serialized, on `path` or the endpoint's own; `headers` name the
  /// body's type among whatever else the wire wants.
  pub fn post(path: Option<String>, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
    Self { method: Method::Post, path, query: Vec::new(), headers, body }
  }

  /// A `POST` of a JSON body on the endpoint's own path, the shape most services are asked in.
  ///
  /// # Errors
  ///
  /// [`Error::Build`] when the body cannot be serialized.
  pub fn post_json(body: &Value) -> Result<Self, Error> {
    let body = serde_json::to_vec(body)
      .map_err(|error| Error::Build(format!("request body is not serializable: {error}")))?;
    Ok(Self::post(None, vec![("content-type".to_owned(), "application/json".to_owned())], body))
  }

  /// A `POST` of an HTML form, already encoded, on the endpoint's own path.
  pub fn post_form(form: &str) -> Self {
    Self::post(
      None,
      vec![("content-type".to_owned(), "application/x-www-form-urlencoded".to_owned())],
      form.as_bytes().to_vec(),
    )
  }
}

/// One configured target: where a draft goes, and how its calls are proven.
#[derive(Clone, Debug)]
pub struct Endpoint {
  base_url: String,
  path: String,
  headers: Vec<(String, String)>,
  credential_headers: Vec<(&'static str, CredentialField)>,
  auth: AuthScheme,
  session_id: Option<String>,
}

impl Endpoint {
  /// Builds a target from configuration-shaped values.
  ///
  /// `base_url` must be an absolute `http(s)` URL and `path` a non-empty request path. No
  /// credential travels here: the auth scheme names where one sits, the value arrives with
  /// [`Credentials`] when a call is built, and an empty credential is a configuration mistake that
  /// fails that call rather than a request sent half-addressed.
  pub fn new(base_url: &str, path: &str, auth: AuthScheme) -> Result<Self, Error> {
    let base_url = base_url.trim();
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
      return Err(Error::Build(format!(
        "outbound base URL `{base_url}` is not an absolute http(s) URL"
      )));
    }
    if path.is_empty() {
      return Err(Error::Build("outbound path is empty".to_owned()));
    }
    Ok(Self {
      base_url: base_url.to_owned(),
      path: path.to_owned(),
      headers: Vec::new(),
      credential_headers: Vec::new(),
      auth,
      session_id: None,
    })
  }

  /// Adds one static header. A repeated name replaces the earlier value, case-insensitively.
  pub fn with_header(mut self, name: &str, value: &str) -> Self {
    insert_header(&mut self.headers, name, value);
    self
  }

  /// Places one of the account's credentials in a named header, when a call is built.
  pub fn with_credential_header(mut self, name: &'static str, credential: CredentialField) -> Self {
    self.credential_headers.push((name, credential));
    self
  }

  /// Binds `{session}` header templates to a conversation. Without one, each standalone call
  /// gets a fresh ID, shared by all template headers on that call.
  pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
    self.session_id = Some(session_id.into());
    self
  }

  /// The conversation identity bound by the server, when this target has one.
  pub fn session_id(&self) -> Option<&str> {
    self.session_id.as_deref()
  }

  /// The auth scheme this target proves its calls by, for the asks that read it back - the
  /// client's credential renewal among them.
  pub fn get_auth(&self) -> AuthScheme {
    self.auth
  }

  /// The configured path with `{model}` substituted, percent-encoded. A path without the
  /// placeholder comes back unchanged, which is the common case: only protocols that address the
  /// model in the URL (Gemini, Bedrock) need it. Verb changes on top of this (`:generateContent`
  /// for a stream) are the tree's business, not the target's. A model id containing `/` is escaped
  /// as `%2F`; a protocol whose path expects real separators inside the model id would need a rule
  /// of its own here.
  pub fn resolve_path(&self, model: &str) -> String {
    self.path.replace("{model}", &percent_encode(model))
  }

  /// Joins one draft, this target and the account's credentials into the call a transport
  /// performs, refusing credentials that have already run out before anything is sent.
  ///
  /// The path comes from the draft when it named one and from the endpoint otherwise, then any
  /// `{field}` placeholder it still carries is filled from the credentials. A credential the auth
  /// scheme or the endpoint's own headers name and the account does not carry fails here, before
  /// anything is sent: a request sent half-addressed is worse than a request not sent. So do
  /// credentials past their own `expires_at`, against the `now` the caller read - with
  /// [`Error::Renewal`], because sending them would only learn the same thing from the service,
  /// slower.
  ///
  /// The renewal exchanges are exempt by construction: they carry no credentials of the account
  /// they renew, so there is nothing here to refuse.
  pub fn build_call(
    &self,
    draft: Draft,
    credentials: &Credentials,
    now: u64,
  ) -> Result<Call, Error> {
    if let Some(expires_at) = credentials.expires_at
      && now >= expires_at
    {
      return Err(Error::Renewal { expires_at });
    }
    let url = self.render_url(&draft, credentials)?;
    let mut headers = self.render_headers(&draft, credentials)?;
    if let AuthScheme::SigV4 = self.auth {
      sign_sigv4(&mut headers, &url, &draft, credentials)?;
    }
    Ok(Call { method: draft.method, url, headers, body: draft.body })
  }

  /// The absolute URL of a draft: the base URL and the path with their placeholders filled, then
  /// the query in wire order.
  fn render_url(&self, draft: &Draft, credentials: &Credentials) -> Result<String, Error> {
    let base_url = fill_placeholders(&self.base_url, credentials)?;
    let path = fill_placeholders(draft.path.as_deref().unwrap_or(&self.path), credentials)?;
    let mut url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let mut separator = '?';
    for (name, value) in &draft.query {
      url.push(separator);
      url.push_str(name);
      url.push('=');
      url.push_str(&percent_encode(value));
      separator = '&';
    }
    Ok(url)
  }

  /// The headers of a draft, merged in the order that lets a protocol override configuration: the
  /// static ones with `{session}` expanded, then the auth scheme's, then the credential headers,
  /// then the draft's own.
  fn render_headers(
    &self,
    draft: &Draft,
    credentials: &Credentials,
  ) -> Result<Vec<(String, String)>, Error> {
    let mut headers = self.headers.clone();
    if headers.iter().any(|(_, value)| value.contains("{session}")) {
      let fallback = self.session_id.is_none().then(|| uuid::Uuid::new_v4().to_string());
      let session = self.session_id.as_deref().or(fallback.as_deref()).unwrap();
      for (_, value) in &mut headers {
        *value = value.replace("{session}", session);
      }
    }
    match self.auth {
      AuthScheme::None => {}
      AuthScheme::Bearer(_) => {
        let key = require_field(credentials, CredentialField::ApiKey)?;
        insert_header(&mut headers, "authorization", &format!("Bearer {key}"));
      }
      AuthScheme::Header(name) => {
        let key = require_field(credentials, CredentialField::ApiKey)?;
        insert_header(&mut headers, name, key);
      }
      // The signed headers are written after everything else, out of the account's own
      // credentials.
      AuthScheme::SigV4 => {}
    }
    for (name, credential) in &self.credential_headers {
      insert_header(&mut headers, name, require_field(credentials, *credential)?);
    }
    for (name, value) in &draft.headers {
      insert_header(&mut headers, name, value);
    }
    Ok(headers)
  }
}

/// Signs a finished call with the account's AWS credentials and writes the headers the signature
/// travels in.
///
/// Bedrock is the one service this crate signs for: its Converse calls and its control plane carry
/// the same service in their scope, over the URL exactly as rendered.
fn sign_sigv4(
  headers: &mut Vec<(String, String)>,
  url: &str,
  draft: &Draft,
  credentials: &Credentials,
) -> Result<(), Error> {
  let signed = sigv4::sign(
    url,
    draft.method.as_str(),
    "bedrock",
    &draft.body,
    &SigV4Credentials {
      region: require_field(credentials, CredentialField::Region)?,
      access_key_id: require_field(credentials, CredentialField::AccessKeyId)?,
      secret_access_key: require_field(credentials, CredentialField::SecretAccessKey)?,
      // The token is the one piece the credentials may carry or not: a permanent key has none,
      // and asking for one would refuse the accounts that never had it.
      session_token: credentials.get_field(CredentialField::SessionToken),
    },
  )
  .map_err(|reason| Error::Build(format!("the call could not be signed: {reason}")))?;
  insert_header(headers, "x-amz-date", &signed.x_amz_date);
  if let Some(token) = &signed.x_amz_security_token {
    insert_header(headers, "x-amz-security-token", token);
  }
  insert_header(headers, "authorization", &signed.authorization);
  Ok(())
}

fn require_field(credentials: &Credentials, field: CredentialField) -> Result<&str, Error> {
  credentials
    .get_field(field)
    .ok_or_else(|| Error::Build(format!("this call needs the `{}` credential", field.get_name())))
}

/// Fills the `{field}` placeholders a base URL or path may carry from the account's own
/// credentials, such as the region in a Bedrock host or the workspace in a Qwen one.
///
/// A placeholder is percent-encoded like any other path segment, and one this table does not know
/// is refused: an address nobody can fill is a bug in the target, not something to send as it stands.
/// A `{model}` placeholder is the tree's to fill, before the draft is made - the model is request
/// knowledge, the fields here are account knowledge.
fn fill_placeholders(path: &str, credentials: &Credentials) -> Result<String, Error> {
  let mut filled = String::with_capacity(path.len());
  let mut rest = path;
  while let Some(start) = rest.find('{') {
    filled.push_str(&rest[..start]);
    let Some(end) = rest[start..].find('}') else {
      return Err(Error::Build("the target's address has an unclosed placeholder".to_owned()));
    };
    let name = &rest[start + 1..start + end];
    let field = CredentialField::from_placeholder(name).ok_or_else(|| {
      Error::Build(format!("the target's address reads from the unknown field `{name}`"))
    })?;
    filled.push_str(&percent_encode(require_field(credentials, field)?));
    rest = &rest[start + end + 1..];
  }
  filled.push_str(rest);
  Ok(filled)
}

/// Sets one header, replacing an earlier entry with the same name (case-insensitive).
///
/// Replacement rather than append: `Call.headers` reaches the client through `HeaderMap::append`,
/// so a name listed twice goes out twice, and a duplicated `content-type` is server-side confusion
/// nobody benefits from. Wire headers merge last, so a protocol can always override what
/// configuration set.
fn insert_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
  if let Some(slot) = headers.iter_mut().find(|(existing, _)| existing.eq_ignore_ascii_case(name)) {
    slot.1 = value.to_owned();
    return;
  }
  headers.push((name.to_owned(), value.to_owned()));
}

/// Escapes everything outside the unreserved set, so a value can sit inside one path segment.
fn percent_encode(value: &str) -> String {
  let mut out = String::with_capacity(value.len());
  for byte in value.bytes() {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
        out.push(char::from(byte));
      }
      _ => out.push_str(&format!("%{byte:02X}")),
    }
  }
  out
}
