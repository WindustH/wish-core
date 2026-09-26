//! The compaction call as the caller describes it, and the renderers that put it on a wire.
//!
//! A [`UpstreamCompactionRequest`](super::UpstreamCompactionRequest) is deliberately smaller than a model call: a
//! history and the model that compacts it, plus the prompt controls of the calls that sent it, which
//! only a deployment that compacts on its ordinary model call takes. Each renderer says what it does
//! with that.

pub mod openai_responses;
