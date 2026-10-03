//! API client module for Microsoft Teams

pub mod apps;
mod chat;
pub mod client;
mod files;
mod filesearch;
mod graph;
mod me;
pub mod media;
mod presence;
mod teams;

use anyhow::Result;

// Re-export data types for TUI integration
pub use apps::{app_catalog_data, AppCatalog, AppEntitlement, AppManifest, ConfigurableTab, StaticTab, WebApplicationInfo};
pub use chat::{ChatInfo, MessageInfo, MessagesPage, ReactionCount, REACTION_EMOJI};
// Re-exported for future TUI/file-browser callers; unused by the CLI today.
#[allow(unused_imports)]
pub use files::{FileVersion, SharedFile};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;
pub use teams::TeamMemberInfo;

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{
    delete_message_with_client, edit_message_body, edit_message_with_client,
    emoji_for_reaction_type, list_chats_data, message_url, reaction_add_body, reaction_add_url,
    reaction_remove_url, reaction_type_for_emoji, read_messages_data, read_messages_page,
    remove_reaction_with_client, send_message_with_client, send_reaction_with_client,
};
pub use media::{fetch_media_data, MediaBytes, MAX_BYTES};
pub use me::whoami_data;
pub use presence::get_presence_data;
pub use teams::{
    add_member_body, add_team_member_data, channel_react_body,
    channel_reply_set_reaction_path, channel_reply_unset_reaction_path,
    channel_set_reaction_path, channel_unset_reaction_path, create_channel_body,
    create_channel_data, create_channel_path, create_team_body, create_team_data,
    create_team_path, join_team_data, list_team_members_data, list_teams_data,
    member_path, members_path, operation_failed, operation_succeeded,
    operation_team_id, operation_url, remove_team_member_data,
    set_channel_reaction_data, standard_team_template, unset_channel_reaction_data,
    TeamCreateResult, TeamsAsyncOperation, TEAM_CREATE_POLL_SECS,
    TEAM_CREATE_TIMEOUT_SECS,
};
#[allow(unused_imports)]
pub use files::{
    copy_body, copy_file_data, create_link_data, delete_file_data, download_file_data,
    download_file_version_data, drive_item_path, folder_children_path, list_chat_files_data,
    list_chat_files_data_opts, list_file_versions_data, list_folder_children_data,
    move_body, move_file_data, rename_body, rename_file_data, restore_file_version_data,
    upload_file_data,
};
pub use filesearch::{
    clamp_limit, drive_search_path, parse_drive_search_response,
    parse_people_search_response, people_search_path, search_files_data,
    search_people_data, FIND_MAX_LIMIT,
};
pub use teams::{channel_path, delete_channel_data, update_channel_body, update_channel_data};

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

/// Edit one own message (native Teams API, PUT per-message URL)
pub async fn edit_message(chat_id: &str, message_id: &str, text: &str) -> Result<()> {
    chat::edit_message(chat_id, message_id, text).await
}

/// Delete one own message (native Teams API, DELETE per-message URL)
pub async fn delete_message(chat_id: &str, message_id: &str) -> Result<()> {
    chat::delete_message(chat_id, message_id).await
}

/// Add (or with `remove`, remove) an emoji reaction on one message.
pub async fn react(chat_id: &str, message_id: &str, emoji: &str, remove: bool) -> Result<()> {
    chat::react(chat_id, message_id, emoji, remove).await
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

/// Search OneDrive files by name/content (om-jb-filesearch)
pub async fn search_files(query: &str, limit: usize) -> Result<()> {
    filesearch::search_files(query, limit).await
}

/// Search the directory for people (om-jb-filesearch)
pub async fn search_people(query: &str, limit: usize) -> Result<()> {
    filesearch::search_people(query, limit).await
}

/// List one team's roster (members + owners; `owners_only` filters)
pub async fn list_team_members(team_id: &str, owners_only: bool) -> Result<()> {
    teams::list_team_members(team_id, owners_only).await
}

/// Add one user to a team (`owner` grants the owner role)
pub async fn add_team_member(team_id: &str, user: &str, owner: bool) -> Result<()> {
    teams::add_team_member(team_id, user, owner).await
}

/// Remove one membership from a team
pub async fn remove_team_member(team_id: &str, member_id: &str) -> Result<()> {
    teams::remove_team_member(team_id, member_id).await
}
