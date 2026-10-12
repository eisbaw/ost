//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod graph;
mod me;
mod planner;
mod presence;
mod recordings;
mod teams;
mod transcripts;

use anyhow::Result;

// Re-export data types for TUI integration
pub use chat::{ChatInfo, MessageInfo};
pub use me::UserInfo;
pub use planner::{BucketInfo, PlanInfo, PlannerTaskInfo};
pub use presence::PresenceInfo;
pub use recordings::{
    clamp_limit as recordings_clamp_limit, is_video as is_recording_video,
    list_recordings_data, parse_recordings_response, recordings_children_path,
    recordings_search_path, search_recordings_data, sort_newest as sort_recordings_newest,
    RecordingInfo, RecordingSource, RECORDINGS_MAX_LIMIT,
};
pub use teams::TeamInfo;
pub use transcripts::{
    clamp_limit as transcripts_clamp_limit, is_transcript,
    list_transcripts_data, parse_transcripts_response, search_transcripts_data,
    sort_newest as sort_transcripts_newest, transcripts_children_path,
    transcripts_search_path, TranscriptInfo, TranscriptSource, TRANSCRIPTS_MAX_LIMIT,
};

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use planner::{
    buckets_path, create_task_body, create_task_data, list_buckets_data, list_plans_data,
    list_tasks_data, parse_buckets, parse_plans, parse_task, parse_tasks, plans_path,
    set_complete_body, set_task_complete_data, task_path, tasks_path,
};
pub use presence::get_presence_data;
pub use teams::list_teams_data;

/// List recent chats (native Teams API)
pub async fn list_chats(limit: usize) -> Result<()> {
    chat::list_chats(limit).await
}

/// Read messages from a chat (native Teams API)
pub async fn read_messages(chat_id: &str, limit: usize) -> Result<()> {
    chat::read_messages(chat_id, limit).await
}

/// Send a message to a chat (native Teams API)
pub async fn send_message(to: &str, message: &str) -> Result<()> {
    chat::send_message(to, message).await
}

/// Get current presence status
pub async fn get_presence() -> Result<()> {
    presence::get_presence().await
}

/// Set presence status
pub async fn set_presence(status: &str) -> Result<()> {
    presence::set_presence(status).await
}

/// Show current user info
pub async fn whoami() -> Result<()> {
    me::whoami().await
}

/// List joined teams and their channels
pub async fn list_teams() -> Result<()> {
    teams::list_teams().await
}
