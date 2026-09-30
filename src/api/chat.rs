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

// ---------------------------------------------------------------------------
// Pinned messages
// ---------------------------------------------------------------------------

/// One server-side pinned chat message. `graph_pin_id` is set only for
/// Graph-sourced pins (the `pinnedChatMessageInfo` id Graph DELETE
/// takes); chat-service pins carry the message id alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PinnedRef {
    pub message_id: String,
    pub sender: Option<String>,
    pub preview: Option<String>,
    /// Message arrival time (ISO 8601).
    pub time: Option<String>,
    pub pinned_by: Option<String>,
    pub pinned_at: Option<String>,
    pub graph_pin_id: Option<String>,
}

/// Where a chat's pins came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinSource {
    ChatService,
    Graph,
}

/// Chat-service thread GET (properties carry the thread's state).
pub fn thread_pins_url(base: &str, chat_id: &str) -> String {
    format!("{}/v1/threads/{}?view=msnp24Equivalent", base, chat_id.trim())
}

/// Graph `GET /chats/{id}/pinnedMessages?$expand=message` (needs
/// Chat.Read: expect 403 on the Teams web token).
pub fn graph_pins_path(chat_id: &str) -> String {
    format!("/chats/{}/pinnedMessages?$expand=message", chat_id.trim())
}

/// Scalar JSON (string or number) as a trimmed non-empty string.
fn pin_scalar(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// First present scalar among `keys` (case-insensitive).
fn pin_field(o: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        let v = o.get(*k).or_else(|| o.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v));
        pin_scalar(v)
    })
}

/// Plain one-line preview from message HTML/text.
fn pin_preview(content: &str) -> Option<String> {
    let text = strip_html(content);
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(one).filter(|s| !s.is_empty())
}

/// A bare id from a string list: chat-service message ids are arrival
/// ms, so only all-digit tokens count (flags like "true" never pin).
fn bare_pin_id(s: &str) -> Option<String> {
    let t = s.trim().trim_matches('"').trim();
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.to_string())
}

fn collect_thread_pins(v: &serde_json::Value, depth: usize, out: &mut Vec<PinnedRef>) {
    if depth > 4 {
        return;
    }
    match v {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.starts_with('[') || t.starts_with('{') {
                if let Ok(inner) = serde_json::from_str::<serde_json::Value>(t) {
                    collect_thread_pins(&inner, depth + 1, out);
                    return;
                }
            }
            out.extend(t.split(',').filter_map(bare_pin_id).map(|message_id| PinnedRef {
                message_id,
                ..Default::default()
            }));
        }
        serde_json::Value::Number(n) => out.push(PinnedRef {
            message_id: n.to_string(),
            ..Default::default()
        }),
        serde_json::Value::Array(a) => {
            for e in a {
                collect_thread_pins(e, depth + 1, out);
            }
        }
        serde_json::Value::Object(o) => {
            let id = pin_field(o, &["messageId", "messageid", "id"])
                .filter(|s| !s.contains(['/', '?', '#', ' ']));
            match id {
                Some(message_id) => out.push(PinnedRef {
                    message_id,
                    sender: pin_field(o, &["sender", "imdisplayname", "senderDisplayName"]),
                    preview: pin_field(o, &["content", "preview", "messagePreview"])
                        .and_then(|c| pin_preview(&c)),
                    time: pin_field(o, &["originalarrivaltime", "composetime", "messageTime"]),
                    pinned_by: pin_field(o, &["pinnedBy", "pinnedby"]),
                    pinned_at: pin_field(o, &["pinnedTime", "pinnedAt", "pinnedDateTime"]),
                    graph_pin_id: None,
                }),
                None => {
                    // Id-less wrapper (e.g. `{pins:[…]}`): descend into
                    // containers only — its scalars (times, flags) are
                    // never ids.
                    for inner in o.values().filter(|v| v.is_array() || v.is_object()) {
                        collect_thread_pins(inner, depth + 1, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Pins from a chat-service thread body (`GET /v1/threads/{id}`). The
/// shape is undocumented, so this is tolerant: every `properties` key
/// whose lowercase name contains "pinned" is read, its value a JSON
/// string, an array, or comma-separated ids; object entries take
/// `messageId|id` (+ optional pinnedBy/pinnedTime/sender/content).
/// Deduplicated by message id, first wins. Pure (no network).
pub fn parse_thread_pins(value: &serde_json::Value) -> Vec<PinnedRef> {
    let props = value
        .get("properties")
        .and_then(|p| p.as_object())
        .or_else(|| value.as_object());
    let mut out = Vec::new();
    if let Some(props) = props {
        for (k, v) in props {
            if k.to_lowercase().contains("pinned") {
                collect_thread_pins(v, 0, &mut out);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.message_id.clone()));
    out
}

/// Pins from a Graph `pinnedMessages?$expand=message` body. Entries
/// without a message id are skipped. Pure (no network).
pub fn parse_graph_pins(value: &serde_json::Value) -> Result<Vec<PinnedRef>> {
    let list = value
        .get("value")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("pinned messages response has no value[]"))?;
    let mut out = Vec::new();
    for e in list {
        let msg = &e["message"];
        let Some(message_id) = pin_scalar(msg.get("id")) else { continue };
        if msg["deletedDateTime"].as_str().is_some_and(|s| !s.is_empty()) {
            continue;
        }
        let sender = pin_scalar(msg["from"]["user"].get("displayName"))
            .or_else(|| pin_scalar(msg["from"]["application"].get("displayName")));
        out.push(PinnedRef {
            message_id,
            sender,
            preview: msg["body"]["content"].as_str().and_then(pin_preview),
            time: pin_scalar(msg.get("createdDateTime")),
            pinned_by: None,
            pinned_at: None,
            graph_pin_id: pin_scalar(e.get("id")),
        });
    }
    Ok(out)
}

/// Fill a thread pin's missing sender/preview/time from its chat-service
/// message (`GET …/messages/{id}`). Present fields are kept. Pure.
pub fn fill_pin_from_message(pin: &mut PinnedRef, msg: &serde_json::Value) {
    if pin.sender.is_none() {
        pin.sender = pin_scalar(msg.get("imdisplayname"));
    }
    if pin.preview.is_none() {
        pin.preview = msg["content"].as_str().and_then(pin_preview);
    }
    if pin.time.is_none() {
        pin.time = pin_scalar(msg.get("originalarrivaltime")).or_else(|| pin_scalar(msg.get("composetime")));
    }
}

/// Source precedence: non-empty chat-service pins, else Graph pins,
/// else an empty chat-service read (authoritative "no pins"), else the
/// chat-service error (Graph's 403 is the expected, uninformative one).
/// `graph` is `None` when it was never tried. Pure.
pub fn pick_pins(
    thread: Result<Vec<PinnedRef>>,
    graph: Option<Result<Vec<PinnedRef>>>,
) -> Result<(PinSource, Vec<PinnedRef>)> {
    match (thread, graph) {
        (Ok(t), _) if !t.is_empty() => Ok((PinSource::ChatService, t)),
        (_, Some(Ok(g))) => Ok((PinSource::Graph, g)),
        (Ok(t), _) => Ok((PinSource::ChatService, t)),
        (Err(e), Some(Err(g))) => Err(e.context(format!("graph pinnedMessages fallback also failed: {:#}", g))),
        (Err(e), None) => Err(e),
    }
}

/// A chat's server-side pinned messages. Chat service thread
/// properties first (skypetoken; shape undocumented, parsed tolerantly),
/// Graph `pinnedMessages` fallback (needs Chat.Read — 403 on the Teams
/// web token). Thread pins missing a preview are filled from their
/// chat-service message (failure leaves them `None`). GETs only; the
/// consumption horizon never moves.
pub async fn chat_pinned_messages_data(client: &TeamsClient, chat_id: &str) -> Result<(PinSource, Vec<PinnedRef>)> {
    let chat_id = chat_id.trim();
    if chat_id.is_empty() || chat_id.contains(['/', '?', '#', ' ']) {
        bail!("bad chat id");
    }
    let base = client.chat_service_url();
    let thread = async {
        let v: serde_json::Value = client
            .chat_get(&thread_pins_url(&base, chat_id))
            .await?
            .json()
            .await
            .context("Failed to parse thread response")?;
        Ok::<_, anyhow::Error>(parse_thread_pins(&v))
    }
    .await;
    let need_graph = !matches!(&thread, Ok(t) if !t.is_empty());
    let graph = if need_graph {
        Some(
            async {
                let v: serde_json::Value =
                    client.graph_get(&graph_pins_path(chat_id)).await?.json().await.context("pinned messages json")?;
                parse_graph_pins(&v)
            }
            .await,
        )
    } else {
        None
    };
    let (source, mut pins) = pick_pins(thread, graph)?;
    if source == PinSource::ChatService {
        for pin in pins.iter_mut().filter(|p| p.preview.is_none() || p.sender.is_none()) {
            if pin.message_id.contains(['/', '?', '#', ' ']) {
                continue;
            }
            let url = format!("{}/v1/users/ME/conversations/{}/messages/{}", base, chat_id, pin.message_id);
            if let Ok(resp) = client.chat_get(&url).await {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    fill_pin_from_message(pin, &v);
                }
            }
        }
    }
    Ok((source, pins))
}

#[cfg(test)]
mod pinned_tests {
    use super::*;

    #[test]
    fn thread_pins_from_json_string_property() {
        let pins = serde_json::json!([
            {"messageId": "1727000000100", "pinnedBy": "8:orgid:aaa", "pinnedTime": "1727000000900"},
            {"id": 1727000000200u64, "content": "<p>Ship <b>Friday</b></p>", "imdisplayname": "Ava Hart"},
            {"messageId": "1727000000100"}
        ])
        .to_string();
        let thread = serde_json::json!({
            "id": "19:a@thread.v2", "type": "Thread",
            "properties": {"topic": "Launch", "pinnedMessages": pins, "ispinned": "true"}
        });
        let got = parse_thread_pins(&thread);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].message_id, "1727000000100");
        assert_eq!(got[0].pinned_by.as_deref(), Some("8:orgid:aaa"));
        assert_eq!(got[0].pinned_at.as_deref(), Some("1727000000900"));
        assert!(got[0].preview.is_none() && got[0].graph_pin_id.is_none());
        assert_eq!(got[1].message_id, "1727000000200");
        assert_eq!(got[1].preview.as_deref(), Some("Ship Friday"));
        assert_eq!(got[1].sender.as_deref(), Some("Ava Hart"));
    }

    #[test]
    fn thread_pins_from_array_and_csv_and_absent() {
        let arr = serde_json::json!({"properties": {"PinnedMessages": [
            {"messageid": "1727000000300", "sender": "Jamie Brooks"}, "1727000000400"]}});
        let ids: Vec<_> = parse_thread_pins(&arr).into_iter().map(|p| p.message_id).collect();
        assert_eq!(ids, ["1727000000300", "1727000000400"]);
        let csv = serde_json::json!({"properties": {"pinnedmessages": "1727000000500, 1727000000600,"}});
        assert_eq!(parse_thread_pins(&csv).len(), 2);
        let none = serde_json::json!({"properties": {"topic": "x", "alerts": "true"}});
        assert!(parse_thread_pins(&none).is_empty());
        assert!(parse_thread_pins(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn graph_pins_parse_and_fill_from_message() {
        let v = serde_json::json!({"value": [
            {"id": "pin-1", "message": {"id": "1727000000700", "createdDateTime": "2026-09-28T10:00:00Z",
             "from": {"user": {"displayName": "Alex Carter"}},
             "body": {"contentType": "html", "content": "<p>Budget  due</p>"}}},
            {"id": "pin-2", "message": {"id": "1727000000800", "deletedDateTime": "2026-09-28T11:00:00Z"}},
            {"id": "pin-3"}
        ]});
        let got = parse_graph_pins(&v).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].graph_pin_id.as_deref(), Some("pin-1"));
        assert_eq!(got[0].sender.as_deref(), Some("Alex Carter"));
        assert_eq!(got[0].preview.as_deref(), Some("Budget due"));
        assert!(parse_graph_pins(&serde_json::json!({"error": {"code": "Forbidden"}})).is_err());

        let mut p = PinnedRef { message_id: "1".into(), sender: Some("Kept".into()), ..Default::default() };
        fill_pin_from_message(&mut p, &serde_json::json!({"imdisplayname": "Other",
            "content": "<div>hi there</div>", "originalarrivaltime": "2026-09-28T09:00:00Z"}));
        assert_eq!(p.sender.as_deref(), Some("Kept"));
        assert_eq!(p.preview.as_deref(), Some("hi there"));
        assert_eq!(p.time.as_deref(), Some("2026-09-28T09:00:00Z"));
    }

    #[test]
    fn source_precedence_and_graph_403() {
        let pin = |id: &str| PinnedRef { message_id: id.into(), ..Default::default() };
        let forbidden = || Err(anyhow::anyhow!("HTTP 403 Forbidden: Missing scope permissions"));
        // Thread pins win; Graph untried.
        let (s, p) = pick_pins(Ok(vec![pin("1")]), None).unwrap();
        assert_eq!((s, p.len()), (PinSource::ChatService, 1));
        // Empty thread + Graph 403 = no pins, not an error.
        let (s, p) = pick_pins(Ok(vec![]), Some(forbidden())).unwrap();
        assert_eq!((s, p.len()), (PinSource::ChatService, 0));
        // Thread failure falls back to Graph.
        let (s, p) = pick_pins(Err(anyhow::anyhow!("chatsvc 500")), Some(Ok(vec![pin("2")]))).unwrap();
        assert_eq!((s, p[0].message_id.as_str()), (PinSource::Graph, "2"));
        // Both fail: the chat-service error leads.
        let e = pick_pins(Err(anyhow::anyhow!("chatsvc 500")), Some(forbidden())).unwrap_err();
        let msg = format!("{:#}", e);
        assert!(msg.contains("chatsvc 500") && msg.contains("403"), "{}", msg);
        assert_eq!(e.root_cause().to_string(), "chatsvc 500");
    }

    #[test]
    fn pin_paths() {
        assert_eq!(thread_pins_url("https://h", " 19:a@thread.v2 "), "https://h/v1/threads/19:a@thread.v2?view=msnp24Equivalent");
        assert_eq!(graph_pins_path("19:a@thread.v2"), "/chats/19:a@thread.v2/pinnedMessages?$expand=message");
    }
}
