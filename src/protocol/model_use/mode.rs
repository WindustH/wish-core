//! The per-wire modes: which vendor's extension, reasoning form or deployment one wire is spoken
//! in.
//!
//! A wire that several vendors reimplement, or that one vendor serves in more than one form, is
//! named by its protocol plus a mode. Rendering, reading a body and decoding a stream all consult
//! the same mode, so it lives here, beside them rather than inside any one of them; each renderer
//! re-exports the mode it takes.

/// Which reasoning extension this chat endpoint speaks on top of the official wire, and which
/// instruction roles it takes.
///
/// The official wire has no reasoning round trip at all; every vendor patches one in differently,
/// so each patch is its own mode however small the difference between two of them is. `Official`
/// and `Compatible` are the plain wire: nothing is read and nothing is sent. They differ only in
/// instruction roles, which only OpenAI's own endpoint takes as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatCompletionApiCompatMode {
  /// OpenAI's own endpoint: reasoning is neither read nor sent.
  #[default]
  Official,
  /// Any other endpoint that speaks the plain wire (routers, local servers, hosts of open models):
  /// reasoning as on `Official`, instructions in the one form every such endpoint takes.
  Compatible,
  /// `reasoning_content`, which must be replayed whenever tools are in play.
  DeepSeek,
  /// `reasoning_content` plus `thinking.clear_thinking: false` (the default strips the history).
  Zai,
  /// `reasoning_content` plus `thinking.keep: "all"` (k2.6 needs it; k2.7-code fixes it to that).
  KimiK2,
  /// `reasoning_content`; k3 has no `thinking` object.
  KimiK3,
  /// `reasoning_content` plus `preserve_thinking: true` (Bailian keeps nothing by default).
  Qwen,
  /// `reasoning_content` plus `reasoning_split: true` (otherwise the thoughts stay in `content`).
  MiniMax,
  /// `reasoning_content`; the history is kept unconditionally.
  Mimo,
  /// `reasoning_content`; the history is kept unconditionally.
  TokenHub,
  /// Reasoning rides in `content` as `thinking` chunks, and the history is replayed the same way.
  Mistral,
}

/// The assistant-message field plaintext reasoning rides in on the chat wire's vendor extensions.
pub(crate) const REASONING_FIELD: &str = "reasoning_content";

impl ChatCompletionApiCompatMode {
  /// Whether this is Mistral's own chat wire, which spells a few things its own way: reasoning in
  /// `content` chunks, `max_tokens` for the cap, no `store` or `stream_options`, `any` for a
  /// required tool call, and `model_length` for context exhaustion.
  pub(crate) fn is_mistral(self) -> bool {
    matches!(self, ChatCompletionApiCompatMode::Mistral)
  }

  /// Whether this is the plain wire, with no reasoning extension on top.
  pub(crate) fn is_plain(self) -> bool {
    matches!(self, ChatCompletionApiCompatMode::Official | ChatCompletionApiCompatMode::Compatible)
  }

  /// Whether the reasoning rides the flat [`REASONING_FIELD`], rather than nowhere (the plain
  /// wire) or in `content` chunks (Mistral).
  pub(crate) fn has_reasoning_field(self) -> bool {
    !self.is_plain() && !self.is_mistral()
  }
}

/// Which reasoning extension this Messages endpoint speaks on top of the official wire.
///
/// The official wire steers thinking with `adaptive` or with a budget that follows the shared tier
/// preset; every vendor that reimplements it patches in its own control, so each patch is its own
/// mode however small the difference between two of them is. `Official` is Claude's own shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessagesApiCompatMode {
  /// Claude's own shape: the tier preset (`adaptive`, or `enabled` with a budget).
  #[default]
  Official,
  /// `thinking{type: enabled|disabled}` plus `output_config.effort`.
  DeepSeek,
  /// `thinking{type: enabled|disabled}`; nothing else.
  Zai,
  /// `output_config.effort` only: the k3 wire is always on and takes no `thinking` object.
  Kimi,
  /// `output_config.effort` alone: it is the native replacement for the legacy thinking budget.
  Qwen,
  /// `thinking{type: adaptive|disabled}`; nothing else.
  MiniMax,
  /// `thinking{type: enabled|disabled}`; nothing else.
  Mimo,
  /// No documented controls: thinking blocks are read, nothing is sent.
  TokenHub,
}

/// Which of the Responses wire's reasoning forms this endpoint is asked for, and whose deployment of
/// the wire it is: the caller picks both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResponsesApiMode {
  pub reasoning_form: ReasoningForm,
  /// Whose deployment of this wire the call goes to.
  pub deployment: ResponsesDeployment,
}

/// Which form the reasoning round trip takes on the Responses wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReasoningForm {
  /// Send reasoning back as plaintext `reasoning_text` content.
  Plaintext,
  /// Ask for encrypted reasoning (`include: ["reasoning.encrypted_content"]`) and send it back.
  #[default]
  Ciphertext,
}

/// Which deployment of the Responses wire an endpoint lives on.
///
/// The body and the events are the same on both; what changes is what the call has to carry beside
/// them, and what the reply says about the account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResponsesDeployment {
  /// The platform API, which asks for nothing beyond auth.
  #[default]
  Platform,
  /// The Codex backend a ChatGPT subscription is served from: it routes by markers of its own, and
  /// the account's quota rides the reply.
  Codex,
}
