//! Stable protocol configuration identifiers and their wire modes.
use super::{
  ModelUseProtocol,
  mode::{
    ChatCompletionApiCompatMode as Chat, MessagesApiCompatMode as Messages, ReasoningForm,
    ResponsesApiMode, ResponsesDeployment,
  },
};
pub fn parse_protocol(name: &str) -> Result<ModelUseProtocol, crate::protocol::error::Error> {
  Ok(match name {
    "openai_chat" => ModelUseProtocol::OpenAiChat(Chat::Official),
    "compatible_chat" => ModelUseProtocol::OpenAiChat(Chat::Compatible),
    "deepseek_chat" => ModelUseProtocol::OpenAiChat(Chat::DeepSeek),
    "zai_chat" => ModelUseProtocol::OpenAiChat(Chat::Zai),
    "kimi_k2_chat" => ModelUseProtocol::OpenAiChat(Chat::KimiK2),
    "kimi_k3_chat" => ModelUseProtocol::OpenAiChat(Chat::KimiK3),
    "qwen_chat" => ModelUseProtocol::OpenAiChat(Chat::Qwen),
    "minimax_chat" => ModelUseProtocol::OpenAiChat(Chat::MiniMax),
    "mimo_chat" => ModelUseProtocol::OpenAiChat(Chat::Mimo),
    "tokenhub_chat" => ModelUseProtocol::OpenAiChat(Chat::TokenHub),
    "mistral_chat" => ModelUseProtocol::OpenAiChat(Chat::Mistral),
    "openai_responses" => ModelUseProtocol::OpenAiResponses(Default::default()),
    "plaintext_responses" => ModelUseProtocol::OpenAiResponses(ResponsesApiMode {
      reasoning_form: ReasoningForm::Plaintext,
      ..Default::default()
    }),
    "codex_responses" => ModelUseProtocol::OpenAiResponses(ResponsesApiMode {
      deployment: ResponsesDeployment::Codex,
      ..Default::default()
    }),
    "anthropic_messages" => ModelUseProtocol::AnthropicMessages(Messages::Official),
    "deepseek_messages" => ModelUseProtocol::AnthropicMessages(Messages::DeepSeek),
    "zai_messages" => ModelUseProtocol::AnthropicMessages(Messages::Zai),
    "kimi_messages" => ModelUseProtocol::AnthropicMessages(Messages::Kimi),
    "qwen_messages" => ModelUseProtocol::AnthropicMessages(Messages::Qwen),
    "minimax_messages" => ModelUseProtocol::AnthropicMessages(Messages::MiniMax),
    "mimo_messages" => ModelUseProtocol::AnthropicMessages(Messages::Mimo),
    "tokenhub_messages" => ModelUseProtocol::AnthropicMessages(Messages::TokenHub),
    "google_generate_content" => ModelUseProtocol::GoogleGenerateContent,
    "google_vertex_generate_content" => ModelUseProtocol::GoogleVertexGenerateContent,
    "google_antigravity_generate_content" => ModelUseProtocol::GoogleAntigravityGenerateContent,
    "google_interactions" => ModelUseProtocol::GoogleInteractions,
    "bedrock_converse" => ModelUseProtocol::BedrockConverse,
    "mistral_conversations" => ModelUseProtocol::MistralConversations,
    _ => {
      return Err(crate::protocol::error::Error::Build(format!(
        "unknown model-use protocol: {name}"
      )));
    }
  })
}
