//! Calling a model: the call as our caller describes it, and one protocol's translation of it.
//!
//! The shape of a call is ours and lives here: [`message`] is the conversation, [`tool`] what the
//! model may call, [`request`] what the caller wants, [`response`] what came back and [`stream`] the
//! events a streamed reply arrives as. Then, one tree per direction: [`request`]'s modules render
//! our call into a wire's body, [`response`]'s read a wire's body back, and [`stream`]'s turn its
//! events into [`StreamEvent`](crate::protocol::StreamEvent)s. [`mode`] holds the per-wire modes all
//! three trees consult, and [`context`] the units a history can be cut into.
//!
//! The three trees stay apart because each has its own direction of travel and its own kind of
//! state: rendering is a function of the request alone, reading a body needs nothing else, and a
//! stream decoder is stateful and belongs to exactly one stream. What one protocol's trees must
//! agree on beyond the wire - how the reasoning it produced may be sent back to it - is
//! [`ModelUseProtocol::get_reasoning_replay`].

pub mod context;
pub mod message;
pub(crate) mod mistral_chunks;
pub mod mode;
pub mod request;
pub mod response;
pub mod stream;
pub mod tool;

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::http_error;

use message::ReasoningOpaqueKind;
use mode::{
  ChatCompletionApiCompatMode as ChatCompat, MessagesApiCompatMode as MessagesCompat,
  ReasoningForm, ResponsesApiMode,
};
use stream::{ReplayDisposition, StreamDecoder};

/// Which protocol kind of model use a client speaks: one variant per wire, each carrying the
/// mode that says which vendor's reasoning extension rides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUseProtocol {
  /// Which vendor's reasoning extension this endpoint speaks; `Official` is Claude's own shape.
  AnthropicMessages(MessagesCompat),
  /// Which vendor's reasoning extension this endpoint speaks; `Official` is the plain wire.
  OpenAiChat(ChatCompat),
  /// The same protocol in two wire forms: which reasoning form is asked for is the caller's choice.
  OpenAiResponses(ResponsesApiMode),
  GoogleGenerateContent,
  /// Vertex AI's serving of the same `generateContent` family: the wire body, the decoder and the
  /// error envelope are Gemini's own, and the call is proven by a Google access token instead of
  /// an API key - the caller obtains and renews the token, and an
  /// [`Endpoint`](crate::protocol::endpoint::Endpoint) whose auth is `Bearer` carries it.
  GoogleVertexGenerateContent,
  GoogleInteractions,
  BedrockConverse,
  /// Mistral's stateful wire: the history as `inputs` entries, the answer as `outputs` entries.
  MistralConversations,
}

impl ModelUseProtocol {
  /// Headers this protocol needs besides `content-type` and auth.
  pub(crate) fn get_headers(self) -> &'static [(&'static str, &'static str)] {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => request::anthropic_messages::HEADERS,
      ModelUseProtocol::OpenAiChat(..) => request::openai_chat::HEADERS,
      ModelUseProtocol::OpenAiResponses(mode) => request::openai_responses::get_headers(mode),
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        request::google_generate_content::HEADERS
      }
      ModelUseProtocol::GoogleInteractions => request::google_interactions::HEADERS,
      ModelUseProtocol::BedrockConverse => request::bedrock_converse::HEADERS,
      ModelUseProtocol::MistralConversations => request::mistral_conversations::HEADERS,
    }
  }

  pub(crate) fn render(self, request: &request::Request) -> Result<Value, Error> {
    match self {
      ModelUseProtocol::AnthropicMessages(mode) => {
        request::anthropic_messages::render(request, mode)
      }
      ModelUseProtocol::OpenAiChat(mode) => request::openai_chat::render(request, mode),
      ModelUseProtocol::OpenAiResponses(mode) => request::openai_responses::render(request, mode),
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        request::google_generate_content::render(request)
      }
      ModelUseProtocol::GoogleInteractions => request::google_interactions::render(request),
      ModelUseProtocol::BedrockConverse => request::bedrock_converse::render(request),
      ModelUseProtocol::MistralConversations => request::mistral_conversations::render(request),
    }
  }

  /// Resolve the request's response mode into its wire endpoint.
  pub(crate) fn resolve_request_path(self, request: &request::Request, path: String) -> String {
    if !request.stream {
      return path;
    }
    match self {
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        request::google_generate_content::resolve_stream_path(&path)
      }
      ModelUseProtocol::BedrockConverse => request::bedrock_converse::resolve_stream_path(&path),
      _ => path,
    }
  }

  /// A fresh decoder for one stream of this protocol.
  pub(crate) fn create_stream_decoder(self) -> StreamDecoder {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => {
        StreamDecoder::AnthropicMessages(stream::anthropic_messages::Decoder::new())
      }
      ModelUseProtocol::OpenAiChat(mode) => {
        StreamDecoder::OpenAiChat(stream::openai_chat::Decoder::new(mode))
      }
      ModelUseProtocol::OpenAiResponses(..) => {
        StreamDecoder::OpenAiResponses(stream::openai_responses::Decoder::new())
      }
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        StreamDecoder::GoogleGenerateContent(stream::google_generate_content::Decoder::new())
      }
      ModelUseProtocol::GoogleInteractions => {
        StreamDecoder::GoogleInteractions(stream::google_interactions::Decoder::new())
      }
      ModelUseProtocol::BedrockConverse => {
        StreamDecoder::BedrockConverse(stream::bedrock_converse::Decoder::new())
      }
      ModelUseProtocol::MistralConversations => {
        StreamDecoder::MistralConversations(stream::mistral_conversations::Decoder::new())
      }
    }
  }

  pub(crate) fn decode(self, body: &Value) -> Result<response::Response, Error> {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => response::anthropic_messages::decode(body),
      ModelUseProtocol::OpenAiChat(mode) => response::openai_chat::decode(body, mode),
      ModelUseProtocol::OpenAiResponses(..) => response::openai_responses::decode(body),
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        response::google_generate_content::decode(body)
      }
      ModelUseProtocol::GoogleInteractions => response::google_interactions::decode(body),
      ModelUseProtocol::BedrockConverse => response::bedrock_converse::decode(body),
      ModelUseProtocol::MistralConversations => response::mistral_conversations::decode(body),
    }
  }

  pub(crate) fn decode_http_error(self, status: u16, body: &Value) -> Error {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => {
        http_error::decode_anthropic_messages(status, body)
      }
      ModelUseProtocol::OpenAiChat(..) | ModelUseProtocol::OpenAiResponses(..) => {
        http_error::decode_openai_envelope(status, body)
      }
      ModelUseProtocol::GoogleGenerateContent
      | ModelUseProtocol::GoogleVertexGenerateContent
      | ModelUseProtocol::GoogleInteractions => http_error::decode_google_envelope(status, body),
      ModelUseProtocol::BedrockConverse => http_error::decode_bedrock_converse(status, body),
      ModelUseProtocol::MistralConversations => {
        http_error::decode_mistral_conversations(status, body)
      }
    }
  }

  /// The wire's own name, for the errors that say which protocol refused something.
  pub(crate) fn get_name(self) -> &'static str {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => "anthropic messages",
      ModelUseProtocol::OpenAiChat(..) => "openai chat",
      ModelUseProtocol::OpenAiResponses(..) => "openai responses",
      ModelUseProtocol::GoogleGenerateContent => "google generateContent",
      ModelUseProtocol::GoogleVertexGenerateContent => "google vertex generateContent",
      ModelUseProtocol::GoogleInteractions => "google interactions",
      ModelUseProtocol::BedrockConverse => "bedrock converse",
      ModelUseProtocol::MistralConversations => "mistral conversations",
    }
  }

  /// How reasoning this protocol produced may be sent back to it.
  ///
  /// The stream events and the messages they become are wire-neutral, so what the reasoning's
  /// opaque material is, and what a block has to carry before it can go back, is this protocol's
  /// to say: the accumulator records the one and an interrupted call's partial output applies the
  /// other.
  pub(crate) fn get_reasoning_replay(self) -> ReasoningReplay {
    use ReasoningOpaqueKind as Kind;
    let (material, transparent, proof) = match self {
      ModelUseProtocol::AnthropicMessages(mode) => (
        Some(Kind::AnthropicSignature),
        mode != MessagesCompat::Official,
        ReplayProof::SignatureOrCiphertext,
      ),
      ModelUseProtocol::BedrockConverse => {
        (Some(Kind::BedrockSignature), false, ReplayProof::SignatureOrCiphertext)
      }
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        (Some(Kind::GoogleSignature), false, ReplayProof::Signature)
      }
      ModelUseProtocol::GoogleInteractions => {
        (Some(Kind::GoogleInteractionsThought), false, ReplayProof::Signature)
      }
      ModelUseProtocol::OpenAiResponses(mode) => match mode.reasoning_form {
        ReasoningForm::Plaintext => (Some(Kind::OpenAiEncrypted), true, ReplayProof::Plaintext),
        ReasoningForm::Ciphertext => (Some(Kind::OpenAiEncrypted), false, ReplayProof::Ciphertext),
      },
      // The plain chat wire has nowhere to send reasoning back.
      ModelUseProtocol::OpenAiChat(mode) if mode.is_plain() => (None, false, ReplayProof::Never),
      ModelUseProtocol::OpenAiChat(..) | ModelUseProtocol::MistralConversations => {
        (None, true, ReplayProof::Plaintext)
      }
    };
    ReasoningReplay {
      material,
      transparent,
      proof,
      seals_next_part: matches!(
        self,
        ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent
      ),
    }
  }
}

/// One protocol's rules for sending reasoning back (see [`ModelUseProtocol::get_reasoning_replay`]).
///
/// The default is the rule without a protocol: reasoning is display-only, since nothing says what
/// it could be replayed to.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ReasoningReplay {
  /// The format of the opaque material this protocol proves reasoning with, if it has any.
  material: Option<ReasoningOpaqueKind>,
  /// Whether unproven text is itself the replayable form, so a prefix of it can go back before its
  /// block completes. Transparency is the wire's replay representation, never visible text.
  transparent: bool,
  /// What a completed block has to carry to go back.
  proof: ReplayProof,
  /// Whether a signature-only block seals the model part right after it (Gemini's
  /// `thoughtSignature`), and so is worth nothing without that part.
  pub(crate) seals_next_part: bool,
}

/// What a completed reasoning block has to carry before its protocol takes it back.
#[derive(Clone, Copy, Debug, Default)]
enum ReplayProof {
  /// Nothing: the protocol takes no reasoning back.
  #[default]
  Never,
  /// Readable text, which is itself the replay.
  Plaintext,
  /// The encrypted payload.
  Ciphertext,
  /// The signature. An opaque state can arrive without visible text, so text proves nothing.
  Signature,
  /// A signature or a redacted payload.
  SignatureOrCiphertext,
}

impl ReasoningReplay {
  /// The format a block's opaque material is recorded as: `None` when the block carries none of the
  /// material this protocol proves reasoning with, and the redacted form where a payload stands in
  /// for the text.
  pub(crate) fn classify_material(
    &self,
    signature: &str,
    ciphertext: &str,
    replay_item: Option<&Value>,
  ) -> Option<ReasoningOpaqueKind> {
    use ReasoningOpaqueKind as Kind;
    let kind = self.material?;
    let has_material = match kind {
      Kind::GoogleInteractionsThought => replay_item.is_some(),
      Kind::OpenAiEncrypted => !ciphertext.is_empty(),
      Kind::GoogleSignature => !signature.is_empty(),
      _ => !signature.is_empty() || !ciphertext.is_empty(),
    };
    if !has_material {
      return None;
    }
    Some(match kind {
      Kind::AnthropicSignature if !ciphertext.is_empty() => Kind::AnthropicRedacted,
      Kind::BedrockSignature if !ciphertext.is_empty() => Kind::BedrockRedacted,
      other => other,
    })
  }

  /// Whether one reasoning block can go back, given whether the wire confirmed it `complete`: an
  /// unproven transparent prefix always can, and anything else only once complete and carrying the
  /// protocol's proof. A signature or ciphertext that has not arrived yet is not evidence either
  /// way, and the display summary is separate data.
  pub(crate) fn classify(
    &self,
    complete: bool,
    plaintext: &str,
    signature: &str,
    ciphertext: &str,
  ) -> ReplayDisposition {
    if self.transparent && signature.is_empty() && ciphertext.is_empty() && !plaintext.is_empty() {
      return ReplayDisposition::Replayable;
    }
    if !complete {
      return ReplayDisposition::Incomplete;
    }
    let replayable = match self.proof {
      ReplayProof::Never => false,
      ReplayProof::Plaintext => !plaintext.is_empty(),
      ReplayProof::Ciphertext => !ciphertext.is_empty(),
      ReplayProof::Signature => !signature.is_empty(),
      ReplayProof::SignatureOrCiphertext => !signature.is_empty() || !ciphertext.is_empty(),
    };
    if replayable { ReplayDisposition::Replayable } else { ReplayDisposition::NotReplayable }
  }
}
