//! openai extension. Owns its service contracts.
pub(crate) mod chatgpt_image_tool;
pub mod endpoint;
pub(crate) mod fetch;
pub(crate) mod fetch_tool;
pub(crate) mod image;
pub(crate) mod image_tool;
pub mod preparation;
pub(crate) mod search;
pub(crate) mod search_tool;
pub mod sources;
pub(crate) mod speech;
pub(crate) mod speech_tool;
pub mod subscription_image;
pub(crate) mod transcribe_tool;
pub(crate) mod transcription;

pub(crate) mod image_driver;

pub(crate) mod codex_driver;

pub(crate) mod openai_speech_driver;

pub(crate) mod openai_transcription_driver;

pub(crate) mod transcription_driver;

pub(crate) mod execution;

pub(crate) mod package;
