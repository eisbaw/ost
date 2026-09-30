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
    #[serde(default, rename = "clientmessageid", alias = "ClientMessageId")]
    client_message_id: Option<serde_json::Value>,
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

// Idempotent sends. Every post carries a `clientmessageid`; the server
// receipt (Location / OriginalArrivalTime) names the posted copy, and
// `find_message_by_client_id` lets a caller whose POST timed out check
// whether it landed before re-posting with the SAME id, so one logical
// send never becomes two server messages.

/// New client message id: a 19-digit decimal string (the shape Teams
/// clients use). Idempotency key for one logical send.
pub fn new_client_message_id() -> String {
    let v = u128::from_be_bytes(*uuid::Uuid::new_v4().as_bytes());
    let n = 1_000_000_000_000_000_000u128 + v % 9_000_000_000_000_000_000u128;
    n.to_string()
}

/// Server receipt for one chat-service POST.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentMessage {
    /// Posted message id, when the answer named it.
    pub id: Option<String>,
    /// The `clientmessageid` the post carried.
    pub client_message_id: String,
}

/// Server message id from a POST answer (pure): the `Location` header's
/// numeric last path segment wins, else `OriginalArrivalTime` from the
/// JSON body (chat-service ids are the arrival epoch ms). None when the
/// answer names neither.
pub fn sent_id_from_response(location: Option<&str>, body: &str) -> Option<String> {
    if let Some(loc) = location {
        let path = loc.split('?').next().unwrap_or(loc);
        let last = path.rsplit('/').next().unwrap_or("").trim();
        if !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()) {
            return Some(last.to_string());
        }
    }
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = v.as_object()?;
    let t = obj
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("OriginalArrivalTime"))
        .map(|(_, v)| v)?;
    match t {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s)
            if !s.trim().is_empty() && s.chars().all(|c| c.is_ascii_digit()) =>
        {
            Some(s.clone())
        }
        _ => None,
    }
}

/// POST one body carrying `clientmessageid` and read the receipt.
async fn post_with_receipt(
    client: &TeamsClient,
    url: &str,
    mut body: serde_json::Value,
    client_message_id: &str,
) -> Result<SentMessage> {
    body["clientmessageid"] = serde_json::json!(client_message_id);
    let resp = client.chat_post(url, &body).await?;
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // A 2xx already means posted; an unreadable body only loses the id.
    let text = resp.text().await.unwrap_or_default();
    Ok(SentMessage {
        id: sent_id_from_response(location.as_deref(), &text),
        client_message_id: client_message_id.to_string(),
    })
}

/// Send with a caller-owned `clientmessageid` (retries reuse it).
pub async fn send_message_with_client_id(
    client: &TeamsClient,
    chat_id: &str,
    message: &str,
    client_message_id: &str,
) -> Result<SentMessage> {
    let url = format!(
        "{}/v1/users/ME/conversations/{}/messages",
        client.chat_service_url(),
        chat_id
    );
    let body = serde_json::json!({
        "content": format!("<p>{}</p>", html_escape(message)),
        "messagetype": "RichText/Html",
        "contenttype": "text"
    });
    post_with_receipt(client, &url, body, client_message_id).await
}

/// The message carrying `client_message_id` in one page, if any (pure).
pub fn find_by_client_id<'a>(
    messages: &'a [MessageInfo],
    client_message_id: &str,
) -> Option<&'a MessageInfo> {
    let want = client_message_id.trim();
    if want.is_empty() {
        return None;
    }
    messages
        .iter()
        .find(|m| m.client_message_id.as_deref() == Some(want))
}

/// Verify one send: read the newest page of `chat_id` and return the
/// message carrying `client_message_id` (None = not posted, as far as
/// the newest page shows).
pub async fn find_message_by_client_id(
    client: &TeamsClient,
    chat_id: &str,
    client_message_id: &str,
) -> Result<Option<MessageInfo>> {
    let want = client_message_id.trim();
    if want.is_empty() {
        return Ok(None);
    }
    let page = read_messages_page(client, chat_id, 50, None).await?;
    Ok(page
        .messages
        .into_iter()
        .find(|m| m.client_message_id.as_deref() == Some(want)))
}

/// Wire `clientmessageid` (string or number), trimmed; None when
/// absent or blank.
fn client_message_id_of(v: Option<&serde_json::Value>) -> Option<String> {
    let s = match v? {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
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
    /// The `clientmessageid` the sender posted with (the idempotency key
    /// a pending local send reconciles by).
    pub client_message_id: Option<String>,
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
            client_message_id: client_message_id_of(msg.client_message_id.as_ref()),
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

#[cfg(test)]
mod idempotent_send_tests {
    use super::*;

    #[test]
    fn client_message_id_is_19_digits_and_fresh() {
        let a = new_client_message_id();
        let b = new_client_message_id();
        assert_eq!(a.len(), 19);
        assert!(a.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(a, b);
    }

    #[test]
    fn sent_id_prefers_location_then_arrival_time() {
        let loc = "https://h/v1/users/ME/conversations/19:a@thread.v2/messages/1727540406676";
        assert_eq!(sent_id_from_response(Some(loc), "").as_deref(), Some("1727540406676"));
        assert_eq!(
            sent_id_from_response(None, r#"{"OriginalArrivalTime":1727540406677}"#).as_deref(),
            Some("1727540406677")
        );
        assert_eq!(
            sent_id_from_response(Some("https://h/x/messages"), r#"{"originalarrivaltime":"42"}"#)
                .as_deref(),
            Some("42")
        );
        assert_eq!(sent_id_from_response(None, "{}"), None);
        assert_eq!(sent_id_from_response(None, "not json"), None);
    }

    #[test]
    fn history_rows_carry_client_message_id() {
        let with: NativeMessage =
            serde_json::from_str(r#"{"id":"2","clientmessageid":"555","content":"<p>b</p>"}"#)
                .unwrap();
        let num: NativeMessage =
            serde_json::from_str(r#"{"id":"3","ClientMessageId":556}"#).unwrap();
        let without: NativeMessage = serde_json::from_str(r#"{"id":"1"}"#).unwrap();
        assert_eq!(client_message_id_of(with.client_message_id.as_ref()).as_deref(), Some("555"));
        assert_eq!(client_message_id_of(num.client_message_id.as_ref()).as_deref(), Some("556"));
        assert_eq!(client_message_id_of(without.client_message_id.as_ref()), None);
        let row = |id: &str, c: Option<&str>| MessageInfo {
            id: id.into(),
            sender: "A".into(),
            timestamp: String::new(),
            content: "x".into(),
            raw: String::new(),
            client_message_id: c.map(String::from),
        };
        let rows = vec![row("1", None), row("2", Some("555"))];
        assert_eq!(find_by_client_id(&rows, "555").map(|m| m.id.as_str()), Some("2"));
        assert!(find_by_client_id(&rows, "556").is_none());
        assert!(find_by_client_id(&rows, " ").is_none());
    }
}
