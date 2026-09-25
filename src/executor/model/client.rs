//! One upstream, ready to call: a model-use protocol, a configured outbound target and a transport.
//!
//! This is the first layer where a [`Request`] becomes a network round trip, and deliberately only
//! that: render the body, resolve the path, dispatch the draft, execute one attempt, then hand the
//! reply either to the protocol's decoder or to its error envelope. When `Request::stream` is true, the same call returns a stream: the
//! same one attempt, its body read record by record (SSE text framing, or bedrock's binary
//! event-stream frames holding the same shape) and handed to the protocol's decoder, with nothing
//! assembled on the way. Both call lanes own the retry loop: a transient failure is waited out
//! and the whole attempt replaced while [`RetryPolicy`] allows it, and a streamed call only retries
//! until its first event is in hand - after that a replay would splice a second copy of the answer
//! into what the caller has already seen.

use std::collections::VecDeque;
use std::time::Duration;

use serde_json::Value;

use crate::protocol::account_state::{self, AccountState, AccountStateProtocol};
use crate::protocol::error::Error;
use crate::protocol::model_list::{self, ModelCatalog, ModelListProtocol, ModelListQuery};
use crate::protocol::model_use::ModelUseProtocol;
use crate::protocol::model_use::stream::StreamDecoder;
use crate::protocol::outbound::{
  AuthProtocol, Credentials, CredentialsRefreshProtocol, Draft, Outbound,
};
use crate::protocol::upstream_compaction::request as upstream_compaction_wire;
use crate::protocol::upstream_compaction::{
  Unsupported as CompactionUnsupported, UpstreamCompactionProtocol,
};

use crate::protocol::wire::Call;
use crate::protocol::wire::Method;
use crate::protocol::wire::Reply;
use crate::protocol::wire::ReplyStream;
use crate::protocol::wire::Transport;
use crate::protocol::{
  Message, Request, Response, StreamAccumulator, StreamEvent, UpstreamCompaction,
  UpstreamCompactionRequest, http_error, model_use::request::openai_responses::ResponsesDeployment,
};
use crate::transport::{BedrockStream, SseEvent, SseStream};
use crate::utils::retry::{RetryDecision, RetryPolicy, retry};

/// The response mode selected by `Request::stream`.
pub enum CallResponse<S> {
  Complete(Box<Response>),
  Stream(S),
}

/// Request identifiers used by the official coding client for this provider preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeAgentIdentity {
  Codex,
  Claude,
}

/// One model-use protocol bound to one configured endpoint, the read protocols named beside it,
/// and one transport.
///
/// Generic over the transport rather than holding a `dyn`: a trait method returning `impl Future`
/// is not object safe, and there is exactly one transport in flight per client anyway.
#[derive(Clone)]
pub struct Client<T> {
  stream_observer: Option<super::StreamObserverFactory>,
  code_agent_identity: Option<CodeAgentIdentity>,
  model_use: ModelUseProtocol,
  outbound: Outbound,
  credentials: Credentials,
  transport: T,
  retry: RetryPolicy,
  /// The clock this client reads, as unix seconds; the machine's when the caller gave none. The
  /// expiry judgment reads it, at every dispatch.
  clock: Option<std::sync::Arc<dyn Fn() -> u64 + Send + Sync>>,
  account_state: Option<AccountStateProtocol>,
  upstream_compaction: Option<UpstreamCompactionProtocol>,
  model_list: Option<ModelListProtocol>,
  token_count: Option<crate::protocol::TokenCountProtocol>,
}

impl<T: Transport> Client<T> {
  /// Observe every physical stream attempt, including retries and streamed compaction.
  pub fn with_stream_observer(mut self, observer: super::StreamObserverFactory) -> Self {
    self.stream_observer = Some(observer);
    self
  }
  pub fn get_model_use_protocol(&self) -> ModelUseProtocol {
    self.model_use
  }

  /// Builds a client from its required axes: the protocol to speak, the outbound target, and the
  /// transport to send by. Credentials, the retry policy, the clock and the read protocols each
  /// have a builder of their own.
  pub fn new(model_use: ModelUseProtocol, outbound: Outbound, transport: T) -> Self {
    Self {
      stream_observer: None,
      code_agent_identity: None,
      model_use,
      outbound,
      credentials: Credentials::default(),
      transport,
      retry: RetryPolicy::default(),
      clock: None,
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

  /// The account's material, placed into every call this client makes.
  ///
  /// A client whose plan names a credential and whose material carries none fails the call when
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
    self.outbound = self.outbound.with_session_id(session_id);
    self
  }

  /// Names the account-state protocol this upstream serves, chosen when the client is built.
  ///
  /// One axis, two readings: a protocol a reply carries fills [`Response::account_state`] and its
  /// streamed twin on every call, and a protocol served by a request of its own is read by
  /// [`Client::get_account_state`]. Naming a protocol the other side of that divide still fails the
  /// ask that cannot be served, with the reason its feature states.
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
      (UpstreamCompactionProtocol::OpenAiResponses, ModelUseProtocol::OpenAiResponses(..)) => {
        self.upstream_compaction = Some(compaction);
        return Ok(self);
      }
      // A streamed compaction is answered by the Codex deployment, on the streamed call it always
      // serves: the trigger item it carries tells it apart, rather than an endpoint of its own.
      (
        UpstreamCompactionProtocol::OpenAiResponsesStreamed,
        ModelUseProtocol::OpenAiResponses(variant),
      ) if variant.deployment == ResponsesDeployment::Codex => {
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
  pub fn with_token_count(
    mut self,
    protocol: crate::protocol::TokenCountProtocol,
  ) -> Result<Self, Error> {
    protocol.validate_model_use(self.model_use)?;
    self.token_count = Some(protocol);
    Ok(self)
  }

  pub fn get_upstream_compaction_protocol(&self) -> Option<UpstreamCompactionProtocol> {
    self.upstream_compaction
  }

  pub fn get_token_count_protocol(&self) -> Option<crate::protocol::TokenCountProtocol> {
    self.token_count
  }

  /// Count the input of a model request without generating output. Never falls back to an
  /// estimate. The provider count can differ from eventual usage and is not billed usage.
  pub async fn count_tokens(
    &self,
    request: &Request,
  ) -> Result<crate::protocol::TokenCount, Error> {
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
    protocol: crate::protocol::TokenCountProtocol,
    request: &Request,
  ) -> Result<crate::protocol::TokenCount, Error> {
    let path = protocol.resolve_path(&self.outbound.resolve_path(&request.model))?;
    let body = Self::serialize_body(crate::protocol::token_count::request::render(
      protocol,
      self.model_use,
      request,
    )?)?;
    let call = self.outbound.dispatch(
      self.build_draft(path, self.build_headers(&[]), body),
      &self.credentials,
      self.get_current_time(),
    )?;
    let reply = self.transport.execute(&call).await?;
    if !reply.is_success() {
      return Err(
        self
          .decode_upstream_error(reply.status, &reply.body)
          .with_retry_after(reply.get_retry_after_ms()),
      );
    }
    crate::protocol::token_count::response::decode(protocol, &Self::decode_reply_body(&reply)?)
  }

  /// Names the model-list protocol this upstream serves, for [`Client::get_model_list`].
  #[must_use]
  pub fn with_model_list(mut self, model_list: ModelListProtocol) -> Self {
    self.model_list = Some(model_list);
    self
  }

  /// Replaces the retry policy of this client; `max_attempts: 1` sends exactly one attempt.
  #[must_use]
  pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
    self.retry = retry;
    self
  }

  /// Replaces the clock this client reads, as unix seconds. The expiry judgment reads it - at
  /// every dispatch and in [`Client::are_credentials_expired`] - and the machine's clock is the
  /// default. A caller who wants determinism hands its own.
  #[must_use]
  pub fn with_clock(mut self, clock: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
    self.clock = Some(clock);
    self
  }

  /// When the material stops being accepted, when it says so itself; `None` is material that
  /// does not run out.
  #[must_use]
  pub fn get_credential_expiry(&self) -> Option<u64> {
    self.credentials.expires_at
  }

  /// Whether the material has run out, against the clock this client reads.
  #[must_use]
  pub fn are_credentials_expired(&self) -> bool {
    crate::protocol::outbound::is_expired(&self.credentials, self.get_current_time())
  }

  /// Exchanges the material this client holds for fresh material, over the transport this
  /// client holds and by the renewal option the target's bearer auth carries - every credential
  /// that renews is an access token read from `Authorization` - and hands the renewed material
  /// back, installed nowhere.
  ///
  /// What comes back is [`Credentials::renew`] over the exchange's answer: a new access token,
  /// the rotated refresh token (kept, when the endpoint rotated nothing), the account and expiry
  /// the exchange named, everything else as it was. Installing it is [`Client::set_credentials`],
  /// and storing what the exchange rotated stays with whoever holds the account state.
  ///
  /// # Errors
  ///
  /// [`Error::Unsupported`] when the target's auth carries no renewal - a key or a signature
  /// neither runs out nor exchanges - [`Error::Build`] when the material does not carry what
  /// its exchange needs, and the rest as that exchange states them.
  pub async fn refresh_credentials(&self) -> Result<Credentials, Error> {
    let refresh = match self.outbound.get_auth() {
      AuthProtocol::Bearer(Some(refresh)) => refresh,
      auth => {
        return Err(Error::build_unsupported(
          "credential refresh",
          auth.get_name(),
          "carries no renewal: a key or a signature neither runs out nor exchanges",
        ));
      }
    };
    match refresh {
      CredentialsRefreshProtocol::OAuth => {
        let tokens =
          crate::protocol::outbound::oauth::refresh(&self.transport, &self.credentials).await?;
        Ok(self.credentials.renew(&tokens))
      }
      CredentialsRefreshProtocol::GoogleAdc { adc, scope } => {
        let tokens = crate::protocol::outbound::adc::fetch_token(
          &self.transport,
          adc,
          scope,
          self.get_current_time(),
        )
        .await?;
        // A Google exchange rotates nothing: the grant was made once, and the token it hands
        // back is the only thing that changed. `renew` keeps the rest as it was.
        Ok(self.credentials.renew(&crate::protocol::outbound::Tokens {
          access_token: tokens.access_token,
          expires_at: Some(tokens.expires_at),
          ..Default::default()
        }))
      }
    }
  }

  /// Installs material in place of the old, touched by nothing else: the protocols, the target
  /// and the clock stay as they were.
  pub fn set_credentials(&mut self, credentials: Credentials) {
    self.credentials = credentials;
  }

  /// Sends a request in the response mode it declares. Buffered calls retry whole attempts;
  /// streamed calls retry only until the first event, then hand ownership of the stream back.
  pub async fn call(
    &self,
    request: &Request,
  ) -> Result<CallResponse<EventStream<T::Stream>>, Error> {
    if request.stream {
      self.stream_with_retries(request).await.map(CallResponse::Stream)
    } else {
      retry(&self.retry, |_| self.attempt(request), classify_retry)
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
  /// [`Error::Unsupported`] when no model-list protocol was named for this client, or when the
  /// wire publishes no list to ask for; the rest as [`mod@crate::protocol::model_list::fetch`]
  /// states them.
  pub async fn get_model_list(&self, query: &ModelListQuery) -> Result<ModelCatalog, Error> {
    let protocol = self.model_list.ok_or_else(|| {
      Error::build_unsupported(
        "model list",
        self.model_use.get_name(),
        "was not named when this client was built",
      )
    })?;
    model_list::fetch(&self.transport, protocol, query, &self.credentials, self.get_current_time())
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
    account_state::fetch(
      &self.transport,
      protocol,
      &self.credentials,
      base_url,
      self.get_current_time(),
    )
    .await
  }

  /// Internal: one attempt of [`Client::call`]: render, resolve, build, execute, then decode or report.
  fn build_model_use_call(&self, request: &Request) -> Result<Call, Error> {
    let session_id = self
      .outbound
      .session_id()
      .map(str::to_owned)
      .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut body = self.model_use.render(request)?;
    self.apply_code_agent_identity(&mut body, &session_id);
    let body = Self::serialize_body(body)?;
    let path =
      self.model_use.resolve_request_path(request, self.outbound.resolve_path(&request.model));
    self.outbound.clone().with_session_id(session_id).dispatch(
      self.build_draft(path, self.build_headers(&[]), body),
      &self.credentials,
      self.get_current_time(),
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

  async fn attempt(&self, request: &Request) -> Result<Response, Error> {
    let call = self.build_model_use_call(request)?;
    let reply = self.transport.execute(&call).await?;
    if !reply.is_success() {
      return Err(
        self
          .decode_upstream_error(reply.status, &reply.body)
          .with_retry_after(reply.get_retry_after_ms()),
      );
    }
    let body = Self::decode_reply_body(&reply)?;
    let mut response = self.model_use.decode(&body)?;
    response.account_state = self.parse_account_state_reading(&reply.headers, Some(&body))?;
    Ok(response)
  }

  /// Internal: the call a compaction is asked with, built the one way both lanes build it: the wire's own
  /// path or its compacting suffix, the rendered trigger body, and the headers this wire adds
  /// beside what all of its calls carry. Refused here when this client's wire has no compaction
  /// call, before anything is sent.
  fn build_upstream_compaction_call(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<Call, Error> {
    let (variant, path) = match (self.model_use, self.upstream_compaction) {
      // A deployment that compacts on its ordinary call keeps that call's path.
      (
        ModelUseProtocol::OpenAiResponses(variant),
        Some(UpstreamCompactionProtocol::OpenAiResponsesStreamed),
      ) => (variant, self.outbound.resolve_path(&request.model)),
      (
        ModelUseProtocol::OpenAiResponses(variant),
        Some(UpstreamCompactionProtocol::OpenAiResponses),
      ) => (variant, format!("{}/compact", self.outbound.resolve_path(&request.model))),
      _ => {
        return Err(Error::build_unsupported(
          "upstream compaction",
          self.model_use.get_name(),
          CompactionUnsupported::NoCall.get_text(),
        ));
      }
    };
    let mut body = upstream_compaction_wire::openai_responses::render(request, variant)?;
    let session_id = self
      .outbound
      .session_id()
      .map(str::to_owned)
      .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if variant.deployment == ResponsesDeployment::Codex {
      self.apply_code_agent_identity(&mut body, &session_id);
    }
    let body = Self::serialize_body(body)?;
    self.outbound.clone().with_session_id(session_id).dispatch(
      self.build_draft(
        path,
        self.build_headers(upstream_compaction_wire::openai_responses::get_extra_headers(variant)),
        body,
      ),
      &self.credentials,
      self.get_current_time(),
    )
  }

  /// Internal: one attempt of [`Client::compact_upstream`]: render, resolve, build, execute, then read or report.
  async fn attempt_upstream_compaction(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    if matches!(self.upstream_compaction, Some(UpstreamCompactionProtocol::OpenAiResponsesStreamed))
    {
      return self.compact_upstream_from_stream(request).await;
    }
    let call = self.build_upstream_compaction_call(request)?;
    let reply = self.transport.execute(&call).await?;
    if !reply.is_success() {
      return Err(
        self
          .decode_upstream_error(reply.status, &reply.body)
          .with_retry_after(reply.get_retry_after_ms()),
      );
    }
    let body = Self::decode_reply_body(&reply)?;
    // The call was built, so this wire is the responses one: the decode has no refusal of its
    // own to make.
    let mut compaction =
      crate::protocol::upstream_compaction::response::openai_responses::decode(&body)?;
    compaction.account_state = self.parse_account_state_reading(&reply.headers, Some(&body))?;
    Ok(compaction)
  }

  /// Internal: one attempt of a compaction on a deployment that answers it on the call it always serves: the
  /// trigger item in the body is what asks for the compaction, and the item standing in for the
  /// history arrives on the stream like any other item.
  async fn compact_upstream_from_stream(
    &self,
    request: &UpstreamCompactionRequest,
  ) -> Result<UpstreamCompaction, Error> {
    let call = self.build_upstream_compaction_call(request)?;
    let observer = self.stream_observer.as_ref().map(|factory| factory(&request.model));
    let reply = self.transport.execute_stream(&call).await?;
    // The responses wire is the only one with this call, and it is served over SSE.
    let mut source = WireEventSource::Sse(SseStream::new(reply));
    if !source.is_success() {
      let body = source.read_error_body().await;
      return Err(
        self
          .decode_upstream_error(source.get_status(), &body)
          .with_retry_after(source.get_retry_after_ms()),
      );
    }
    let account_state = self.parse_account_state_reading(source.get_headers(), None)?;
    let mut decoder = self.model_use.create_stream_decoder()?;
    let mut accumulator = StreamAccumulator::new().with_account_state(account_state);
    let mut stopped = false;
    while !stopped {
      match source.next().await? {
        Some(wire_event) => {
          for event in decoder.feed(wire_event.event.as_deref(), &wire_event.data)? {
            if let Some(observer) = &observer {
              observer.observe(&event);
            }
            stopped |= matches!(event, StreamEvent::Stop(_));
            accumulator.feed(event)?;
          }
        }
        None => {
          for event in decoder.finish()? {
            accumulator.feed(event)?;
          }
          break;
        }
      }
    }
    decode_streamed_upstream_compaction(accumulator.finish()?)
  }

  /// Internal: the account reading this reply carries, when the endpoint named an account protocol.
  ///
  /// `body` is `None` on the streamed path, where only the reply head is in hand: a protocol that
  /// needs the payload reports [`Error::Build`] there instead of quietly finding nothing.
  fn parse_account_state_reading(
    &self,
    headers: &[(String, String)],
    body: Option<&Value>,
  ) -> Result<Option<AccountState>, Error> {
    match self.account_state {
      Some(protocol) => account_state::parse_reply(protocol, headers, body).map(Some),
      None => Ok(None),
    }
  }

  /// Opens a streamed call, replacing the attempt while no event has been handed over.
  ///
  /// Nothing is assembled here: [`EventStream::next`] hands over the protocol's events, and the
  /// `StreamAccumulator` is one consumer of them, not the
  /// only one. A non-`2xx` reply never becomes a stream; its body is mapped exactly like the
  /// buffered path. Before the stream is handed over its first event is read: up to that point the
  /// attempt delivered nothing, so it may still be replaced - when the failure is one the transport
  /// and the policy would replace at all: a connect failure or a head that never came, but not a
  /// failure after the service started answering. Once an event is in the caller's hands a failure
  /// is terminal, because a replay would splice a second copy of the answer into what the caller
  /// has already seen.
  async fn stream_with_retries(&self, request: &Request) -> Result<EventStream<T::Stream>, Error> {
    let mut attempt = 1;
    loop {
      let error = match self.open_stream(request).await {
        Ok(mut stream) => match stream.next().await {
          Ok(Some(event)) => {
            stream.pending.push_front(event);
            return Ok(stream);
          }
          Ok(None) => return Ok(stream),
          Err(error) => error,
        },
        Err(error) => error,
      };
      if attempt >= self.retry.max_attempts || !error.is_retryable() {
        return Err(error);
      }
      tokio::time::sleep(Duration::from_millis(
        self.retry.calculate_backoff_ms(attempt, error.get_retry_after_ms()),
      ))
      .await;
      attempt += 1;
    }
  }

  /// Internal: one attempt of [`Client::call`]: open the body, or report why it cannot be opened.
  async fn open_stream(&self, request: &Request) -> Result<EventStream<T::Stream>, Error> {
    let decoder = self.model_use.create_stream_decoder()?;
    let call = self.build_model_use_call(request)?;
    let observer = self.stream_observer.as_ref().map(|factory| factory(&request.model));
    let reply = self.transport.execute_stream(&call).await?;
    let mut source = match self.model_use {
      ModelUseProtocol::BedrockConverse => WireEventSource::Bedrock(BedrockStream::new(reply)),
      _ => WireEventSource::Sse(SseStream::new(reply)),
    };
    if !source.is_success() {
      let body = source.read_error_body().await;
      return Err(
        self
          .decode_upstream_error(source.get_status(), &body)
          .with_retry_after(source.get_retry_after_ms()),
      );
    }
    let account_state = self.parse_account_state_reading(source.get_headers(), None)?;
    Ok(EventStream {
      observer,
      source,
      decoder,
      pending: VecDeque::new(),
      done: false,
      account_state,
      protocol: self.model_use,
    })
  }

  fn get_current_time(&self) -> u64 {
    self.clock.as_ref().map_or_else(
      || {
        std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)
          .map_or(0, |since| since.as_secs())
      },
      |clock| clock(),
    )
  }

  fn build_draft(&self, path: String, headers: Vec<(String, String)>, body: Vec<u8>) -> Draft {
    Draft { method: Method::Post, path: Some(path), query: Vec::new(), headers, body }
  }

  fn serialize_body(body: Value) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(&body)
      .map_err(|error| Error::Build(format!("request body is not serializable: {error}")))
  }

  fn decode_reply_body(reply: &Reply) -> Result<Value, Error> {
    serde_json::from_slice(&reply.body).map_err(|error| {
      Error::Malformed(format!(
        "2xx response body is not JSON ({error}): {}",
        http_error::decode_body_text(&reply.body)
      ))
    })
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
    match serde_json::from_slice::<Value>(body) {
      Ok(body) => self.model_use.decode_http_error(status, &body),
      Err(_) => Error::from_http(
        status,
        None,
        format!("HTTP {status}: {}", http_error::decode_body_text(body)),
      ),
    }
  }
}

/// One wire event of a streamed body: the protocol's event name, when its framing carries one, and
/// its payload. What the name means is the decoder's business, not the reader's.
struct WireEvent {
  event: Option<String>,
  data: String,
}

/// Where a streamed body's wire events come from. Bedrock's binary event-stream frames carry the
/// same shape an SSE dispatch does - an event name and a payload - so one reader per framing, one
/// shape for both.
enum WireEventSource<S: ReplyStream> {
  Sse(SseStream<S>),
  Bedrock(BedrockStream<S>),
}

impl<S: ReplyStream> WireEventSource<S> {
  fn get_status(&self) -> u16 {
    match self {
      WireEventSource::Sse(stream) => stream.get_status(),
      WireEventSource::Bedrock(stream) => stream.get_status(),
    }
  }

  fn is_success(&self) -> bool {
    match self {
      WireEventSource::Sse(stream) => stream.is_success(),
      WireEventSource::Bedrock(stream) => stream.is_success(),
    }
  }

  fn get_retry_after_ms(&self) -> Option<u64> {
    match self {
      WireEventSource::Sse(stream) => stream.get_retry_after_ms(),
      WireEventSource::Bedrock(stream) => stream.get_retry_after_ms(),
    }
  }

  fn get_headers(&self) -> &[(String, String)] {
    match self {
      WireEventSource::Sse(stream) => stream.get_headers(),
      WireEventSource::Bedrock(stream) => stream.get_headers(),
    }
  }

  async fn read_error_body(&mut self) -> Vec<u8> {
    match self {
      WireEventSource::Sse(stream) => stream.read_error_body().await,
      WireEventSource::Bedrock(stream) => stream.read_error_body().await,
    }
  }

  /// The next wire event, or `None` at the end of the body. SSE comments (heartbeats) never
  /// surface.
  async fn next(&mut self) -> Result<Option<WireEvent>, Error> {
    match self {
      WireEventSource::Sse(stream) => loop {
        match stream.next().await? {
          Some(SseEvent::Dispatch { event, data, .. }) => {
            return Ok(Some(WireEvent { event, data }));
          }
          Some(SseEvent::Comment(_)) => continue,
          None => return Ok(None),
        }
      },
      WireEventSource::Bedrock(stream) => match stream.next().await? {
        Some(frame) => Ok(Some(WireEvent { event: Some(frame.event), data: frame.data })),
        None => Ok(None),
      },
    }
  }
}

/// One streamed call in flight: body wire events decoded into the protocol's normalized events.
pub struct EventStream<S: ReplyStream> {
  observer: Option<Box<dyn super::StreamObserver>>,
  protocol: ModelUseProtocol,
  source: WireEventSource<S>,
  decoder: StreamDecoder,
  pending: VecDeque<StreamEvent>,
  done: bool,
  account_state: Option<AccountState>,
}

impl<S: ReplyStream> EventStream<S> {
  /// A caller-owned accumulator with this wire's interruption replay rules and reply metadata.
  pub fn create_accumulator(&self) -> StreamAccumulator {
    StreamAccumulator::for_protocol(self.protocol).with_account_state(self.account_state.clone())
  }

  /// Abandon local reading without flushing parsers or synthesizing a normal EOF.
  /// This does not confirm remote cancellation or stop upstream billing.
  pub fn abort(self) {
    drop(self);
  }

  /// The account reading the opening reply carried, when the endpoint named an account protocol.
  ///
  /// It is known before the first event, because the reply head is read before the body is
  /// streamed. Handing it to a `StreamAccumulator` puts it
  /// beside the decoded messages.
  pub fn get_account_state(&self) -> Option<&AccountState> {
    self.account_state.as_ref()
  }

  /// The next normalized event, or `None` at the end of the stream.
  ///
  /// Reading stops at the protocol's terminal event even if the body keeps going: nothing after it
  /// is trustworthy, and the accumulator would reject it anyway.
  pub async fn next(&mut self) -> Result<Option<StreamEvent>, Error> {
    loop {
      if let Some(event) = self.pending.pop_front() {
        return Ok(Some(event));
      }
      if self.done {
        return Ok(None);
      }
      match self.source.next().await? {
        Some(wire_event) => {
          let events = self.decoder.feed(wire_event.event.as_deref(), &wire_event.data)?;
          for event in &events {
            if let Some(observer) = &self.observer {
              observer.observe(event);
            }
          }
          if events.iter().any(|event| matches!(event, StreamEvent::Stop(_))) {
            self.done = true;
            self.observer.take();
          }
          self.pending.extend(events);
        }
        None => {
          let events = self.decoder.finish()?;
          self.observer.take();
          self.pending.extend(events);
          self.done = true;
        }
      }
    }
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
