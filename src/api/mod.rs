//! API client module for Microsoft Teams

pub mod apps;
mod chat;
pub mod client;
mod graph;
mod me;
mod presence;
mod tabs;
mod teams;

use anyhow::Result;

// Re-export data types for TUI integration
pub use apps::{
    app_catalog_data, app_search_data, app_store_data, install_app_for_user, sharepoint_sites_data,
    team_app_definitions_data, AppCatalog, AppEntitlement, AppManifest, AppStore, ConfigurableTab, SharePointSites,
    StaticTab, StoreSection, WebApplicationInfo,
};
pub use chat::{ChatInfo, MessageInfo};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use tabs::TabInfo;
pub use teams::TeamInfo;

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use presence::get_presence_data;
pub use tabs::{
    add_channel_tab_data, add_chat_tab_data, list_chat_tabs_data, list_tabs_data,
    remove_channel_tab_data, remove_chat_tab_data, rename_channel_tab_data, rename_chat_tab_data,
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

/// List a channel's pinned tabs (read-only)
pub async fn list_tabs(channel_id: &str) -> Result<()> {
    tabs::list_tabs(channel_id).await
}

/// Dump every joined channel's tabs (one auth, read-only)
pub async fn list_tabs_all() -> Result<()> {
    tabs::list_tabs_all().await
}

/// Add a website/app tab to a channel (prints the created tab)
pub async fn add_channel_tab(
    channel_id: &str,
    name: &str,
    app: Option<&str>,
    url: Option<&str>,
) -> Result<()> {
    tabs::add_channel_tab(channel_id, name, app, url).await
}

/// Rename a channel tab (prints the updated tab)
pub async fn rename_channel_tab(channel_id: &str, tab_id: &str, name: &str) -> Result<()> {
    tabs::rename_channel_tab(channel_id, tab_id, name).await
}

/// Remove a channel tab
pub async fn remove_channel_tab(channel_id: &str, tab_id: &str) -> Result<()> {
    tabs::remove_channel_tab(channel_id, tab_id).await
}

/// Add a website/app tab to a chat (prints the created tab)
pub async fn add_chat_tab(
    chat_id: &str,
    name: &str,
    app: Option<&str>,
    url: Option<&str>,
) -> Result<()> {
    tabs::add_chat_tab(chat_id, name, app, url).await
}

/// Rename a chat tab (prints the updated tab)
pub async fn rename_chat_tab(chat_id: &str, tab_id: &str, name: &str) -> Result<()> {
    tabs::rename_chat_tab(chat_id, tab_id, name).await
}

/// Remove a chat tab
pub async fn remove_chat_tab(chat_id: &str, tab_id: &str) -> Result<()> {
    tabs::remove_chat_tab(chat_id, tab_id).await
}
