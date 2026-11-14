//! deepseek extension. Owns its service contracts.
mod endpoint;
pub(crate) mod fetch;
pub(crate) mod fetch_tool;
pub(crate) mod search;
pub(crate) mod search_tool;
pub mod sources;
mod speech;
mod speech_driver;
mod speech_tool;
mod transcribe_tool;
mod transcription;
mod transcription_driver;

pub(crate) mod package;
