//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod graph;
mod me;
mod presence;
mod teams;
mod todo;

use anyhow::Result;

// Re-export data types for TUI integration
pub use chat::{ChatInfo, MessageInfo};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;
// Re-exported for future TUI callers; unused by the CLI today.
#[allow(unused_imports)]
pub use todo::{TodoListInfo, TodoTaskInfo};

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use presence::get_presence_data;
pub use teams::list_teams_data;
#[allow(unused_imports)]
pub use todo::{
    complete_todo_task_data, create_todo_task_data, list_todo_lists_data,
    list_todo_tasks_data,
};

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

/// List Microsoft To Do lists
pub async fn list_todo_lists() -> Result<()> {
    todo::list_todo_lists().await
}

/// List tasks in one To Do list
pub async fn list_todo_tasks(list_id: &str, limit: usize) -> Result<()> {
    todo::list_todo_tasks(list_id, limit).await
}

/// Create one task in a To Do list
pub async fn create_todo_task(list_id: &str, title: &str) -> Result<()> {
    todo::create_todo_task(list_id, title).await
}

/// Mark one To Do task completed
pub async fn complete_todo_task(list_id: &str, task_id: &str) -> Result<()> {
    todo::complete_todo_task(list_id, task_id).await
}
