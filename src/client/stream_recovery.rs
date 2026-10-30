//! In-process recovery state for one frozen physical request. Never serialized or logged.
use crate::protocol::{
  Usage,
  attempt::Call,
  model_use::{recovery::RecoveryProtocol, stream::StreamDecoder},
};
use crate::utils::retry::RetryPolicy;
pub(super) struct Recovery {
  pub observer: Option<super::AttemptObserverFactory>,
  pub model: String,
  pub protocol: RecoveryProtocol,
  pub original: Call,
  pub policy: RetryPolicy,
  pub token: Option<String>,
  pub decoder: Option<StreamDecoder>,
  pub revision: u64,
  pub hops: u32,
  pub failures: u32,
  pub continuation: bool,
  pub waiting: Option<u64>,
  pub usage: Usage,
  pub committed_usage: Usage,
  pub previous_usage: Usage,
  pub initial_input: Option<u64>,
}
impl Recovery {
  pub fn new(protocol: RecoveryProtocol, original: Call, policy: RetryPolicy) -> Self {
    Self {
      observer: None,
      model: String::new(),
      protocol,
      original,
      policy,
      token: None,
      decoder: None,
      revision: 0,
      hops: 0,
      failures: 0,
      continuation: false,
      waiting: None,
      usage: Usage::default(),
      committed_usage: Usage::default(),
      previous_usage: Usage::default(),
      initial_input: None,
    }
  }
}
