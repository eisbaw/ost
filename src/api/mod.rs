//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod graph;
mod me;
pub mod media;
mod presence;
mod teams;

use anyhow::Result;

// Re-export data types for TUI integration
pub use chat::{ChatInfo, MessageInfo, MessagesPage, ReactionCount, REACTION_EMOJI};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;

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
