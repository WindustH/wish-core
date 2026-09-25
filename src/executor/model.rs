//! Model requests, streaming, token counting, provider discovery.
mod caller;
pub mod client;
mod continuation;
mod execute;
mod observer;
pub use observer::{StreamObserver, StreamObserverFactory};
pub mod tokens;

pub use caller::{ModelCaller, ModelStream};
pub use client::{CallResponse, Client, EventStream};
pub(super) use continuation::{Continuation, combine_usage};
pub(super) use execute::{ModelResult, execute_model};
