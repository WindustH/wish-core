//! What came back from a compaction call, and the readers that turn a wire's body into it.
//!
//! The reply is a conversation to continue from, not an answer:
//! [`UpstreamCompaction::conversation`](super::UpstreamCompaction::conversation) holds
//! the items the next call has to start with, in the order the service put them in, and each reader
//! below says how one wire's body becomes that list.

pub mod openai_responses;
