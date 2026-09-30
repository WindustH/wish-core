//! The model side of a run: the `ModelCaller` contract a provider client fulfils, and one logical
//! model call - its segments, continued past output limits, with stream events delivered live.
mod caller;
mod continuation;
mod execute;

pub use caller::{CallResponse, ModelCaller, ModelStream};
pub(super) use execute::{ModelResult, execute_model};
