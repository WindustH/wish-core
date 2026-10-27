//! Execution port shared by tool adapters and the execution loop.
pub use crate::session::ToolInput;
use crate::{
  session::{ToolCall, ToolOutcome},
  utils::control::ExecutionControl,
};
use std::future::Future;

/// Executors must observe cancellation and return after their work has stopped or its outcome
/// is known to be unknown. The loop never drops a started execution to pretend it was cancelled.
/// Futures must not panic. Parallel mode requires tools safe to execute concurrently.
pub trait ToolExecutor: Sync {
  /// Applications may accept enabled capabilities through an alternative, unadvertised
  /// route. Generic executors retain strict registration unless they opt in explicitly.
  fn accepts_unadvertised_call(&self, _name: &str) -> bool {
    false
  }

  /// Opt-in application hook. Generic engine fixtures retain the ordinary execution contract.
  fn execute_with_input(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
    _input: Option<ToolInput>,
  ) -> impl Future<Output = ToolOutcome> + Send {
    self.execute(call, control)
  }

  fn execute(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> impl Future<Output = ToolOutcome> + Send;
}
