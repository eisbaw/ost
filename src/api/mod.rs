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
pub use chat::{ChatInfo, MessageInfo, MessagesPage};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;

// Re-export ChannelInfo for use in TUI sidebar (currently consumed
// only through TeamInfo.channels, but kept public for future callers).
#[allow(unused_imports)]
pub use teams::ChannelInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{
    find_by_client_id, find_message_by_client_id, list_chats_data, new_client_message_id,
    read_messages_data, read_messages_page, send_message_with_client, send_message_with_client_id,
    sent_id_from_response, SentMessage,
};
// OstMac PRAISE: Praise card post.
pub use chat::{
    praise_card_content, praise_message_body, send_praise_with_client_id,
    ADAPTIVE_CARD_CONTENT_TYPE, PRAISE_APP_ID,
};
// OstMac stickers: native AMS-upload sticker send + detect.
pub use chat::{
    is_sticker_html, parse_sticker_object_id, png_dimensions,
    send_sticker_message_with_client_id, sticker_content, sticker_create_body,
    sticker_effective_label, sticker_label, sticker_message_body, sticker_view_url,
    upload_sticker_image_data, STICKER_DEFAULT_LABEL, STICKER_EDGE_MAX, STICKER_IMAGE_MAX_BYTES,
    STICKER_ITEMTYPE,
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

/// Send a local PNG file as a sticker (AMS upload + sticker message)
pub async fn send_sticker(to: &str, png_path: &str, label: &str) -> Result<()> {
    chat::send_sticker(to, png_path, label).await
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
