//! Built-in tool implementations. Register their specifications in SessionConfig.tools and
//! supply the implementation (or an application dispatcher) to executor::run.
pub mod search_history;
pub mod shell;
pub mod view_image;
