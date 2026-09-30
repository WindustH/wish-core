//! Executes the model and tool actions the Session state machine selects: one run at a time, on an
//! `ExecutionControl` that interrupts and shutdown cancel, with context compaction between actions.
pub mod compaction;
mod control;
pub mod model;
mod observe;
mod run;
pub mod tool;

pub use control::ExecutionControl;
pub(crate) use observe::deliver_new_events;
#[allow(unused_imports)] // wish-test
pub use run::run;
pub use run::{BoundaryResult, RunBoundary, run_with_boundary};
