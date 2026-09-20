//! The protocol layer and the transport that carries it, with a persistent session state machine on top.
//!
//! `protocol` holds the wire-agnostic model, the per-protocol renderers, readers and stream
//! decoders that translate it, the readers that put what a service says about an account or its
//! models into one shape, the attempt contract a transport implements - the call, the reply and
//! the streamed body it gets back - and the failure vocabulary all of them speak; every request
//! those trees make is the same product: what the wire says, rendered as a draft, and how it is
//! reached - `outbound`, the target, the auth plan and the account material joined into one call.
//! `transport` implements that contract over HTTP with `reqwest`, `client` binds a protocol to a
//! target and runs the attempts, and `retry` is the policy it spends. `session` owns transactional state and paged history through `storage`; `agent` executes model
//! and tool effects selected by that state machine.

pub mod agent;
pub mod client;
pub mod session;
pub mod storage;

pub mod protocol;
pub mod retry;
pub mod transport;

pub use protocol::error::Error;
pub use retry::RetryPolicy;
