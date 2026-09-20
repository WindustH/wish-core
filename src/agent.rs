//! Executes model and tool effects selected by the session state machine.
mod control;
mod model_caller;
mod run;
mod tool;
pub use control::RunControl;
pub use model_caller::{ModelCaller, ModelStream};
pub use run::run;
pub use tool::{ToolCall, ToolExecutor, ToolOutcome};
