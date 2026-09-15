//! The compaction call as the caller describes it, and the renderers that put it on a wire.
//!
//! A [`UpstreamCompactionRequest`](super::UpstreamCompactionRequest) is deliberately smaller than a model call: a
//! history and the model that compacts it, nothing else, because the wires that carry this call
//! accept nothing else on it. Each renderer says what it does with that.

pub mod openai_responses;
