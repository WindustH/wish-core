//! The one place a payload becomes a call: what a tree says, aimed and proven.
//!
//! Every request this crate makes is the same product in two halves. The payload trees -
//! [`model_use`](crate::protocol::model_use), [`compaction`](crate::protocol::upstream_compaction),
//! [`account`](crate::protocol::account_state), [`model_list`](crate::protocol::model_list) - render
//! what the wire says and hand it over as a [`Draft`]: method, path, query, headers, body,
//! everything the wire sees in the request and nothing the caller does. This module owns the
//! other half, which comes from configuration rather than from a protocol: [`Outbound`] states
//! where a draft goes and how its calls are proven, [`Credentials`] carry the account's material,
//! and [`Outbound::dispatch`] joins the two into the one [`Call`] a transport performs.
//!
//! The join has its order, and the order is the module: a path's placeholders are filled from the
//! account's own material (`{workspace_id}`, `{region}`), headers merge static-then-auth-then-wire
//! so a protocol can always override what configuration set, and AWS material signs the call last -
//! a signature covers the request exactly as it stands on the wire, so it is computed after
//! everything else is in place, and the secret never travels: it only derives the signature.
//!
//! Deliberately absent: credentials inside a URL, because a credential in a URL is a credential
//! in every log line that ever touches it. Material may be closer to its end than the call it is
//! asked for: [`expired`] judges that off the credentials alone, [`Outbound::dispatch`] refuses
//! by it before anything is sent, and [`oauth`] beside it is the exchange the auth protocol's
//! renewal option names - deciding when to refresh, and storing what the exchange rotated,
//! stays with the caller, because this crate holds no account state. Google ADC sits
//! in the same place for Vertex: [`adc`] parses the credential file and runs the exchange that
//! turns it into a caller's material.

mod credentials;

pub mod adc;
pub use credentials::{CredentialField, Credentials, Tokens};
pub mod oauth;
pub(crate) mod sigv4;
mod table;

use crate::protocol::error::Error;
use crate::protocol::wire::{Call, Method};
use sigv4::{SigV4Credentials, sign};
pub(crate) use table::{Source, SourceHeader};

/// How a target's calls are proven: where the credential sits, and - when it is the kind that
/// runs out - how it renews.
///
/// Placement is stated, never guessed from the protocol: whoever builds the target (a preset, a
/// config file) decides whether the credential is a bearer token, one named header, or an AWS
/// signature. That keeps protocol quirks such as `x-api-key` versus
/// `x-goog-api-key` out of the assembly code, and keeps every secret out of configuration - the
/// value arrives with [`Credentials`] when the call is dispatched. The renewal, where the placed
/// credential is the renewing kind, rides with the bearer placement as the option it carries -
/// every credential that renews (a subscription's token pair, an ADC grant) is an access token
/// read from `Authorization`, which is the one placement that takes the option. The way
/// material renews comes with where it came from, which is why it is an option here and not a
/// second axis to pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthProtocol {
  /// No credential: the target answers anyone.
  None,
  /// `Authorization: Bearer <token>`, with the account's key as the token. The option names how
  /// that token renews when it is a subscription's or a grant's rather than a key: `None` is
  /// material that never runs out.
  Bearer(Option<CredentialsRefreshProtocol>),
  /// The account's key in one named header (`x-api-key`, `x-goog-api-key`). No renewal option:
  /// every credential read from a header is a key, and a key never runs out.
  Header(&'static str),
  /// AWS SigV4: the call is signed with the account's own AWS material rather than carrying a
  /// key. The region, key id and secret arrive with [`Credentials`], and the signature is
  /// computed at the moment the call is built, over the request exactly as it stands. A
  /// signature is derived per call and never renews, so there is no option to carry.
  SigV4,
}

/// How renewing material renews, as the option its [`AuthProtocol`] placement carries.
///
/// A fact of where the material came from, paired with where it sits rather than an axis named
/// on its own: an OAuth refresh token is self-describing, an ADC grant needs the file it was
/// parsed from, and a key needs nothing because it never runs out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialsRefreshProtocol {
  /// A subscription's refresh token, exchanged at its issuer's token endpoint
  /// ([`oauth`]): everything the exchange needs is the material itself.
  OAuth,
  /// A Google ADC grant, renewed by the exchange [`adc`] runs over the file's own fields.
  GoogleAdc {
    /// The parsed credential file: what the exchange is made of.
    adc: adc::Adc,
    /// The scope a service account asks for; a login's grant was made at login and ignores
    /// this.
    scope: String,
  },
}

impl AuthProtocol {
  /// The name this protocol is written as in an error, when an ask cannot be served of it.
  pub(crate) fn name(&self) -> &'static str {
    match self {
      AuthProtocol::None => "none",
      AuthProtocol::Bearer(_) => "bearer",
      AuthProtocol::Header(..) => "header",
      AuthProtocol::SigV4 => "sigv4",
    }
  }
}

/// What one of the payload trees says: a request already rendered, aimed at nothing yet.
///
/// This is the intermediate form between a tree and this module. The path a tree resolved itself -
/// a conversation path names its model - overrides the target's own template; `None` uses the
/// template as the path. Query parameters travel in the order the wire wants them, because a
/// cursor is an opaque token that may carry anything, and are percent-encoded on the way out.
#[derive(Clone, Debug)]
pub struct Draft {
  /// The method the wire is asked with; a read is a `GET`, a call is a `POST`.
  pub method: Method,
  /// The request path, when the tree resolved it; `None` uses the target's own template.
  pub path: Option<String>,
  /// Query parameters in wire order.
  pub query: Vec<(String, String)>,
  /// The headers this wire's requests carry, besides what configuration and the plan place.
  pub headers: Vec<(String, String)>,
  /// The body as rendered; empty for a read.
  pub body: Vec<u8>,
}

/// One configured outbound target: where a draft goes, and how its calls are proven.
#[derive(Clone, Debug)]
pub struct Outbound {
  base_url: String,
  path: Option<String>,
  headers: Vec<(String, String)>,
  material_headers: Vec<(&'static str, CredentialField)>,
  auth: AuthProtocol,
}

impl Outbound {
  /// Builds a target from configuration-shaped values.
  ///
  /// `base_url` must be an absolute `http(s)` URL and `path` a non-empty request path. No
  /// credential travels here: the plan names where one sits, the value arrives with
  /// [`Credentials`] when a call is dispatched, and an empty credential is a configuration
  /// mistake that fails that call rather than a request sent half-addressed.
  pub fn new(base_url: &str, path: &str, auth: AuthProtocol) -> Result<Self, Error> {
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
      path: Some(path.to_owned()),
      headers: Vec::new(),
      material_headers: Vec::new(),
      auth,
    })
  }

  /// Adds one static header. A repeated name replaces the earlier value, case-insensitively.
  pub fn with_header(mut self, name: &str, value: &str) -> Self {
    insert_header(&mut self.headers, name, value);
    self
  }

  /// The auth protocol this target proves its calls by, for the asks that read it back - the
  /// client's credential renewal among them.
  pub fn auth(&self) -> &AuthProtocol {
    &self.auth
  }

  /// The configured path template with `{model}` substituted, percent-encoded. A template without
  /// the placeholder comes back unchanged, which is the common case: only protocols that address
  /// the model in the URL (Gemini, Bedrock) need it. Verb changes on top of this
  /// (`:generateContent` for a stream) are the tree's business, not the target's. A model id
  /// containing `/` is escaped as `%2F`; a protocol whose path expects real separators inside the
  /// model id would need a rule of its own here.
  pub fn resolve_path(&self, model: &str) -> String {
    self.path.as_deref().unwrap_or_default().replace("{model}", &percent_encode(model))
  }

  /// Joins one draft, this target and the account's material into the call a transport performs,
  /// refusing material that has already run out before anything is sent.
  ///
  /// The path comes from the draft when it named one and from the template otherwise, then any
  /// `{field}` placeholder it still carries is filled from the material. A material the plan or
  /// the target's own headers name and the account does not carry fails here, before anything is
  /// sent: a request sent half-addressed is worse than a request not sent. So does material past
  /// its own `expires_at`, against the `now` the caller read - with [`Error::Renewal`], because
  /// sending it would only learn the same thing from the service, slower.
  ///
  /// The renewal exchanges are exempt by construction: they dispatch no material of the account
  /// they renew, so there is nothing here to refuse.
  pub fn dispatch(&self, draft: Draft, material: &Credentials, now: u64) -> Result<Call, Error> {
    if let Some(expires_at) = material.expires_at
      && now >= expires_at
    {
      return Err(Error::Renewal { expires_at });
    }
    let template = draft.path.as_deref().or(self.path.as_deref()).ok_or_else(|| {
      Error::Build(
        "this call needs a path, and neither its draft nor its target names one".to_owned(),
      )
    })?;
    let path = fill_path(template, material)?;
    let mut url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
    let mut separator = '?';
    for (name, value) in &draft.query {
      url.push(separator);
      url.push_str(name);
      url.push('=');
      url.push_str(&percent_encode(value));
      separator = '&';
    }
    let mut headers = self.headers.clone();
    match &self.auth {
      AuthProtocol::None => {}
      AuthProtocol::Bearer(_) => {
        let key = field(material, CredentialField::ApiKey)?;
        insert_header(&mut headers, "authorization", &format!("Bearer {key}"));
      }
      AuthProtocol::Header(name) => {
        let key = field(material, CredentialField::ApiKey)?;
        insert_header(&mut headers, name, key);
      }
      // The signed headers are written after everything else, out of the account's own material.
      AuthProtocol::SigV4 => {}
    }
    for (name, credential) in &self.material_headers {
      insert_header(&mut headers, name, field(material, *credential)?);
    }
    for (name, value) in &draft.headers {
      insert_header(&mut headers, name, value);
    }
    if let AuthProtocol::SigV4 = self.auth {
      // Bedrock is the one service this crate signs for: its Converse calls and its control plane
      // carry the same service in their scope, over the URL exactly as rendered above.
      let signed = sign(
        &url,
        draft.method.as_str(),
        "bedrock",
        &draft.body,
        &SigV4Credentials {
          region: field(material, CredentialField::Region)?,
          access_key_id: field(material, CredentialField::AccessKeyId)?,
          secret_access_key: field(material, CredentialField::SecretAccessKey)?,
          // The token is the one piece the material may carry or not: a permanent key has none,
          // and asking for one would refuse the accounts that never had it.
          session_token: material.field(CredentialField::SessionToken),
        },
      )
      .map_err(|reason| Error::Build(format!("the call could not be signed: {reason}")))?;
      insert_header(&mut headers, "x-amz-date", &signed.x_amz_date);
      if let Some(token) = &signed.x_amz_security_token {
        insert_header(&mut headers, "x-amz-security-token", token);
      }
      insert_header(&mut headers, "authorization", &signed.authorization);
    }
    Ok(Call { method: draft.method, url, headers, body: draft.body })
  }
}

/// Whether the account's material has stopped being accepted, against the `now` the caller reads.
///
/// Judgment, not reporting: it answers one question - is a renewal due before the next call - and
/// it is what `Client::credentials_expired` reads. Dispatch enforces the same expiry on its own,
/// refusing a call over material this returns `true` for.
pub fn expired(material: &Credentials, now: u64) -> bool {
  material.expires_at.is_some_and(|expires_at| now >= expires_at)
}

fn field(material: &Credentials, credential: CredentialField) -> Result<&str, Error> {
  material
    .field(credential)
    .ok_or_else(|| Error::Build(format!("this call needs the `{}` credential", credential.name())))
}

/// Fills the `{field}` placeholders a path may carry from the account's own material.
///
/// A placeholder is percent-encoded like any other path segment, and one this table does not know
/// is refused: a path nobody can fill is a bug in the target, not something to send as it stands.
/// A `{model}` placeholder is the tree's to fill, before the draft is made - the model is request
/// knowledge, the fields here are account knowledge.
fn fill_path(path: &str, material: &Credentials) -> Result<String, Error> {
  let mut filled = String::with_capacity(path.len());
  let mut rest = path;
  while let Some(start) = rest.find('{') {
    filled.push_str(&rest[..start]);
    let Some(end) = rest[start..].find('}') else {
      return Err(Error::Build("the target's path has an unclosed placeholder".to_owned()));
    };
    let name = &rest[start + 1..start + end];
    let credential = CredentialField::from_placeholder(name).ok_or_else(|| {
      Error::Build(format!("the target's path reads from the unknown field `{name}`"))
    })?;
    filled.push_str(&percent_encode(field(material, credential)?));
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
