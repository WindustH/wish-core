//! One upstream, ready to call: a model-use protocol, a configured endpoint and a transport.
//!
//! This is the first layer where a [`Request`] becomes a network round trip, and deliberately only
//! that: render the body, resolve the path, build the call from the draft, execute one attempt,
//! then hand the reply either to the protocol's decoder or to its error envelope. When
//! `Request::stream` is true, the same call returns a stream: the same one attempt, its body read
//! record by record (SSE text framing, or Bedrock's binary event-stream frames holding the same
//! shape) and handed to the protocol's decoder, with nothing assembled on the way.
//!
//! The client owns the retry loop, [`retry`] over [`RetryPolicy`]: a transient failure is waited
//! out and the whole attempt replaced while the policy allows it. A streamed attempt is the opening
//! and its first event, so a streamed call is only replaced until that event is in hand - after
//! that a replay would splice a second copy of the answer into what the caller has already seen.
mod observer;
mod stream;

pub use observer::{AttemptObserver, AttemptObserverFactory};
pub use stream::EventStream;

use serde_json::Value;

use crate::executor::model::{CallResponse, ModelCaller};
use crate::protocol::account_state::{self, AccountState, AccountStateProtocol};
use crate::protocol::attempt::{Call, Reply, ReplyStream, Transport};
use crate::protocol::endpoint::{AuthScheme, CredentialRenewal, Credentials, Draft, Endpoint};
use crate::protocol::error::Error;
use crate::protocol::model_list::{self, ModelListPage, ModelListProtocol, ModelListQuery};
use crate::protocol::model_use::ModelUseProtocol;
use crate::protocol::upstream_compaction::request as upstream_compaction_wire;
use crate::protocol::upstream_compaction::{NO_COMPACTION_CALL, UpstreamCompactionProtocol};
use crate::protocol::{
  ContentBlock, Message, Request, Response, StreamAccumulator, TokenCount, TokenCountProtocol,
  UpstreamCompaction, UpstreamCompactionRequest, http_error, model_use::mode::ResponsesDeployment,
};
use crate::transport::{Framing, RecordStream};
use crate::utils::retry::{RetryDecision, RetryPolicy, retry};
use crate::utils::time::unix_seconds;

/// Request identifiers used by the official coding client for this provider preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeAgentIdentity {
  Codex,
  Claude,
  Copilot,
}

/// One model-use protocol bound to one configured endpoint, the read protocols named beside it,
/// and one transport.
///
/// Generic over the transport rather than holding a `dyn`: a trait method returning `impl Future`
/// is not object safe, and there is exactly one transport in flight per client anyway.
#[derive(Clone)]
pub struct Client<T> {
  attempt_observer: Option<AttemptObserverFactory>,
  code_agent_identity: Option<CodeAgentIdentity>,
  model_use: ModelUseProtocol,
  endpoint: Endpoint,
  credentials: Credentials,
  transport: T,
  retry: RetryPolicy,
  account_state: Option<AccountStateProtocol>,
  upstream_compaction: Option<UpstreamCompactionProtocol>,
  model_list: Option<ModelListProtocol>,
  token_count: Option<TokenCountProtocol>,
}

impl<T: Transport> Client<T> {
  /// Observe every physical stream attempt, including retries and streamed compaction.
  pub fn with_attempt_observer(mut self, observer: AttemptObserverFactory) -> Self {
    self.attempt_observer = Some(observer);
    self
  }
  pub fn get_model_use_protocol(&self) -> ModelUseProtocol {
    self.model_use
  }

  /// Builds a client from its required axes: the protocol to speak, the endpoint it is aimed at,
  /// and the transport to send by. Credentials and the read protocols each have a builder of their
  /// own; every call is retried by the default [`RetryPolicy`].
  pub fn new(model_use: ModelUseProtocol, endpoint: Endpoint, transport: T) -> Self {
    Self {
      attempt_observer: None,
      code_agent_identity: None,
      model_use,
      endpoint,
      credentials: Credentials::default(),
      transport,
      retry: RetryPolicy::default(),
      account_state: None,
      upstream_compaction: None,
      model_list: None,
      token_count: None,
    }
  }

  /// Apply the coding client's body identifiers to model requests for this provider.
  #[must_use]
  pub fn with_code_agent_identity(mut self, identity: CodeAgentIdentity) -> Self {
    self.code_agent_identity = Some(identity);
    self
  }

  /// The account's credentials, placed into every call this client makes.
  ///
  /// A client whose auth scheme names a credential the account does not carry fails the call when
  /// it is made: an empty credential is a configuration mistake, not a request sent
  /// half-addressed.
  #[must_use]
  pub fn with_credentials(mut self, credentials: Credentials) -> Self {
    self.credentials = credentials;
    self
  }

  /// Bind provider header templates to one conversation, including token count and compaction.
  #[must_use]
  pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
    self.endpoint = self.endpoint.with_session_id(session_id);
    self
  }

  /// Names the account-state protocol this upstream serves, chosen when the client is built.
  ///
  /// One axis, two readings: a protocol a reply carries fills [`Response::account_state`] and its
  /// streamed twin on every call, and a protocol served by a request of its own is read by
  /// [`Client::get_account_state`] and leaves replies alone. Asking a reply-borne protocol for a
  /// reading of its own still fails, with the reason its feature states.
  pub fn with_account_state(mut self, account_state: AccountStateProtocol) -> Self {
    self.account_state = Some(account_state);
    self
  }

  /// Names the compaction protocol this upstream serves, refused here when the pairing with the
  /// model-use protocol is not one this crate knows: an ask this wire cannot serve never reaches
  /// the network.
  ///
  /// Both kinds live on the Responses wire; the streamed one additionally asks a deployment that
  /// compacts on the call it always serves.
  ///
  /// # Errors
  ///
  /// [`Error::Unsupported`] when the chosen protocol cannot pair with the client's model-use
  /// protocol.
  pub fn with_upstream_compaction(
    mut self,
    compaction: UpstreamCompactionProtocol,
  ) -> Result<Self, Error> {
    let reason = match (compaction, self.model_use) {
      // The platform's own compaction endpoint; the Codex deployment serves only streamed calls.
      (UpstreamCompactionProtocol::OpenAiResponses, ModelUseProtocol::OpenAiResponses(mode))
        if mode.deployment != ResponsesDeployment::Codex =>
      {
        self.upstream_compaction = Some(compaction);
        return Ok(self);
      }
      (UpstreamCompactionProtocol::OpenAiResponses, ModelUseProtocol::OpenAiResponses(..)) => {
        "is not served by the Codex deployment, which compacts on its streamed call instead"
      }
      // A streamed compaction is answered by the Codex deployment, on the streamed call it always
      // serves: the trigger item it carries tells it apart, rather than an endpoint of its own.
      (
        UpstreamCompactionProtocol::OpenAiResponsesStreamed,
        ModelUseProtocol::OpenAiResponses(mode),
      ) if mode.deployment == ResponsesDeployment::Codex => {
        self.upstream_compaction = Some(compaction);
        return Ok(self);
      }
      (
        UpstreamCompactionProtocol::OpenAiResponsesStreamed,
        ModelUseProtocol::OpenAiResponses(..),
      ) => "is served only by a deployment that compacts on the call it always serves",
      _ => "is served only on the openai responses wire",
    };
    Err(Error::build_unsupported("upstream compaction", compaction.get_id(), reason))
  }

  /// Explicitly enable a token-count endpoint. Generation compatibility alone does not imply
  /// availability. Unsupported protocol/deployment pairings fail before any network request.
  pub fn with_token_count(mut self, protocol: TokenCountProtocol) -> Result<Self, Error> {
    protocol.validate_model_use(self.model_use)?;
    self.token_count = Some(protocol);
    Ok(self)
  }

  pub fn get_upstream_compaction_protocol(&self) -> Option<UpstreamCompactionProtocol> {
    self.upstream_compaction
  }

  pub fn get_token_count_protocol(&self) -> Option<TokenCountProtocol> {
    self.token_count
  }

  /// Count the input of a model request without generating output. Never falls back to an
  /// estimate. The provider count can differ from eventual usage and is not billed usage.
  pub async fn count_tokens(&self, request: &Request) -> Result<TokenCount, Error> {
    let protocol = self.token_count.ok_or_else(|| {
      Error::build_unsupported(
        "token counting",
        self.model_use.get_name(),
        "no token-count protocol configured",
      )
    })?;
    retry(&self.retry, |_| self.attempt_token_count(protocol, request), classify_retry).await
  }

  async fn attempt_token_count(
    &self,
    protocol: TokenCountProtocol,
    request: &Request,
  ) -> Result<TokenCount, Error> {
    let path = protocol.resolve_path(&self.endpoint.resolve_path(&request.model))?;
    let body = crate::protocol::token_count::request::render(protocol, self.model_use, request)?;
    let call = self.build_json_call(self.call_session_id(), path, &[], body)?;
    let (_, body) = self.read_reply(self.transport.execute(&call).await?)?;
    crate::protocol::token_count::response::decode(protocol, &body)
  }

  /// Names the model-list protocol this upstream serves, for [`Client::get_model_list`].
  #[must_use]
  pub fn with_model_list(mut self, model_list: ModelListProtocol) -> Self {
    self.model_list = Some(model_list);
    self
  }

  /// The API key or OAuth token the account is reached with, for a caller that reaches another
  /// service of the same account by it.
  #[must_use]
  pub fn get_api_key(&self) -> &str {
    &self.credentials.api_key
  }

  /// Exchanges the credentials this client holds for fresh ones, over the transport this client
  /// holds and by the renewal option the endpoint's bearer auth carries - every credential that
  /// renews is an access token read from `Authorization` - and hands the renewed credentials back,
  /// installed nowhere.
  ///
  /// What comes back is [`Credentials::renew`] over the exchange's answer: a new access token,
  /// the rotated refresh token (kept, when the endpoint rotated nothing), the account and expiry
  /// the exchange named, everything else as it was. Installing it - building the client again
  /// with it - and storing what the exchange rotated stay with whoever holds the account state.
  ///
  /// # Errors
  ///
  /// [`Error::Unsupported`] when the endpoint's auth carries no renewal - a key or a signature
  /// neither runs out nor exchanges - [`Error::Build`] when the credentials do not carry what
  /// their exchange needs, and the rest as that exchange states them.
  pub async fn refresh_credentials(&self) -> Result<Credentials, Error> {
    match self.endpoint.get_auth() {
      AuthScheme::Bearer(Some(CredentialRenewal::CodexOAuth)) => {
        let tokens =
          crate::protocol::endpoint::codex_oauth::refresh(&self.transport, &self.credentials)
            .await?;
        Ok(self.credentials.renew(&tokens))
      }
      AuthScheme::Bearer(Some(CredentialRenewal::CopilotToken)) => {
        let tokens =
          crate::protocol::endpoint::copilot_oauth::exchange(&self.transport, &self.credentials)
            .await?;
        Ok(self.credentials.renew(&tokens))
      }
      auth => Err(Error::build_unsupported(
        "credential refresh",
        auth.get_name(),
        "carries no renewal: a key or a signature neither runs out nor exchanges",
      )),
    }
  }

  /// Sends a request in the response mode it declares. Buffered calls retry whole attempts;
  /// streamed calls retry only until the first event, then hand ownership of the stream back.
  pub async fn call(
    &self,
    request: &Request,
  ) -> Result<CallResponse<EventStream<T::Stream>>, Error> {
    if request.stream {
      retry(&self.retry, |_| self.attempt_streamed_call(request), classify_retry)
        .await
        .map(CallResponse::Stream)
    } else {
      retry(&self.retry, |_| self.attempt_buffered_call(request), classify_retry)
        .await
        .map(|response| CallResponse::Complete(Box::new(response)))
    }
  }

  /// Asks the service to stand in for a conversation, replacing the attempt while the failure looks
  /// transient and attempts are left.
  ///
  /// The history handed over stays the caller's to keep; what comes back is the history to continue
  /// from - the service's own opaque item, ahead of whatever it handed back verbatim beside it. A
  /// protocol with no compaction call says so before anything is sent.
  ///
  /// On a deployment that answers a compaction on the call it always serves, a whole attempt is
  /// replaced rather than just its opening: nothing of a compaction reaches the caller before the
  /// item standing in for the history does, so a replay cannot splice anything into what they
  /// already have - it only asks the service to compact twice.
  pub async fn compact_upstream(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    retry(&self.retry, |_| self.attempt_upstream_compaction(request), classify_retry).await
  }

  /// Reads one page of the model list this upstream serves, over the protocol named when the
  /// client was built.
  ///
  /// # Errors
  ///
  /// [`Error::Unsupported`] when no model-list protocol was named for this client; the rest as
  /// [`mod@crate::protocol::model_list::fetch`] states them.
  pub async fn get_model_list(&self, query: &ModelListQuery) -> Result<ModelListPage, Error> {
    let protocol = self.model_list.ok_or_else(|| {
      Error::build_unsupported(
        "model list",
        self.model_use.get_name(),
        "was not named when this client was built",
      )
    })?;
    let headers = self.endpoint.get_headers();
    model_list::fetch(&self.transport, protocol, query, headers, &self.credentials, unix_seconds())
      .await
  }

  /// Reads the account state this upstream reports, over the protocol named when the client was
  /// built. `base_url` overrides where the ask goes, for a reading kept behind another door than
  /// the conversation target.
  ///
  /// # Errors
  ///
  /// [`Error::Unsupported`] when no account-state protocol was named, or when the one named has
  /// no ask of its own; the rest as [`mod@crate::protocol::account_state::fetch`] states them.
  pub async fn get_account_state(&self, base_url: Option<&str>) -> Result<AccountState, Error> {
    let protocol = self.account_state.ok_or_else(|| {
      Error::build_unsupported(
        "account state",
        self.model_use.get_name(),
        "was not named when this client was built",
      )
    })?;
    let headers = self.endpoint.get_headers();
    account_state::fetch(
      &self.transport,
      protocol,
      &self.credentials,
      base_url,
      headers,
      unix_seconds(),
    )
    .await
  }

  /// Searches the web as this client's account, for a search service that comes with the
  /// subscription it holds (see [`crate::protocol::web_search`]). The request goes out through
  /// this client's transport, with its current credentials, so a renewed token is used at once.
  pub async fn search(
    &self,
    protocol: crate::protocol::web_search::SearchProtocol,
    query: &crate::protocol::web_search::SearchQuery,
    base_url: Option<&str>,
    headers: &[(String, String)],
  ) -> Result<crate::protocol::web_search::SearchResults, Error> {
    crate::protocol::web_search::search(
      &self.transport,
      protocol,
      &self.credentials,
      base_url,
      headers,
      query,
      unix_seconds(),
    )
    .await
  }

  /// Internal: the call one attempt of [`Client::call`] sends: the rendered body with the coding
  /// client's identity applied, on the path the request resolves to.
  fn build_model_use_call(&self, request: &Request) -> Result<Call, Error> {
    let session_id = self.call_session_id();
    let mut body = self.model_use.render(request)?;
    self.apply_code_agent_identity(&mut body, &session_id);
    let path =
      self.model_use.resolve_request_path(request, self.endpoint.resolve_path(&request.model));
    self.build_json_call(session_id, path, &self.get_code_agent_headers(request), body)
  }

  /// Internal: the headers the coding client's identity puts on one model call, which follow what
  /// the call carries.
  fn get_code_agent_headers(&self, request: &Request) -> Vec<(&'static str, &'static str)> {
    let mut headers = Vec::new();
    if self.code_agent_identity == Some(CodeAgentIdentity::Copilot) {
      // Copilot bills the turns a person starts; the calls that carry a tool's result back are the
      // agent's own, as its editor marks them. What a tool hands the model to look at (an image it
      // read) rides as a user message that names the tool's call.
      let by_user = matches!(request.conversation.last(),
        Some(Message::User { metadata, .. }) if metadata.get("tool_call_id").is_none());
      headers.push(("x-initiator", if by_user { "user" } else { "agent" }));
      // A call that shows the model an image is asked for one that sees.
      if request.conversation.iter().any(|message| {
        matches!(message, Message::User { content, .. }
          if content.iter().any(|block| matches!(block, ContentBlock::Image { .. })))
      }) {
        headers.push(("copilot-vision-request", "true"));
      }
    }
    headers
  }

  /// The conversation this client's calls belong to, or a fresh id for a call that belongs to
  /// none.
  fn call_session_id(&self) -> String {
    self
      .endpoint
      .session_id()
      .map(str::to_owned)
      .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
  }

  /// Internal: a JSON `body` posted to `path` in the session `session_id`, with the protocol's
  /// headers and `extra` beside them, and the credentials placed.
  fn build_json_call(
    &self,
    session_id: String,
    path: String,
    extra: &[(&'static str, &'static str)],
    body: Value,
  ) -> Result<Call, Error> {
    let body = serde_json::to_vec(&body)
      .map_err(|error| Error::Build(format!("request body is not serializable: {error}")))?;
    self.endpoint.clone().with_session_id(session_id).build_call(
      Draft::post(Some(path), self.build_headers(extra), body),
      &self.credentials,
      unix_seconds(),
    )
  }

  fn apply_code_agent_identity(&self, body: &mut Value, session_id: &str) {
    let Some(object) = body.as_object_mut() else { return };
    match (self.code_agent_identity, self.model_use) {
      (Some(CodeAgentIdentity::Codex), ModelUseProtocol::OpenAiResponses(..)) => {
        object
          .entry("prompt_cache_key".to_owned())
          .or_insert_with(|| serde_json::json!(session_id));
        object.insert(
          "client_metadata".to_owned(),
          serde_json::json!({"session_id": session_id, "thread_id": session_id}),
        );
      }
      (Some(CodeAgentIdentity::Claude), ModelUseProtocol::AnthropicMessages(..)) => {
        object.insert(
          "metadata".to_owned(),
          serde_json::json!({"user_id": serde_json::json!({"session_id": session_id}).to_string()}),
        );
      }
      _ => {}
    }
  }

  /// Internal: one attempt of a buffered [`Client::call`].
  async fn attempt_buffered_call(&self, request: &Request) -> Result<Response, Error> {
    let call = self.build_model_use_call(request)?;
    let (headers, body) = self.read_reply(self.transport.execute(&call).await?)?;
    let mut response = self.model_use.decode(&body)?;
    response.account_state = self.parse_account_state_reading(&headers, Some(&body))?;
    Ok(response)
  }

  /// Internal: one attempt of a streamed [`Client::call`]: the stream opened, and its first event
  /// read ahead.
  ///
  /// Nothing is assembled here: [`EventStream::next`] hands over the protocol's events, and the
  /// `StreamAccumulator` is one consumer of them, not the only one. Until the first event is in
  /// hand the attempt delivered nothing, so it may still be replaced - when the failure is one the
  /// transport and the policy would replace at all: a connect failure or a head that never came,
  /// but not a failure after the service started answering. Once an event is in the caller's
  /// hands a failure is terminal.
  async fn attempt_streamed_call(
    &self,
    request: &Request,
  ) -> Result<EventStream<T::Stream>, Error> {
    let call = self.build_model_use_call(request)?;
    let framing = match self.model_use {
      ModelUseProtocol::BedrockConverse => Framing::AwsEventStream,
      _ => Framing::Sse,
    };
    let mut stream = self.open_event_stream(&call, &request.model, framing).await?;
    stream.read_ahead().await?;
    Ok(stream)
  }

  /// Internal: the call a compaction is asked with, built the one way both lanes build it: the wire's own
  /// path or its compacting suffix, the rendered trigger body, and the headers this wire adds
  /// beside what all of its calls carry. Refused here when this client's wire has no compaction
  /// call, before anything is sent.
  fn build_upstream_compaction_call(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<Call, Error> {
    let (mode, path) = match (self.model_use, self.upstream_compaction) {
      // A deployment that compacts on its ordinary call keeps that call's path.
      (
        ModelUseProtocol::OpenAiResponses(mode),
        Some(UpstreamCompactionProtocol::OpenAiResponsesStreamed),
      ) => (mode, self.endpoint.resolve_path(&request.model)),
      (
        ModelUseProtocol::OpenAiResponses(mode),
        Some(UpstreamCompactionProtocol::OpenAiResponses),
      ) => (mode, format!("{}/compact", self.endpoint.resolve_path(&request.model))),
      _ => {
        return Err(Error::build_unsupported(
          "upstream compaction",
          self.model_use.get_name(),
          NO_COMPACTION_CALL,
        ));
      }
    };
    let mut body = upstream_compaction_wire::openai_responses::render(request, mode)?;
    let session_id = self.call_session_id();
    if mode.deployment == ResponsesDeployment::Codex {
      self.apply_code_agent_identity(&mut body, &session_id);
    }
    let extra = upstream_compaction_wire::openai_responses::get_extra_headers(mode);
    self.build_json_call(session_id, path, extra, body)
  }

  /// Internal: one attempt of [`Client::compact_upstream`]: render, resolve, build, execute, then read or report.
  async fn attempt_upstream_compaction(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    if matches!(self.upstream_compaction, Some(UpstreamCompactionProtocol::OpenAiResponsesStreamed))
    {
      return self.attempt_streamed_compaction(request).await;
    }
    let call = self.build_upstream_compaction_call(request)?;
    let (headers, body) = self.read_reply(self.transport.execute(&call).await?)?;
    // The call was built, so this wire is the responses one: the decode has no refusal of its
    // own to make.
    let mut compaction =
      crate::protocol::upstream_compaction::response::openai_responses::decode(&body)?;
    compaction.account_state = self.parse_account_state_reading(&headers, Some(&body))?;
    Ok(compaction)
  }

  /// Internal: one attempt of a compaction on a deployment that answers it on the call it always
  /// serves: the trigger item in the body is what asks for the compaction, and the item standing
  /// in for the history arrives on the stream like any other item.
  async fn attempt_streamed_compaction(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    let call = self.build_upstream_compaction_call(request)?;
    // The responses wire is the only one with this call, and it is served over SSE.
    let mut stream = self.open_event_stream(&call, &request.model, Framing::Sse).await?;
    let mut accumulator = StreamAccumulator::new().with_account_state(stream.account_state.clone());
    while let Some(event) = stream.next().await? {
      accumulator.feed(event)?;
    }
    decode_streamed_upstream_compaction(accumulator.finish()?)
  }

  /// Internal: the account reading this reply carries, when the endpoint named an account protocol
  /// that replies carry. One read by a request of its own finds nothing here.
  ///
  /// `body` is `None` on the streamed path, where only the reply head is in hand: a protocol that
  /// needs the payload reports [`Error::Build`] there instead of quietly finding nothing.
  fn parse_account_state_reading(
    &self,
    headers: &[(String, String)],
    body: Option<&Value>,
  ) -> Result<Option<AccountState>, Error> {
    match self.account_state {
      Some(protocol) if protocol.is_passive() => {
        account_state::parse_reply(protocol, headers, body).map(Some)
      }
      _ => Ok(None),
    }
  }

  /// Internal: executes a streamed call and reads its head. A non-`2xx` reply never becomes a
  /// stream: its bounded body is mapped exactly like the buffered path's. A served one is handed
  /// back as events decoded from records in `framing`, with the account reading its head carried
  /// and an observer made for `model`.
  async fn open_event_stream(
    &self,
    call: &Call,
    model: &str,
    framing: Framing,
  ) -> Result<EventStream<T::Stream>, Error> {
    let observer = self.attempt_observer.as_ref().map(|factory| factory(model));
    let mut reply = self.transport.execute_stream(call).await?;
    if !reply.is_success() {
      let body = reply.read_error_body().await;
      return Err(
        self
          .decode_upstream_error(reply.get_status(), &body)
          .with_retry_after(reply.get_retry_after_ms()),
      );
    }
    let account_state = self.parse_account_state_reading(reply.get_headers(), None)?;
    Ok(EventStream::new(RecordStream::new(reply, framing), self.model_use, observer, account_state))
  }

  /// Internal: a buffered reply's headers and JSON body, or the failure its protocol's envelope
  /// reads out of a non-`2xx` one.
  fn read_reply(&self, reply: Reply) -> Result<(Vec<(String, String)>, Value), Error> {
    let reply = reply.require_success(|status, body| self.decode_upstream_error(status, body))?;
    let body = http_error::decode_json_body("2xx", &reply.body)?;
    Ok((reply.headers, body))
  }

  fn build_headers(&self, extra: &[(&'static str, &'static str)]) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> =
      vec![("content-type".to_owned(), "application/json".to_owned())];
    headers.extend(
      self
        .model_use
        .get_headers()
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
    );
    headers.extend(extra.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())));
    headers
  }

  fn decode_upstream_error(&self, status: u16, body: &[u8]) -> Error {
    http_error::decode_error_reply(status, body, |status, body| {
      self.model_use.decode_http_error(status, body)
    })
  }
}

/// Internal: the history a streamed compaction stands in for: what the stream handed over, exactly one item
/// of which has to be the `compaction` item holding it.
///
/// A reply without that item - or with more than one - is a failure rather than an empty
/// compaction: the history it stands in for would otherwise look complete while what it stood for
/// was gone. The streamed path reports no warnings, because its decoder tolerates what it cannot
/// represent.
fn decode_streamed_upstream_compaction(response: Response) -> Result<UpstreamCompaction, Error> {
  let found = response
    .messages
    .iter()
    .filter(|message| matches!(message, Message::UpstreamCompaction { .. }))
    .count();
  if found != 1 {
    return Err(Error::Malformed(format!(
      "a compaction reply carries exactly one `compaction` item, this one carried {found}"
    )));
  }
  Ok(UpstreamCompaction {
    conversation: response.messages,
    usage: response.usage,
    account_state: response.account_state,
    warnings: Vec::new(),
  })
}

// Protocol retryability belongs to the client; utils does not interpret network errors.
fn classify_retry(error: &Error) -> RetryDecision {
  if error.is_retryable() {
    RetryDecision::Retry { minimum_delay_ms: error.get_retry_after_ms() }
  } else {
    RetryDecision::Stop
  }
}

impl<T: Transport + Sync> ModelCaller for Client<T> {
  fn supports_upstream_compaction(&self) -> bool {
    self.get_upstream_compaction_protocol().is_some()
  }
  async fn compact_upstream(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    Client::compact_upstream(self, request).await
  }
  async fn count_tokens(&self, request: &Request) -> Result<Option<TokenCount>, Error> {
    if self.get_token_count_protocol().is_some() {
      Client::count_tokens(self, request).await.map(Some)
    } else {
      Ok(None)
    }
  }
  fn get_model_use_protocol(&self) -> Option<ModelUseProtocol> {
    Some(Client::get_model_use_protocol(self))
  }
  type Stream = EventStream<T::Stream>;
  async fn call(&self, request: &Request) -> Result<CallResponse<Self::Stream>, Error> {
    Client::call(self, request).await
  }
}
