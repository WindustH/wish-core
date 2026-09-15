//! Calling a model: the call as our caller describes it, and one protocol's translation of it.
//!
//! The shape of a call is ours and lives here: [`message`] is the conversation, [`tool`] what the
//! model may call, [`request`] what the caller wants, [`response`] what came back and [`stream`] the
//! events a streamed reply arrives as. Then, one tree per direction: [`request`]'s modules render
//! our call into a wire's body, [`response`]'s read a wire's body back, and [`stream`]'s turn its
//! events into [`StreamEvent`](crate::protocol::StreamEvent)s.
//!
//! The three trees stay apart because each has its own direction of travel and its own kind of
//! state: rendering is a function of the request alone, reading a body needs nothing else, and a
//! stream decoder is stateful and belongs to exactly one stream.

pub mod message;
pub mod request;
pub mod response;
pub mod stream;
pub mod tool;

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::http_error;

use request::anthropic_messages::MessagesApiCompatMode as MessagesCompat;
use request::openai_chat::ChatCompletionApiCompatMode as ChatCompat;
use request::openai_responses::ResponsesApiCompatMode as ResponsesCompat;
use stream::StreamDecoder;

/// Which protocol kind of model use a client speaks: one variant per wire, each carrying the
/// compat mode that says which vendor's reasoning extension rides it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUseProtocol {
  /// Which vendor's reasoning extension this endpoint speaks; `Official` is Claude's own shape.
  AnthropicMessages(MessagesCompat),
  /// Which vendor's reasoning extension this endpoint speaks; `Official` is the plain wire.
  OpenAiChat(ChatCompat),
  /// The same protocol in two wire forms: which reasoning form is asked for is the caller's choice.
  OpenAiResponses(ResponsesCompat),
  GoogleGenerateContent,
  /// Vertex AI's serving of the same `generateContent` family: the wire body, the decoder and the
  /// error envelope are Gemini's own, and the call is proven by an ADC access token
  /// ([`crate::protocol::outbound::adc`]) instead of an API key - the caller exchanges and renews
  /// the token, and an [`Outbound`](crate::protocol::outbound::Outbound) whose auth is `Bearer`
  /// carries it.
  GoogleVertexGenerateContent,
  GoogleInteractions,
  BedrockConverse,
  /// Mistral's stateful wire: the history as `inputs` entries, the answer as `outputs` entries.
  MistralConversations,
}

impl ModelUseProtocol {
  /// Headers this protocol needs besides `content-type` and auth.
  pub(crate) fn headers(self) -> &'static [(&'static str, &'static str)] {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => request::anthropic_messages::HEADERS,
      ModelUseProtocol::OpenAiChat(..) => request::openai_chat::HEADERS,
      ModelUseProtocol::OpenAiResponses(mode) => mode.deployment.headers(),
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        request::google_generate_content::HEADERS
      }
      ModelUseProtocol::GoogleInteractions => request::google_interactions::HEADERS,
      ModelUseProtocol::BedrockConverse => request::bedrock_converse::HEADERS,
      ModelUseProtocol::MistralConversations => request::mistral_conversations::HEADERS,
    }
  }

  pub(crate) fn render(self, request: &request::Request, stream: bool) -> Result<Value, Error> {
    match self {
      ModelUseProtocol::AnthropicMessages(mode) => {
        request::anthropic_messages::render(request, mode, stream)
      }
      ModelUseProtocol::OpenAiChat(mode) => request::openai_chat::render(request, mode, stream),
      ModelUseProtocol::OpenAiResponses(variant) => {
        request::openai_responses::render(request, variant, stream)
      }
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        request::google_generate_content::render(request)
      }
      ModelUseProtocol::GoogleInteractions => request::google_interactions::render(request, stream),
      ModelUseProtocol::BedrockConverse => request::bedrock_converse::render(request),
      ModelUseProtocol::MistralConversations => {
        request::mistral_conversations::render(request, stream)
      }
    }
  }

  /// The streamed form of a resolved path, when the streaming verb differs from the buffered
  /// one. A protocol whose streamed call uses the same URL returns it unchanged.
  pub(crate) fn stream_path(self, path: String) -> String {
    match self {
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        stream::google_generate_content::stream_path(&path)
      }
      ModelUseProtocol::BedrockConverse => stream::bedrock_converse::stream_path(&path),
      _ => path,
    }
  }

  /// The stream decoder of this protocol, or why it does not have one yet.
  pub(crate) fn stream_decoder(self) -> Result<StreamDecoder, Error> {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => {
        Ok(StreamDecoder::AnthropicMessages(stream::anthropic_messages::Decoder::new()))
      }
      ModelUseProtocol::OpenAiChat(mode) => {
        Ok(StreamDecoder::OpenAiChat(stream::openai_chat::Decoder::new(mode)))
      }
      ModelUseProtocol::OpenAiResponses(..) => {
        Ok(StreamDecoder::OpenAiResponses(stream::openai_responses::Decoder::new()))
      }
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        Ok(StreamDecoder::GoogleGenerateContent(stream::google_generate_content::Decoder::new()))
      }
      ModelUseProtocol::GoogleInteractions => {
        Ok(StreamDecoder::GoogleInteractions(stream::google_interactions::Decoder::new()))
      }
      ModelUseProtocol::BedrockConverse => {
        Ok(StreamDecoder::BedrockConverse(stream::bedrock_converse::Decoder::new()))
      }
      ModelUseProtocol::MistralConversations => {
        Ok(StreamDecoder::MistralConversations(stream::mistral_conversations::Decoder::new()))
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

  pub(crate) fn http_error(self, status: u16, body: &Value) -> Error {
    match self {
      ModelUseProtocol::AnthropicMessages(..) => http_error::anthropic_messages(status, body),
      ModelUseProtocol::OpenAiChat(..) => http_error::openai_chat(status, body),
      ModelUseProtocol::OpenAiResponses(..) => http_error::openai_responses(status, body),
      ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent => {
        http_error::google_generate_content(status, body)
      }
      ModelUseProtocol::GoogleInteractions => http_error::google_interactions(status, body),
      ModelUseProtocol::BedrockConverse => http_error::bedrock_converse(status, body),
      ModelUseProtocol::MistralConversations => http_error::mistral_conversations(status, body),
    }
  }

  /// The wire's own name, for the errors that say which protocol refused something.
  pub(crate) fn name(self) -> &'static str {
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
}
