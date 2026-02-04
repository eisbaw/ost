//! TUI module for Teams CLI
//!
//! Terminal user interface using Ratatui.

mod app;
mod backend;
mod compose;
mod help;
mod messages;
mod search;
mod sidebar;
mod ui;

pub use app::run;
