//! Native Teams chat API (chatsvcagg / chat service)
//!
//! Uses the Skype token with `Authentication: skypetoken={token}` header,
//! bypassing Graph API which requires tenant admin consent for Chat.Read.

use anyhow::{Context, Result};
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
// Read receipts (OstMac om-receipts lane)
// ---------------------------------------------------------------------------
//
// Wire shape mirrors the native chat service consumption horizon:
// PUT .../v1/users/ME/conversations/{id}/properties?name=consumptionhorizon
// with {"consumptionhorizon": "<t1>;<t2>;<messageId>"} marks read;
// GET .../v1/threads/{id}/consumptionhorizons lists peer positions.

/// One peer read position: user key + last-read message id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadReceipt {
    pub user: String,
    pub message_id: String,
    pub horizon: String,
}

/// PUT target for marking one conversation read up to a message.
pub fn consumptionhorizon_url(base: &str, chat_id: &str) -> String {
    format!(
        "{}/v1/users/ME/conversations/{}/properties?name=consumptionhorizon",
        base, chat_id
    )
}

/// GET target for listing peer read positions in one thread.
pub fn consumptionhorizons_url(base: &str, thread_id: &str) -> String {
    format!(
        "{}/v1/threads/{}/consumptionhorizons",
        base, thread_id
    )
}

/// Horizon value: "<now_ms>;<now_ms>;<message_id>". Both stamps are the
/// send time (server accepts equal stamps; the id is the read frontier).
pub fn consumptionhorizon_value(message_id: &str, now_ms: u64) -> String {
    format!("{};{};{}", now_ms, now_ms, message_id)
}

/// PUT body for marking read.
pub fn consumptionhorizon_body(message_id: &str, now_ms: u64) -> serde_json::Value {
    serde_json::json!({ "consumptionhorizon": consumptionhorizon_value(message_id, now_ms) })
}

/// Message id from a horizon value: text after the last ';'.
/// Empty/blank horizons yield None (caller drops the entry).
pub fn receipt_message_id(horizon: &str) -> Option<String> {
    let id = horizon.rsplit(';').next()?.trim();
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

/// User key from one consumptionhorizon entry: `mri` → `id` → `user`
/// → display name, else "". Never fails (unknown shapes stay parseable).
fn receipt_user(entry: &serde_json::Value) -> String {
    for k in ["mri", "id", "user", "imdisplayname", "displayName"] {
        if let Some(s) = entry.get(k).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Parse the GET consumptionhorizons envelope into receipts. Tolerant:
/// missing/empty lists yield vec![], entries without a parseable horizon
/// are dropped, bare-string entries use "" as the user key.
pub fn parse_consumptionhorizons(value: &serde_json::Value) -> Vec<ReadReceipt> {
    let list = value
        .get("consumptionhorizons")
        .and_then(|v| v.as_array());
    let Some(list) = list else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in list {
        if let Some(s) = e.as_str() {
            let horizon = s.trim().to_string();
            if let Some(mid) = receipt_message_id(&horizon) {
                out.push(ReadReceipt {
                    user: String::new(),
                    message_id: mid,
                    horizon,
                });
            }
            continue;
        }
        let horizon = e
            .get("consumptionhorizon")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let Some(mid) = receipt_message_id(&horizon) else {
            continue;
        };
        out.push(ReadReceipt {
            user: receipt_user(e),
            message_id: mid,
            horizon,
        });
    }
    out
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Mark one conversation read up to `message_id` via PUT. Empty ids are
/// rejected before any network.
pub async fn mark_read_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message_id: &str,
) -> Result<()> {
    if chat_id.trim().is_empty() {
        anyhow::bail!("empty chat_id");
    }
    if message_id.trim().is_empty() {
        anyhow::bail!("empty message_id");
    }
    let base = client.chat_service_url();
    let url = consumptionhorizon_url(&base, chat_id.trim());
    let body = consumptionhorizon_body(message_id.trim(), now_millis());
    tracing::debug!("Marking read at {}", url);
    client.chat_put(&url, &body).await?;
    Ok(())
}

/// Peer read positions for one thread (read-only). Unknown users/shapes
/// parse tolerantly via [`parse_consumptionhorizons`].
pub async fn read_receipts_data(
    client: &TeamsClient,
    thread_id: &str,
) -> Result<Vec<ReadReceipt>> {
    if thread_id.trim().is_empty() {
        anyhow::bail!("empty thread_id");
    }
    let base = client.chat_service_url();
    let url = consumptionhorizons_url(&base, thread_id.trim());
    let resp = client.chat_get(&url).await?;
    let value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse consumptionhorizons response")?;
    Ok(parse_consumptionhorizons(&value))
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

    #[test]
    fn receipt_endpoint_shapes() {
        let base = "https://h";
        assert_eq!(
            consumptionhorizon_url(base, "19:t@thread.v2"),
            "https://h/v1/users/ME/conversations/19:t@thread.v2/properties?name=consumptionhorizon"
        );
        assert_eq!(
            consumptionhorizons_url(base, "19:t@thread.v2"),
            "https://h/v1/threads/19:t@thread.v2/consumptionhorizons"
        );
        assert_eq!(
            consumptionhorizon_value("m42", 1700000000000),
            "1700000000000;1700000000000;m42"
        );
        assert_eq!(
            consumptionhorizon_body("m42", 7),
            serde_json::json!({ "consumptionhorizon": "7;7;m42" })
        );
    }

    #[test]
    fn receipt_message_id_splits_last_segment() {
        assert_eq!(
            receipt_message_id("1;2;m42").as_deref(),
            Some("m42")
        );
        assert_eq!(receipt_message_id("m42").as_deref(), Some("m42"));
        assert_eq!(receipt_message_id(" 1;2; m42 ").as_deref(), Some("m42"));
        assert_eq!(receipt_message_id(""), None);
        assert_eq!(receipt_message_id("   "), None);
        assert_eq!(receipt_message_id("1;2;"), None);
    }

    #[test]
    fn receipt_parse_tolerates_shapes() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"id":"19:t@thread.v2","version":"1","consumptionhorizons":[
                {"mri":"8:orgid:a","consumptionhorizon":"1;2;m1"},
                {"user":"Bo","consumptionhorizon":"3;4;m2"},
                "5;6;m3",
                {"mri":"8:orgid:bad","consumptionhorizon":""},
                {"mri":"8:orgid:nohorizon"}
            ]}"#,
        )
        .unwrap();
        let out = parse_consumptionhorizons(&v);
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[0],
            ReadReceipt {
                user: "8:orgid:a".to_string(),
                message_id: "m1".to_string(),
                horizon: "1;2;m1".to_string(),
            }
        );
        assert_eq!(out[1].user, "Bo");
        assert_eq!(out[1].message_id, "m2");
        assert_eq!(out[2].user, "");
        assert_eq!(out[2].message_id, "m3");
        // Missing/empty lists yield empty, never panic.
        let missing: serde_json::Value = serde_json::from_str(r#"{"id":"x"}"#).unwrap();
        assert!(parse_consumptionhorizons(&missing).is_empty());
        let empty: serde_json::Value =
            serde_json::from_str(r#"{"consumptionhorizons":[]}"#).unwrap();
        assert!(parse_consumptionhorizons(&empty).is_empty());
    }
}
