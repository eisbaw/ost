//! OstMac library surface over the vendored `ost` (teams-cli) sources.
//!
//! Same modules as the binary, plus [`event_hub`] (OstMac patch) which lets
//! embedders subscribe to Trouter push events instead of only printing them.

pub mod api;
pub mod auth;
pub mod calling;
pub mod config;
pub mod event_hub;
pub mod models;
pub mod trouter;
pub mod tui;
