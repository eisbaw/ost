//! Native Teams chat API (chatsvcagg / chat service)
//!
//! Uses the Skype token with `Authentication: skypetoken={token}` header,
//! bypassing Graph API which requires tenant admin consent for Chat.Read.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

// -- Response types for the native chat API --

#[derive(Debug, Deserialize)]
struct ConversationsResponse {
    conversations: Option<Vec<Conversation>>,
}

#[derive(Debug, Deserialize)]
struct Conversation {
    id: Option<String>,
    #[serde(rename = "threadProperties")]
    thread_properties: Option<ThreadProperties>,
    #[serde(rename = "lastMessage")]
    last_message: Option<NativeMessage>,
}

#[derive(Debug, Deserialize)]
struct ThreadProperties {
    topic: Option<String>,
    #[serde(rename = "lastjoinat")]
    last_join_at: Option<String>,
    /// For 1:1 chats, contains member MRIs
    members: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeMessage {
    id: Option<String>,
    #[serde(rename = "composetime")]
    compose_time: Option<String>,
    #[serde(rename = "originalarrivaltime")]
    original_arrival_time: Option<String>,
    #[serde(rename = "imdisplayname")]
    im_display_name: Option<String>,
    content: Option<String>,
    messagetype: Option<String>,
    from: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    messages: Option<Vec<NativeMessage>>,
    #[serde(rename = "_metadata")]
    metadata: Option<MessagesMetadata>,
}

#[derive(Debug, Deserialize)]
struct MessagesMetadata {
    #[serde(rename = "backwardLink")]
    backward_link: Option<String>,
}

/// Strip HTML tags from content for CLI display.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Display name for a conversation.
fn conversation_name(conv: &Conversation) -> String {
    if let Some(ref props) = conv.thread_properties {
        if let Some(ref topic) = props.topic {
            if !topic.is_empty() {
                return topic.clone();
            }
        }
    }
    // Fall back to last message sender or the thread ID
    if let Some(ref msg) = conv.last_message {
        if let Some(ref name) = msg.im_display_name {
            if !name.is_empty() {
                return name.clone();
            }
        }
    }
    conv.id.as_deref().unwrap_or("[unknown]").to_string()
}

/// List recent chats using the native Teams API (prints to stdout).
pub async fn list_chats(limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let chats = list_chats_data(&client, limit).await?;

    println!("\nRecent Chats:");
    println!("{:-<60}", "");

    if chats.is_empty() {
        println!("  (no chats found)");
        return Ok(());
    }

    for chat in &chats {
        println!("{}", chat.name);
        println!("  ID: {}", chat.id);

        if let Some(ref time) = chat.last_message_time {
            println!("  Last: {}", time);
        }
        if let Some(ref preview) = chat.last_message_preview {
            if !preview.trim().is_empty() {
                let sender = chat.last_message_sender.as_deref().unwrap_or("?");
                println!("  [{}]: {}", sender, preview.trim());
            }
        }

        println!();
    }

    Ok(())
}

/// Read messages from a specific chat thread (prints to stdout).
pub async fn read_messages(chat_id: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let msgs = read_messages_data(&client, chat_id, limit).await?;

    if msgs.is_empty() {
        println!("(no messages)");
        return Ok(());
    }

    for msg in &msgs {
        println!("[{}] {}: {}", msg.timestamp, msg.sender, msg.content);
    }

    Ok(())
}

/// Send a message to a chat thread using the native API.
pub async fn send_message(chat_id: &str, message: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    send_message_with_client(&client, chat_id, message).await?;
    println!("Message sent.");
    Ok(())
}

/// HTML-escape text for embedding in Teams RichText/Html messages.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Send a message using an existing client (shared helper).
pub async fn send_message_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message: &str,
) -> Result<()> {
    let base = client.chat_service_url();
    let url = format!("{}/v1/users/ME/conversations/{}/messages", base, chat_id);

    let escaped = html_escape(message);
    let body = serde_json::json!({
        "content": format!("<p>{}</p>", escaped),
        "messagetype": "RichText/Html",
        "contenttype": "text"
    });

    tracing::debug!("Sending message to {}", url);
    client.chat_post(&url, &body).await?;
    Ok(())
}

/// Per-message URL for edits and deletes (pure so embedders/tests pin it).
pub fn message_url(base: &str, chat_id: &str, message_id: &str) -> String {
    format!(
        "{}/v1/users/ME/conversations/{}/messages/{}",
        base, chat_id, message_id
    )
}

/// Edit body for the native chat API. `skypeeditedid` carries the original
/// id so receivers (and our realtime parser) classify it as an edit.
pub fn edit_message_body(message_id: &str, text: &str) -> serde_json::Value {
    let escaped = html_escape(text);
    serde_json::json!({
        "content": format!("<p>{}</p>", escaped),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "skypeeditedid": message_id,
    })
}

/// Edit one own message's text via PUT (prints to stdout).
pub async fn edit_message(chat_id: &str, message_id: &str, text: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    edit_message_with_client(&client, chat_id, message_id, text).await?;
    println!("Message edited.");
    Ok(())
}

/// Edit one own message using an existing client (shared helper).
pub async fn edit_message_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message_id: &str,
    text: &str,
) -> Result<()> {
    let base = client.chat_service_url();
    let url = message_url(&base, chat_id, message_id);
    let body = edit_message_body(message_id, text);
    tracing::debug!("Editing message at {}", url);
    client.chat_put(&url, &body).await?;
    Ok(())
}

/// Delete one own message via DELETE (prints to stdout).
pub async fn delete_message(chat_id: &str, message_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    delete_message_with_client(&client, chat_id, message_id).await?;
    println!("Message deleted.");
    Ok(())
}

/// Delete one own message using an existing client (shared helper).
pub async fn delete_message_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message_id: &str,
) -> Result<()> {
    let base = client.chat_service_url();
    let url = message_url(&base, chat_id, message_id);
    tracing::debug!("Deleting message at {}", url);
    client.chat_delete(&url, None).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Data-returning API functions for TUI integration
// ---------------------------------------------------------------------------

/// Chat metadata for TUI display.
#[allow(dead_code)]
pub struct ChatInfo {
    pub id: String,
    pub name: String,
    pub is_group: bool,
    pub last_message_time: Option<String>,
    pub last_message_sender: Option<String>,
    pub last_message_preview: Option<String>,
}

/// A single message for TUI display.
pub struct MessageInfo {
    /// Server message id; embedders match realtime edits by this.
    /// OstMac: synthetic `timestamp@sender` fallback when the server omits it.
    pub id: String,
    pub sender: String,
    pub timestamp: String,
    pub content: String,
    /// Unstripped server HTML (om-convrich: embedders mine `<at>` mentions
    /// and `<pre>` code blocks from it; `content` stays the stripped text).
    pub raw: String,
}

/// One page of history plus the cursor for the next older page.
pub struct MessagesPage {
    pub messages: Vec<MessageInfo>,
    /// Server `_metadata.backwardLink`: full URL of the next older page,
    /// or None when history is exhausted / the server omits metadata.
    pub backward_link: Option<String>,
}

/// List recent chats and return structured data.
pub async fn list_chats_data(client: &TeamsClient, limit: usize) -> Result<Vec<ChatInfo>> {
    // Strategy 1: CSA AFD endpoint with Bearer auth
    let csa_url = format!(
        "https://teams.microsoft.com/api/csa/api/v1/teams/users/ME/conversations?view=mychats&pageSize={}",
        limit
    );
    tracing::debug!("Trying CSA endpoint: {}", csa_url);
    let resp = match client.csa_get(&csa_url).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("CSA endpoint failed: {:#}, trying chatsvcagg", e);
            // Strategy 2: chatsvcagg with skypetoken auth
            let base = client.chatsvcagg_url();
            let url = format!(
                "{}/api/v2/users/ME/conversations?view=mychats&pageSize={}",
                base, limit
            );
            tracing::debug!("Trying chatsvcagg: {}", url);
            match client.chat_get(&url).await {
                Ok(r) => r,
                Err(e2) => {
                    tracing::debug!("chatsvcagg failed: {:#}, trying chat service", e2);
                    // Strategy 3: chat service (amer.ng.msg) with skypetoken auth
                    let base = client.chat_service_url();
                    let url = format!(
                        "{}/v1/users/ME/conversations?view=mychats&pageSize={}",
                        base, limit
                    );
                    client.chat_get(&url).await?
                }
            }
        }
    };

    let body: ConversationsResponse = resp
        .json()
        .await
        .context("Failed to parse conversations response")?;

    let conversations = body.conversations.unwrap_or_default();

    let mut chats = Vec::new();
    for conv in &conversations {
        let id = conv.id.as_deref().unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }

        let name = conversation_name(conv);
        let is_group = id.contains("thread") || id.contains("meeting");

        let (last_time, last_sender, last_preview) = if let Some(ref msg) = conv.last_message {
            let time = msg
                .original_arrival_time
                .as_deref()
                .or(msg.compose_time.as_deref())
                .map(String::from);
            let sender = msg.im_display_name.clone();
            let preview = msg.content.as_deref().map(|c| {
                let text = strip_html(c);
                if text.len() > 80 {
                    let end = text
                        .char_indices()
                        .map(|(i, _)| i)
                        .take_while(|&i| i <= 77)
                        .last()
                        .unwrap_or(0);
                    format!("{}...", &text[..end])
                } else {
                    text
                }
            });
            (time, sender, preview)
        } else {
            (None, None, None)
        };

        chats.push(ChatInfo {
            id,
            name,
            is_group,
            last_message_time: last_time,
            last_message_sender: last_sender,
            last_message_preview: last_preview,
        });
    }

    Ok(chats)
}

/// Read messages from a specific chat thread and return structured data.
///
/// Newest page only; use [`read_messages_page`] with the returned
/// `backward_link` to walk older history.
pub async fn read_messages_data(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<MessageInfo>> {
    Ok(read_messages_page(client, chat_id, limit, None)
        .await?
        .messages)
}

/// Read one page of history. `page_url` is None for the newest page or
/// Some(previous `backward_link`) for the next older page. Messages come
/// back oldest-first; pages never overlap (verified live 2026-09-22).
pub async fn read_messages_page(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
    page_url: Option<&str>,
) -> Result<MessagesPage> {
    let url = match page_url {
        Some(u) => with_page_size(u, limit),
        None => {
            let base = client.chat_service_url();
            format!(
                "{}/v1/users/ME/conversations/{}/messages?pageSize={}",
                base, chat_id, limit
            )
        }
    };

    tracing::debug!("Reading messages from {}", url);
    let resp = client.chat_get(&url).await?;
    let body: MessagesResponse = resp
        .json()
        .await
        .context("Failed to parse messages response")?;

    let messages = body.messages.unwrap_or_default();

    // Messages come newest-first; reverse for chronological display
    let mut msgs: Vec<&NativeMessage> = messages.iter().collect();
    msgs.reverse();

    let mut result = Vec::new();
    for msg in &msgs {
        let msgtype = msg.messagetype.as_deref().unwrap_or("");
        // Skip non-text messages (e.g. ThreadActivity/*)
        if !msgtype.contains("Text") && !msgtype.contains("RichText") {
            continue;
        }
        // OstMac om-conv: skip media payloads. RichText/Media_CallRecording
        // strips to "TitlePlay" fragments and RichText/Media_CallTranscript
        // to raw JSON; neither is a readable bubble (see task-0011).
        if msgtype.contains("Media_") {
            continue;
        }

        let sender = msg
            .im_display_name
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("?")
            .to_string();
        let time = msg
            .original_arrival_time
            .as_deref()
            .or(msg.compose_time.as_deref())
            .unwrap_or("")
            .to_string();
        let content = msg.content.as_deref().unwrap_or("");
        let text = strip_html(content);

        // OstMac om-richmedia: image-only bubbles strip to "" but are
        // real messages — keep them (the embedder mines `<img>` from raw).
        if text.trim().is_empty() && !has_image(content) {
            continue;
        }

        // OstMac: keep the server id so embedders can match realtime edits.
        let id = msg.id.as_deref().filter(|s| !s.is_empty()).map(String::from);
        let id = id.unwrap_or_else(|| format!("{}@{}", time, sender));
        result.push(MessageInfo {
            id,
            sender,
            timestamp: time,
            content: text.trim().to_string(),
            raw: content.to_string(),
        });
    }

    let backward_link = body.metadata.and_then(|m| m.backward_link);
    Ok(MessagesPage {
        messages: result,
        backward_link,
    })
}

/// True when raw HTML carries an `<img` tag (case-insensitive).
/// Image-only messages strip to empty text but must survive filtering.
fn has_image(html: &str) -> bool {
    html.as_bytes()
        .windows(4)
        .any(|w| w.eq_ignore_ascii_case(b"<img"))
}

/// Rewrite the `pageSize=` query value so a followed `backwardLink` honors
/// the caller's limit. No-op when the marker is absent.
fn with_page_size(url: &str, limit: usize) -> String {
    const MARK: &str = "pageSize=";
    let Some(start) = url.find(MARK) else {
        return url.to_string();
    };
    let val_start = start + MARK.len();
    let val_end = url[val_start..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| val_start + i)
        .unwrap_or(url.len());
    format!("{}{}{}", &url[..val_start], limit, &url[val_end..])
}

// ---------------------------------------------------------------------------
// Chat list actions: mute, hide, folders
// ---------------------------------------------------------------------------
//
// Mute: the per-user conversation property `alerts` on the chat service
// (`"false"` = muted, `"true"` = notify), the same property the chat list
// returns under `properties.alerts`. Hide: Graph v1.0
// `POST /chats/{id}/hideForUser` / `unhideForUser`. Folders: the chat
// service aggregator's `conversationFolders` (read-only here), which needs
// an AAD token for `https://chatsvcagg.teams.microsoft.com`.

/// PUT target for one conversation's `alerts` property.
pub fn alerts_url(base: &str, chat_id: &str) -> String {
    format!(
        "{}/v1/users/ME/conversations/{}/properties?name=alerts",
        base, chat_id
    )
}

/// PUT body: muted chats carry `"false"` (the server stores strings).
pub fn alerts_body(muted: bool) -> serde_json::Value {
    serde_json::json!({ "alerts": if muted { "false" } else { "true" } })
}

/// Muted state from a conversation's `properties` object: `Some(true)`
/// for `alerts: "false"`, `Some(false)` for `"true"`, None when absent.
pub fn alerts_muted(properties: &serde_json::Value) -> Option<bool> {
    match properties.get("alerts")?.as_str()?.trim().to_ascii_lowercase().as_str() {
        "false" => Some(true),
        "true" => Some(false),
        _ => None,
    }
}

/// Mute or unmute one chat for the signed-in user.
pub async fn set_chat_muted_with_client(client: &TeamsClient, chat_id: &str, muted: bool) -> Result<()> {
    let id = chat_id.trim();
    if id.is_empty() {
        bail!("empty chat_id");
    }
    let url = alerts_url(&client.chat_service_url(), id);
    client.chat_put(&url, &alerts_body(muted)).await?;
    Ok(())
}

/// Graph path for hiding (`hideForUser`) or showing (`unhideForUser`).
pub fn hide_chat_path(chat_id: &str, hidden: bool) -> String {
    let verb = if hidden { "hideForUser" } else { "unhideForUser" };
    format!("/chats/{}/{}", chat_id.trim(), verb)
}

/// Graph body naming the user the chat is hidden for.
pub fn hide_chat_body(user_id: &str, tenant_id: &str) -> serde_json::Value {
    serde_json::json!({
        "user": {
            "@odata.type": "#microsoft.graph.teamworkUserIdentity",
            "id": user_id.trim(),
            "tenantId": tenant_id.trim(),
        }
    })
}

/// Hide or unhide one chat for the signed-in user (`user_id` = Entra
/// object id, `tenant_id` = home tenant).
pub async fn set_chat_hidden_with_client(
    client: &TeamsClient,
    chat_id: &str,
    user_id: &str,
    tenant_id: &str,
    hidden: bool,
) -> Result<()> {
    if chat_id.trim().is_empty() {
        bail!("empty chat_id");
    }
    if user_id.trim().is_empty() || tenant_id.trim().is_empty() {
        bail!("missing user identity");
    }
    client
        .graph_post(&hide_chat_path(chat_id, hidden), &hide_chat_body(user_id, tenant_id))
        .await?;
    Ok(())
}

/// Chat service aggregator resource for folder reads.
pub const CHATSVCAGG_SCOPE: &str = "https://chatsvcagg.teams.microsoft.com/.default";

/// Folder list URL (system folders included so Favorites comes back).
pub fn conversation_folders_url() -> String {
    "https://teams.microsoft.com/api/csa/api/v1/teams/users/me/conversationFolders?supportsAdditionalSystemGeneratedFolders=true&supportsSliceItems=true".to_string()
}

/// Server folder types that are views, not folders a chat is moved into.
pub const SYSTEM_FOLDER_TYPES: &[&str] = &[
    "RecentChats",
    "TeamsAndChannels",
    "QuickViews",
    "MutedChats",
    "MeetingChats",
    "EngageCommunities",
];

/// One chat folder from the server: Favorites and user folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationFolder {
    pub id: String,
    pub name: String,
    pub folder_type: String,
    /// Conversation ids in folder order (chats and channels alike).
    pub item_ids: Vec<String>,
}

/// Parse a `conversationFolders` payload: deleted and system folders
/// dropped, blank ids dropped, `conversationFolderOrder` order first
/// (folders it omits keep payload order after it).
pub fn parse_conversation_folders(v: &serde_json::Value) -> Vec<ConversationFolder> {
    let mut out: Vec<ConversationFolder> = Vec::new();
    for f in v.get("conversationFolders").and_then(|x| x.as_array()).into_iter().flatten() {
        let s = |k: &str| f.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        let (id, folder_type) = (s("id"), s("folderType"));
        if id.is_empty() || f.get("isDeleted").and_then(|x| x.as_bool()).unwrap_or(false) {
            continue;
        }
        if SYSTEM_FOLDER_TYPES.iter().any(|t| t.eq_ignore_ascii_case(&folder_type)) {
            continue;
        }
        let mut name = s("name");
        if name.is_empty() {
            name = folder_type.clone();
        }
        let item_ids = f
            .get("conversationFolderItems")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("conversationId").and_then(|x| x.as_str()))
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        out.push(ConversationFolder { id, name, folder_type, item_ids });
    }
    let order: Vec<&str> = v
        .get("conversationFolderOrder")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str())
        .collect();
    out.sort_by_key(|f| order.iter().position(|o| *o == f.id).unwrap_or(usize::MAX));
    out
}

/// Read the signed-in user's chat folders with a chatsvcagg bearer token.
pub async fn conversation_folders_data(bearer: &str) -> Result<Vec<ConversationFolder>> {
    if bearer.trim().is_empty() {
        bail!("no chat service aggregator token");
    }
    let url = conversation_folders_url();
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(bearer)
        .header("x-ms-client-version", "1415/24080616421")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("conversationFolders GET failed")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("conversationFolders GET: {}", status);
    }
    let v: serde_json::Value = resp.json().await.context("Failed to parse conversationFolders")?;
    Ok(parse_conversation_folders(&v))
}

/// One folder edit: `("AddItem" | "RemoveItem", folder id, conversation id)`.
pub type FolderAction = (&'static str, String, String);

/// Pure: the edits that leave `chat_id` in `target` (blank = no folder)
/// and in no other movable folder, from a raw `conversationFolders`
/// payload. Returns `(folderHierarchyVersion, actions)`; no actions when
/// the chat is already where it should be. System folders (views) are
/// never edited; an unknown target is an error.
pub fn folder_move_actions(
    v: &serde_json::Value,
    chat_id: &str,
    target: &str,
) -> Result<(i64, Vec<FolderAction>)> {
    let version = v.get("folderHierarchyVersion").and_then(|x| x.as_i64()).unwrap_or(0);
    let folders = parse_conversation_folders(v);
    let target = target.trim();
    if !target.is_empty() && !folders.iter().any(|f| f.id == target) {
        bail!("unknown folder");
    }
    let mut actions: Vec<FolderAction> = Vec::new();
    for f in &folders {
        let holds = f.item_ids.iter().any(|i| i == chat_id);
        if holds && f.id != target {
            actions.push(("RemoveItem", f.id.clone(), chat_id.to_string()));
        }
        if !holds && f.id == target {
            actions.push(("AddItem", f.id.clone(), chat_id.to_string()));
        }
    }
    Ok((version, actions))
}

/// Folder edit POST body (same URL as the folder GET).
pub fn folder_move_body(version: i64, actions: &[FolderAction]) -> serde_json::Value {
    let list: Vec<serde_json::Value> = actions
        .iter()
        .map(|(action, folder, item)| {
            serde_json::json!({"action": action, "folderId": folder, "itemId": item})
        })
        .collect();
    serde_json::json!({"folderHierarchyVersion": version, "actions": list})
}

async fn conversation_folders_raw(bearer: &str) -> Result<serde_json::Value> {
    let resp = reqwest::Client::new()
        .get(conversation_folders_url())
        .bearer_auth(bearer)
        .header("x-ms-client-version", "1415/24080616421")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("conversationFolders GET failed")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("conversationFolders GET: {}", status);
    }
    resp.json().await.context("Failed to parse conversationFolders")
}

/// Move one chat into `target` (blank = out of every folder): fresh GET
/// for the version and membership, one POST with the edits, then check
/// the returned folders. Returns the folders after the move; an answer
/// that does not show the chat where it was asked to go is an error.
pub async fn conversation_folder_move_with_client(
    client: &TeamsClient,
    bearer: &str,
    chat_id: &str,
    target: &str,
) -> Result<Vec<ConversationFolder>> {
    let chat_id = chat_id.trim();
    if chat_id.is_empty() || bearer.trim().is_empty() {
        bail!("missing chat id or token");
    }
    let current = conversation_folders_raw(bearer).await?;
    let (version, actions) = folder_move_actions(&current, chat_id, target)?;
    if actions.is_empty() {
        return Ok(parse_conversation_folders(&current));
    }
    let skype = client.skype_token()?;
    let resp = reqwest::Client::new()
        .post(conversation_folders_url())
        .bearer_auth(bearer)
        .header("Authentication", format!("skypetoken={}", skype))
        .header("x-ms-client-version", "1415/24080616421")
        .json(&folder_move_body(version, &actions))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("conversationFolders POST failed")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("conversationFolders POST: {}", status);
    }
    // The answer carries the folder state; re-read when it does not.
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    let after = if body.get("conversationFolders").is_some() {
        body
    } else {
        conversation_folders_raw(bearer).await?
    };
    let (_, left) = folder_move_actions(&after, chat_id, target)?;
    if !left.is_empty() {
        bail!("folder move not applied");
    }
    Ok(parse_conversation_folders(&after))
}

#[cfg(test)]
mod chatmenu_tests {
    use super::*;

    fn folders_payload() -> serde_json::Value {
        serde_json::json!({
            "folderHierarchyVersion": 7,
            "conversationFolders": [
                {"id": "t~u~Favorites", "folderType": "Favorites",
                 "conversationFolderItems": [{"conversationId": "19:a@thread.v2"}]},
                {"id": "f1", "name": "Work", "folderType": "UserCreated",
                 "conversationFolderItems": [{"conversationId": "48:notes"}]},
                {"id": "q", "folderType": "QuickViews",
                 "conversationFolderItems": [{"conversationId": "48:notes"}]}
            ]
        })
    }

    #[test]
    fn folder_move_request_shape() {
        let v = folders_payload();
        let (version, actions) = folder_move_actions(&v, "48:notes", "t~u~Favorites").unwrap();
        assert_eq!(version, 7);
        // Out of the user folder, into Favorites; the QuickViews view is untouched.
        assert_eq!(
            actions,
            vec![
                ("AddItem", "t~u~Favorites".to_string(), "48:notes".to_string()),
                ("RemoveItem", "f1".to_string(), "48:notes".to_string()),
            ]
        );
        assert_eq!(
            folder_move_body(version, &actions),
            serde_json::json!({"folderHierarchyVersion": 7, "actions": [
                {"action": "AddItem", "folderId": "t~u~Favorites", "itemId": "48:notes"},
                {"action": "RemoveItem", "folderId": "f1", "itemId": "48:notes"}
            ]})
        );
        // Blank target = out of every folder; already-there = no edits.
        let (_, out) = folder_move_actions(&v, "48:notes", "").unwrap();
        assert_eq!(out, vec![("RemoveItem", "f1".to_string(), "48:notes".to_string())]);
        let (_, none) = folder_move_actions(&v, "19:a@thread.v2", "t~u~Favorites").unwrap();
        assert!(none.is_empty());
        // System views and unknown ids are not move targets.
        assert!(folder_move_actions(&v, "48:notes", "q").is_err());
        assert!(folder_move_actions(&v, "48:notes", "nope").is_err());
    }

    #[test]
    fn alerts_request_shape() {
        assert_eq!(
            alerts_url("https://h", "19:a@thread.v2"),
            "https://h/v1/users/ME/conversations/19:a@thread.v2/properties?name=alerts"
        );
        assert_eq!(alerts_body(true), serde_json::json!({"alerts": "false"}));
        assert_eq!(alerts_body(false), serde_json::json!({"alerts": "true"}));
        assert_eq!(alerts_muted(&serde_json::json!({"alerts": "false"})), Some(true));
        assert_eq!(alerts_muted(&serde_json::json!({"alerts": "True"})), Some(false));
        assert_eq!(alerts_muted(&serde_json::json!({"favorite": "true"})), None);
    }

    #[test]
    fn hide_request_shape() {
        assert_eq!(hide_chat_path(" 19:a@thread.v2 ", true), "/chats/19:a@thread.v2/hideForUser");
        assert_eq!(hide_chat_path("19:a@thread.v2", false), "/chats/19:a@thread.v2/unhideForUser");
        let b = hide_chat_body("oid-1", "tid-2");
        assert_eq!(b["user"]["id"], "oid-1");
        assert_eq!(b["user"]["tenantId"], "tid-2");
        assert_eq!(b["user"]["@odata.type"], "#microsoft.graph.teamworkUserIdentity");
    }

    #[test]
    fn folders_parse_drops_system_and_deleted_and_orders() {
        let v = serde_json::json!({
            "conversationFolderOrder": ["f-work", "f-fav", "f-recent"],
            "conversationFolders": [
                {"id": "f-fav", "name": "Favorites", "folderType": "Favorites",
                 "conversationFolderItems": [{"conversationId": "19:a@thread.v2"}, {"conversationId": " "}]},
                {"id": "f-recent", "name": "Chats", "folderType": "RecentChats", "conversationFolderItems": []},
                {"id": "f-gone", "name": "Old", "folderType": "UserCreated", "isDeleted": true},
                {"id": "f-work", "name": "Work", "folderType": "UserCreated",
                 "conversationFolderItems": [{"conversationId": "19:b@unq.gbl.spaces"}]},
                {"id": "", "name": "Blank", "folderType": "UserCreated"}
            ]
        });
        let f = parse_conversation_folders(&v);
        assert_eq!(f.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["f-work", "f-fav"]);
        assert_eq!(f[0].item_ids, ["19:b@unq.gbl.spaces"]);
        assert_eq!(f[1].item_ids, ["19:a@thread.v2"]);
        assert!(parse_conversation_folders(&serde_json::json!({})).is_empty());
        assert!(conversation_folders_url().contains("/conversationFolders?"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_url_and_body_shape() {
        assert_eq!(
            message_url("https://h", "19:chat", "123"),
            "https://h/v1/users/ME/conversations/19:chat/messages/123"
        );
        let b = edit_message_body("123", "a<b>&\"'");
        assert_eq!(b["messagetype"], "RichText/Html");
        assert_eq!(b["contenttype"], "text");
        assert_eq!(b["skypeeditedid"], "123");
        assert_eq!(
            b["content"],
            "<p>a&lt;b&gt;&amp;&quot;&#39;</p>"
        );
    }

    #[test]
    fn image_tag_detection() {
        assert!(has_image(r#"<p><img src="https://h/v1/objects/0/views/imgo"></p>"#));
        assert!(has_image(r#"<IMG SRC="https://h/x.png">"#));
        assert!(has_image(r#"<p>hi <img
src="x">"#));
        assert!(!has_image("<p>plain text</p>"));
        assert!(!has_image("<p>image word, no tag</p>"));
        assert!(!has_image(""));
    }

    #[test]
    fn page_size_rewrite_mid_and_end() {
        assert_eq!(
            with_page_size("https://h/m?pageSize=2&view=x", 50),
            "https://h/m?pageSize=50&view=x"
        );
        assert_eq!(
            with_page_size("https://h/m?view=x&pageSize=2", 50),
            "https://h/m?view=x&pageSize=50"
        );
        assert_eq!(with_page_size("https://h/m", 50), "https://h/m");
    }

    #[test]
    fn metadata_backward_link_parses() {
        let body: MessagesResponse = serde_json::from_str(
            r#"{"messages":[],"_metadata":{"backwardLink":"https://h/back"},"tenantId":"t"}"#,
        )
        .unwrap();
        assert_eq!(
            body.metadata.unwrap().backward_link.as_deref(),
            Some("https://h/back")
        );
        let bare: MessagesResponse = serde_json::from_str(r#"{"messages":[]}"#).unwrap();
        assert!(bare.metadata.is_none());
    }
}
