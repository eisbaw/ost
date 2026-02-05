//! TUI module for Teams CLI
//!
//! Terminal user interface using Ratatui.

mod app;
mod backend;
mod compose;
mod debug_log;
mod help;
mod log_capture;
mod messages;
mod search;
mod sidebar;
mod ui;

pub use app::run;
pub use log_capture::LogBuffer;
