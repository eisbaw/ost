//! API client module for Microsoft Teams

mod calendar;
mod calweek;
mod chat;
pub mod client;
mod graph;
mod me;
mod presence;
mod schedule;
mod teams;

use anyhow::Result;

// Re-export data types for TUI integration
// Re-exported for future TUI callers; unused by the CLI today.
#[allow(unused_imports)]
pub use calendar::{JoinTarget, LobbyEvent, LobbyState, MeetingInfo};
pub use chat::{ChatInfo, MessageInfo};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
#[allow(unused_imports)]
pub use calendar::{
    calendar_view_path, list_upcoming_meetings_data, lobby_next, parse_calendar_view,
    parse_join_url,
};
pub use calweek::{
    calweek_view_path, cancel_meeting_data, list_week_meetings_data, parse_created_event,
    schedule_event_body, schedule_meeting_data, validate_schedule,
};
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use presence::get_presence_data;
pub use schedule::{
    list_schedule_data, list_shifts_data, list_timeoff_reasons_data, list_timesoffs_data,
    ScheduleInfo, ShiftInfo, TimeOffInfo, TimeOffReason,
};
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

/// List upcoming meetings (Graph calendarView, next 7 days)
pub async fn list_upcoming_meetings(limit: usize) -> Result<()> {
    calendar::list_upcoming_meetings(limit).await
}

/// Print one team's schedule week grid (shifts + time-off, read-only)
pub async fn list_shifts(team_id: &str) -> Result<()> {
    schedule::list_shifts(team_id).await
}
