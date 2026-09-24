//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod files;
mod graph;
mod me;
mod presence;
mod teams;

use anyhow::Result;

// Re-export data types for TUI integration
pub use chat::{ChatInfo, MessageInfo};
// Re-exported for future TUI/file-browser callers; unused by the CLI today.
#[allow(unused_imports)]
pub use files::{FileVersion, SharedFile};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
#[allow(unused_imports)]
pub use files::{
    create_link_data, download_file_data, download_file_version_data, folder_children_path,
    list_chat_files_data, list_chat_files_data_opts, list_file_versions_data,
    list_folder_children_data, restore_file_version_data, upload_file_data,
};
pub use me::whoami_data;
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

/// List shared files in a chat or channel
pub async fn list_files(chat_id: &str, limit: usize) -> Result<()> {
    files::list_files(chat_id, limit).await
}

/// Download a shared file by drive+item id
pub async fn download_file(drive_id: &str, item_id: &str, dest: &str) -> Result<()> {
    files::download_file(drive_id, item_id, dest).await
}

/// Upload a local file to a chat or channel
pub async fn upload_file(chat_id: &str, local_path: &str) -> Result<()> {
    files::upload_file(chat_id, local_path).await
}

/// Create a view-only sharing link for a shared file
pub async fn create_link(drive_id: &str, item_id: &str, scope: &str) -> Result<()> {
    files::create_link(drive_id, item_id, scope).await
}
