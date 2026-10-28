//! Built-in tool implementations. Register their specifications in SessionConfig.tools and
//! supply the implementation (or an application dispatcher) to executor::run.
pub mod ask_user;
pub mod history;
pub mod shell;
pub mod view_image;
pub mod web_search;

pub mod web_fetch;

pub(crate) mod arguments;

pub mod contract;
pub mod task_output;

pub mod image_generate;
