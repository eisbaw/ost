//! TUI module for Teams CLI
//!
//! Terminal user interface using Ratatui.

mod app;
mod compose;
mod messages;
mod sidebar;
mod ui;

pub use app::run;
