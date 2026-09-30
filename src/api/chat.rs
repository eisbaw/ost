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
    pub sender: String,
    pub timestamp: String,
    pub content: String,
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
pub async fn read_messages_data(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<MessageInfo>> {
    let base = client.chat_service_url();
    let url = format!(
        "{}/v1/users/ME/conversations/{}/messages?pageSize={}",
        base, chat_id, limit
    );

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

        let sender = msg.im_display_name.as_deref().unwrap_or("?").to_string();
        let time = msg
            .original_arrival_time
            .as_deref()
            .or(msg.compose_time.as_deref())
            .unwrap_or("")
            .to_string();
        let content = msg.content.as_deref().unwrap_or("");
        let text = strip_html(content);

        if text.trim().is_empty() {
            continue;
        }

        result.push(MessageInfo {
            sender,
            timestamp: time,
            content: text.trim().to_string(),
        });
    }

    Ok(result)
}

// -- Files shared in a chat (chat Shared tab) --

/// One file shared in a chat, from a chat-service message's
/// `properties.files` (the page the timeline reads). The Graph message
/// endpoints need `Chat.Read`, which the Teams web token lacks (403),
/// so this is the chat Shared tab's source. GET only: the consumption
/// horizon (read state) is a separate PUT and never moves here.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatFileRef {
    /// File attachment GUID (the body's `<attachment id>`).
    pub attachment_id: Option<String>,
    pub name: String,
    pub file_type: Option<String>,
    /// SharePoint/OneDrive URL of the file itself.
    pub object_url: String,
    /// Sharing link, when the sender's client made one.
    pub share_url: Option<String>,
    pub sender: Option<String>,
    /// Message arrival time (ISO 8601).
    pub time: Option<String>,
}

/// Chat-service page size for the Shared tab walk (server maximum).
pub const CHAT_FILES_PAGE_SIZE: usize = 200;

/// Files shared on one chat-service messages page, newest first, and
/// the page's `backwardLink` (older history). Deleted messages and
/// files marked deleted are skipped; `properties.files` may arrive as a
/// JSON string or an array. Pure (no network).
pub fn parse_chat_file_refs(page: &serde_json::Value) -> (Vec<ChatFileRef>, Option<String>) {
    let str_of = |v: &serde_json::Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let mut out = Vec::new();
    for msg in page["messages"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let props = &msg["properties"];
        if str_of(&props["deletetime"]).is_some() {
            continue;
        }
        let files: Vec<serde_json::Value> = match &props["files"] {
            serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or_default(),
            serde_json::Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        for f in &files {
            if f["state"].as_str() == Some("deleted") {
                continue;
            }
            let Some(object_url) = str_of(&f["objectUrl"]).or_else(|| str_of(&f["fileInfo"]["fileUrl"])) else {
                continue;
            };
            let name = str_of(&f["fileName"])
                .or_else(|| str_of(&f["title"]))
                .or_else(|| object_url.rsplit('/').next().map(String::from))
                .unwrap_or_else(|| "[unnamed]".to_string());
            out.push(ChatFileRef {
                attachment_id: str_of(&f["id"]),
                name,
                file_type: str_of(&f["fileType"]).or_else(|| str_of(&f["type"])),
                object_url,
                share_url: str_of(&f["fileInfo"]["shareUrl"]),
                sender: str_of(&msg["imdisplayname"]),
                time: str_of(&msg["originalarrivaltime"]).or_else(|| str_of(&msg["composetime"])),
            });
        }
    }
    let back = str_of(&page["_metadata"]["backwardLink"]);
    (out, back)
}

/// The files one chat-service message shares, from a single
/// message object (`GET …/messages/{id}`) or a page holding it. Pure.
pub fn message_file_refs(value: &serde_json::Value, message_id: &str) -> Option<Vec<ChatFileRef>> {
    let want = message_id.trim();
    let one = |m: &serde_json::Value| {
        let id = m["id"].as_str().or_else(|| m["clientmessageid"].as_str()).unwrap_or("");
        id == want
    };
    let msg = if let Some(list) = value["messages"].as_array() {
        list.iter().find(|m| one(m))?.clone()
    } else if one(value) {
        value.clone()
    } else {
        return None;
    };
    Some(parse_chat_file_refs(&serde_json::json!({ "messages": [msg] })).0)
}

/// Chat-service message ids are the arrival time in ms: a history page
/// whose oldest message is newer than `message_id` must go further
/// back. Pure.
pub fn page_reaches(page: &serde_json::Value, message_id: &str) -> bool {
    let Ok(want) = message_id.trim().parse::<i64>() else { return true };
    page["messages"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str()?.parse::<i64>().ok()).any(|id| id <= want))
        .unwrap_or(true)
}

/// The files one chat message shares, read from the chat
/// service (the Graph message route needs Chat.Read: 403 on the Teams
/// web token). Single-message GET first, then a history walk of at most
/// `max_pages` pages until the page reaching the message. GET only.
pub async fn chat_message_file_refs_data(
    client: &TeamsClient,
    chat_id: &str,
    message_id: &str,
    max_pages: usize,
) -> Result<Vec<ChatFileRef>> {
    let (chat_id, message_id) = (chat_id.trim(), message_id.trim());
    let bad = |s: &str| s.is_empty() || s.contains(['/', '?', '#', ' ']);
    if bad(chat_id) || bad(message_id) {
        bail!("bad chat or message id");
    }
    let one = format!(
        "{}/v1/users/ME/conversations/{}/messages/{}",
        client.chat_service_url(),
        chat_id,
        message_id
    );
    if let Ok(resp) = client.chat_get(&one).await {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(refs) = message_file_refs(&v, message_id) {
                return Ok(refs);
            }
        }
    }
    let mut url = format!(
        "{}/v1/users/ME/conversations/{}/messages?pageSize={}",
        client.chat_service_url(),
        chat_id,
        CHAT_FILES_PAGE_SIZE
    );
    for _ in 0..max_pages.max(1) {
        let page: serde_json::Value = client
            .chat_get(&url)
            .await?
            .json()
            .await
            .context("Failed to parse messages response")?;
        if let Some(refs) = message_file_refs(&page, message_id) {
            return Ok(refs);
        }
        let back = page["_metadata"]["backwardLink"].as_str().map(str::to_string);
        let empty = page["messages"].as_array().map_or(true, |a| a.is_empty());
        match back {
            Some(b) if !empty && !page_reaches(&page, message_id) => url = b,
            _ => break,
        }
    }
    bail!("message not found in chat history")
}

/// Files shared in a chat, newest first, deduplicated by file URL: walks
/// chat-service history pages (newest first) until `limit` files or
/// `max_pages` pages. Read-only GETs.
pub async fn chat_file_refs_data(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
    max_pages: usize,
) -> Result<Vec<ChatFileRef>> {
    let chat_id = chat_id.trim();
    if chat_id.is_empty() || chat_id.contains(['/', '?', '#', ' ']) {
        bail!("bad chat id");
    }
    let mut url = format!(
        "{}/v1/users/ME/conversations/{}/messages?pageSize={}",
        client.chat_service_url(),
        chat_id,
        CHAT_FILES_PAGE_SIZE
    );
    let mut out: Vec<ChatFileRef> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..max_pages.max(1) {
        let page: serde_json::Value = client
            .chat_get(&url)
            .await?
            .json()
            .await
            .context("Failed to parse messages response")?;
        let (refs, back) = parse_chat_file_refs(&page);
        let empty_page = page["messages"].as_array().map_or(true, |a| a.is_empty());
        for r in refs {
            if seen.insert(r.object_url.to_lowercase()) {
                out.push(r);
            }
        }
        // The backwardLink carries the first request's pageSize.
        match back {
            Some(b) if out.len() < limit && !empty_page => url = b,
            _ => break,
        }
    }
    out.truncate(limit.max(1));
    Ok(out)
}

#[cfg(test)]
mod chat_files_tests {
    use super::*;

    #[test]
    fn message_file_refs_single_and_page() {
        let files = r#"[{"id":"att-1","fileName":"Plan.docx","objectUrl":"https://c.sharepoint.com/Plan.docx","fileInfo":{"shareUrl":"https://c.sharepoint.com/:w:/s/x"}}]"#;
        let msg = serde_json::json!({"id":"1700000000500","imdisplayname":"Ava Hart",
            "originalarrivaltime":"2026-09-28T10:00:00Z","properties":{"files":files}});
        let refs = message_file_refs(&msg, "1700000000500").unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].attachment_id.as_deref(), Some("att-1"));
        assert_eq!(refs[0].share_url.as_deref(), Some("https://c.sharepoint.com/:w:/s/x"));
        assert!(message_file_refs(&msg, "999").is_none());
        let page = serde_json::json!({"messages":[
            {"id":"1700000000900","properties":{}}, msg.clone(), {"id":"1700000000100"}]});
        assert_eq!(message_file_refs(&page, "1700000000500").unwrap().len(), 1);
        assert!(message_file_refs(&page, "1700000000400").is_none());
        // A message without files yields an empty list, not a miss.
        assert_eq!(message_file_refs(&page, "1700000000900").unwrap().len(), 0);
        // History walk stops once a page reaches the message's time.
        assert!(page_reaches(&page, "1700000000400"));
        let newer = serde_json::json!({"messages":[{"id":"1700000000900"}]});
        assert!(!page_reaches(&newer, "1700000000400"));
    }

    #[test]
    fn file_refs_from_chat_service_page() {
        let files = serde_json::json!([
            {"id": "att-1", "fileName": "Plan.docx", "fileType": "docx",
             "objectUrl": "https://contoso-my.sharepoint.com/personal/a/Documents/Microsoft Teams Chat Files/Plan.docx",
             "fileInfo": {"shareUrl": "https://contoso-my.sharepoint.com/:w:/g/personal/a/xyz"}},
            {"id": "att-2", "title": "Old.xlsx", "state": "deleted", "objectUrl": "https://x/Old.xlsx"}
        ])
        .to_string();
        let page = serde_json::json!({
            "messages": [
                {"imdisplayname": "Alex Carter", "originalarrivaltime": "2026-09-28T09:00:00Z",
                 "properties": {"files": files}},
                {"imdisplayname": "Jamie Brooks", "properties": {"deletetime": "1700000000000",
                 "files": [{"id": "gone", "objectUrl": "https://x/Gone.pdf"}]}},
                {"imdisplayname": "Jamie Brooks", "composetime": "2026-09-27T08:00:00Z",
                 "properties": {"files": [{"id": "att-3", "fileInfo": {"fileUrl": "https://x/Budget.pptx"}}]}},
                {"content": "no files here"}
            ],
            "_metadata": {"backwardLink": "https://svc/v1/users/ME/conversations/c/messages?startTime=1&pageSize=200"}
        });
        let (refs, back) = parse_chat_file_refs(&page);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "Plan.docx");
        assert_eq!(refs[0].attachment_id.as_deref(), Some("att-1"));
        assert_eq!(refs[0].share_url.as_deref(), Some("https://contoso-my.sharepoint.com/:w:/g/personal/a/xyz"));
        assert_eq!(refs[0].sender.as_deref(), Some("Alex Carter"));
        assert_eq!(refs[0].time.as_deref(), Some("2026-09-28T09:00:00Z"));
        assert_eq!(refs[1].name, "Budget.pptx");
        assert_eq!(refs[1].object_url, "https://x/Budget.pptx");
        assert_eq!(refs[1].time.as_deref(), Some("2026-09-27T08:00:00Z"));
        assert!(back.unwrap().contains("startTime=1"));
        let (none, no_back) = parse_chat_file_refs(&serde_json::json!({}));
        assert!(none.is_empty() && no_back.is_none());
    }
}
