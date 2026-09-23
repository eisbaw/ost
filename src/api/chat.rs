//! Native Teams chat API (chatsvcagg / chat service)
//!
//! Uses the Skype token with `Authentication: skypetoken={token}` header,
//! bypassing Graph API which requires tenant admin consent for Chat.Read.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;
use super::me::whoami_data;

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

/// Block-level tags: a boundary here separates words, so it yields one
/// space (pending, only between two non-space chars). Inline tags
/// (`b`, `i`, `at`, `code`, …) vanish silently so `a<b>x</b>b` stays glued.
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "br", "section", "article", "header", "footer", "h1", "h2", "h3", "h4",
    "h5", "h6", "ul", "ol", "li", "dl", "dt", "dd", "table", "tr", "td", "th",
    "blockquote", "pre", "hr",
];

/// Tag name of a raw `<…>` body: attributes and the `/` of closing
/// or self-closed tags stripped, case preserved (caller matches
/// case-insensitively). `<>` yields "".
fn tag_name(body: &str) -> &str {
    let b = body.strip_prefix('/').unwrap_or(body);
    let end = b
        .find(|c: char| c.is_whitespace() || c == '/')
        .unwrap_or(b.len());
    &b[..end]
}

/// Strip HTML tags from content for CLI display.
///
/// Spacing-aware (om-chatnames): block-level tag boundaries become a
/// single space so `</p><p>` never glues words ("tag-boundary glue").
/// No leading/trailing space is added (`<p>hi</p>` → `hi`).
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    let mut pending_space = false;
    for ch in html.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                if BLOCK_TAGS.contains(&tag_name(&tag).to_lowercase().as_str()) {
                    pending_space = true;
                }
                tag.clear();
            } else {
                tag.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
        } else {
            if pending_space {
                pending_space = false;
                if !result.is_empty()
                    && !result.ends_with(char::is_whitespace)
                    && !ch.is_whitespace()
                {
                    result.push(' ');
                }
            }
            result.push(ch);
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

/// Human label for a chat with no topic, mate, or sender — never the
/// raw thread id (om-chatnames). `48:xxx` system chats humanize their
/// suffix (`48:notifications` → `Notifications`); anything else gets a
/// shape-based label (`[Direct message]`, `[Group chat]`, …).
fn system_label_for(chat_id: &str) -> String {
    if let Some(rest) = chat_id.strip_prefix("48:") {
        let mut chars = rest.chars();
        match chars.next() {
            None => return "[System chat]".to_string(),
            Some(first) => {
                return format!(
                    "{}{}",
                    first.to_uppercase().collect::<String>(),
                    chars.as_str()
                );
            }
        }
    }
    if chat_id.contains("meeting") {
        "[Meeting chat]"
    } else if chat_id.contains("@thread") {
        "[Group chat]"
    } else if chat_id.starts_with("19:") {
        "[Direct message]"
    } else {
        "[Chat]"
    }
    .to_string()
}

/// Display name for a conversation: topic → resolved 1:1 mate name →
/// last-message sender → system label. Never the raw thread id.
fn conversation_name(conv: &Conversation, mate: Option<&str>) -> String {
    if let Some(ref props) = conv.thread_properties {
        if let Some(ref topic) = props.topic {
            if !topic.trim().is_empty() {
                return topic.clone();
            }
        }
    }
    if let Some(m) = mate {
        if !m.trim().is_empty() {
            return m.to_string();
        }
    }
    if let Some(ref msg) = conv.last_message {
        if let Some(ref name) = msg.im_display_name {
            if !name.trim().is_empty() {
                return name.clone();
            }
        }
    }
    system_label_for(conv.id.as_deref().unwrap_or(""))
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
    /// Server message id; embedders match realtime edits by this.
    /// OstMac: synthetic `timestamp@sender` fallback when the server omits it.
    pub id: String,
    /// Sender MRI parsed from the `from` user link
    /// (`…/v1/users/ME/contacts/8:orgid:<guid>` → `8:orgid:<guid>`);
    /// "" when the server omits `from`. Om-chatnames: 1:1 mate
    /// attribution matches this, never display-name spelling.
    pub sender_mri: String,
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

/// Roster entry from `GET /v1/threads/{id}/members`: the member MRI
/// (`8:orgid:<guid>`) is `id`. No display names on this endpoint —
/// names come from message attribution (see [`resolve_mate_name`]).
#[derive(Debug, Deserialize)]
struct ThreadMember {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadMembersResponse {
    members: Option<Vec<ThreadMember>>,
}

/// Member MRIs for one thread (read-only). Errors (404 on system
/// threads like `48:notes`, network) propagate to the caller, which
/// falls back to sender/label naming — never fatal to the list.
async fn thread_member_mris(client: &TeamsClient, thread_id: &str) -> Result<Vec<String>> {
    let base = client.chat_service_url();
    let url = format!("{}/v1/threads/{}/members", base, thread_id);
    let resp = client.chat_get(&url).await?;
    let body: ThreadMembersResponse = resp
        .json()
        .await
        .context("Failed to parse thread members response")?;
    Ok(body
        .members
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| m.id)
        .filter(|id| !id.trim().is_empty())
        .collect())
}

/// Decode `%XX` runs; malformed runs pass through untouched.
fn percent_decode(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|b| hex(*b));
            let lo = bytes.get(i + 2).and_then(|b| hex(*b));
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sender MRI from a message `from` user link: the last path segment,
/// percent-decoded (`…/ME/contacts/8:orgid:<guid>` → `8:orgid:<guid>`).
/// Missing/empty `from` → "".
fn mri_from_user_link(from: Option<&str>) -> String {
    let seg = from
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
    if seg.is_empty() {
        return String::new();
    }
    percent_decode(seg)
}

/// True when `mri` is the signed-in user: exact match or the MRI ends
/// with the owner OID (`8:orgid:<oid>`). Empty OID never matches.
fn mri_is_self(mri: &str, self_oid: &str) -> bool {
    if self_oid.trim().is_empty() || mri.trim().is_empty() {
        return false;
    }
    let m = mri.to_lowercase();
    let o = self_oid.to_lowercase();
    m == o || m.ends_with(&o)
}

/// 1:1-shaped thread ids (`19:…@unq.…`): group (`@thread.v2`) and
/// meeting ids are excluded, `48:…` system ids never qualify.
fn is_onetoone_id(chat_id: &str) -> bool {
    chat_id.starts_with("19:") && !chat_id.contains("@thread") && !chat_id.contains("meeting")
}

/// Mate display name for a 1:1 chat, resolved via MRI (om-chatnames):
/// roster MRIs minus self leaves the mate; the mate's name is the
/// newest message attributed to that MRI. `None` unless the roster
/// holds exactly one non-self MRI with at least one message —
/// anything else keeps the sender/label fallback.
async fn resolve_mate_name(
    client: &TeamsClient,
    chat_id: &str,
    self_oid: &str,
) -> Option<String> {
    let members = thread_member_mris(client, chat_id).await.ok()?;
    let mates: Vec<&str> = members
        .iter()
        .map(String::as_str)
        .filter(|m| !mri_is_self(m, self_oid))
        .collect();
    if mates.len() != 1 {
        return None;
    }
    let mate = mates[0].to_lowercase();
    let page = read_messages_page(client, chat_id, 25, None).await.ok()?;
    page.messages
        .iter()
        .rev()
        .filter(|m| m.sender_mri.to_lowercase() == mate)
        .map(|m| m.sender.clone())
        .filter(|s| !s.trim().is_empty() && s != "?")
        .next()
}

/// List recent chats and return structured data.
///
/// 1:1 chats without a topic are named after the mate (roster MRI
/// minus self, attributed through message history); every other
/// fallback is topic → last sender → system label, never a raw id.
/// Mate resolution is best-effort: roster/history/whoami failures
/// keep the sender/label fallback, never fail the list.
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
    // (chat index, conversation index) for 1:1 chats without a topic:
    // the mate name resolves after the first pass (needs whoami OID).
    let mut needs_mate: Vec<(usize, usize)> = Vec::new();
    for (ci, conv) in conversations.iter().enumerate() {
        let id = conv.id.as_deref().unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }

        let name = conversation_name(conv, None);
        let topic_missing = conv
            .thread_properties
            .as_ref()
            .and_then(|p| p.topic.as_deref())
            .map(|t| t.trim().is_empty())
            .unwrap_or(true);
        if topic_missing && is_onetoone_id(&id) {
            needs_mate.push((chats.len(), ci));
        }
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

    // Second pass: 1:1 mate names via MRI resolve. One whoami for the
    // owner OID, then per-chat roster + history attribution. Any
    // failure keeps the first-pass name — the list never fails here.
    if !needs_mate.is_empty() {
        if let Ok(me) = whoami_data(client).await {
            for (chat_idx, conv_idx) in needs_mate {
                let chat_id = chats[chat_idx].id.clone();
                if let Some(mate) = resolve_mate_name(client, &chat_id, &me.id).await {
                    let conv = &conversations[conv_idx];
                    chats[chat_idx].name = conversation_name(conv, Some(&mate));
                } else {
                    tracing::debug!("mate resolve failed for {}", chat_id);
                }
            }
        }
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
        let sender_mri = mri_from_user_link(msg.from.as_deref());
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
            sender_mri,
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
    fn strip_html_block_boundaries_space_words() {
        // Tag-boundary glue: block boundaries separate words…
        assert_eq!(strip_html("<p>Hello</p><p>World</p>"), "Hello World");
        assert_eq!(strip_html("<div>a</div><div>b</div>"), "a b");
        assert_eq!(strip_html("a<br>b"), "a b");
        assert_eq!(strip_html("a<br/>b"), "a b");
        assert_eq!(strip_html("<ul><li>a</li><li>b</li></ul>"), "a b");
        // …but add no leading/trailing space…
        assert_eq!(strip_html("<p>hi</p>"), "hi");
        assert_eq!(strip_html(""), "");
        // …never double existing whitespace…
        assert_eq!(strip_html("<p>a</p> <p>b</p>"), "a b");
        assert_eq!(strip_html("a  <p>b"), "a  b");
        // …and leave inline tags glued.
        assert_eq!(strip_html("a<b>x</b>b"), "axb");
        assert_eq!(strip_html("<p>Hi <at id=\"8:x\">Bo</at>!</p>"), "Hi Bo!");
        // Attributes, case, and entities still handled.
        assert_eq!(strip_html("<P CLASS=\"x\">a</P><p>b</p>"), "a b");
        assert_eq!(strip_html("<p>a &amp; b</p>"), "a & b");
    }

    fn conv(json: &str) -> Conversation {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn conversation_name_never_raw_id() {
        // Topic wins over everything.
        let c = conv(
            r#"{"id":"19:t@thread.v2","threadProperties":{"topic":"Ship it"},
                "lastMessage":{"imdisplayname":"A"}}"#,
        );
        assert_eq!(conversation_name(&c, Some("Mate")), "Ship it");
        // Resolved mate beats last sender (1:1 named after self otherwise).
        let c = conv(
            r#"{"id":"19:a@unq.gbl.spaces","lastMessage":{"imdisplayname":"Self, Pat"}}"#,
        );
        assert_eq!(conversation_name(&c, Some("Mate, Sam")), "Mate, Sam");
        assert_eq!(conversation_name(&c, None), "Self, Pat");
        // Blank topic/mate/sender fall through to the system label.
        let c = conv(
            r#"{"id":"19:a@unq.gbl.spaces","threadProperties":{"topic":"  "},
                "lastMessage":{"imdisplayname":""}}"#,
        );
        assert_eq!(conversation_name(&c, Some(" ")), "[Direct message]");
        // Shape-based labels, never the id.
        for (id, want) in [
            ("19:a@unq.gbl.spaces", "[Direct message]"),
            ("19:t@thread.v2", "[Group chat]"),
            ("19:meeting_x@thread.v2", "[Meeting chat]"),
            ("48:notifications", "Notifications"),
            ("48:mentions", "Mentions"),
            ("48:notes", "Notes"),
            ("weird", "[Chat]"),
        ] {
            let c = conv(&format!(r#"{{"id":"{}"}}"#, id));
            let name = conversation_name(&c, None);
            assert_eq!(name, want);
            assert!(!name.contains(id), "raw id leaks: {}", name);
        }
        // Missing id entirely still labels.
        let c: Conversation = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(conversation_name(&c, None), "[Chat]");
    }

    #[test]
    fn mri_helpers_parse_and_match() {
        // Sender MRI = last `from` segment, percent-decoded.
        assert_eq!(
            mri_from_user_link(Some(
                "https://h/v1/users/ME/contacts/8:orgid:abc-123"
            )),
            "8:orgid:abc-123"
        );
        assert_eq!(
            mri_from_user_link(Some("https://h/v1/users/8%3Aorgid%3Aabc")),
            "8:orgid:abc"
        );
        assert_eq!(mri_from_user_link(None), "");
        assert_eq!(mri_from_user_link(Some("")), "");
        // Malformed % runs pass through.
        assert_eq!(percent_decode("a%2Fb%zzc%"), "a/b%zzc%");
        // Self match: exact or OID suffix, case-insensitive, never empty.
        assert!(mri_is_self("8:orgid:ABC-123", "abc-123"));
        assert!(mri_is_self("abc-123", "ABC-123"));
        assert!(!mri_is_self("8:orgid:abc-123", "other-oid"));
        assert!(!mri_is_self("8:orgid:abc-123", ""));
        assert!(!mri_is_self("", "abc-123"));
        // 1:1 id shapes.
        assert!(is_onetoone_id("19:a_b@unq.gbl.spaces"));
        assert!(!is_onetoone_id("19:t@thread.v2"));
        assert!(!is_onetoone_id("19:meeting_x@thread.v2"));
        assert!(!is_onetoone_id("48:notes"));
        // Roster payload parses (member MRI = `id`).
        let roster: ThreadMembersResponse = serde_json::from_str(
            r#"{"totalMemberCount":2,"members":[{"id":"8:orgid:self"},{"id":"8:orgid:mate"}],"isDeleted":false}"#,
        )
        .unwrap();
        let ids: Vec<_> = roster
            .members
            .unwrap()
            .into_iter()
            .filter_map(|m| m.id)
            .collect();
        assert_eq!(ids, vec!["8:orgid:self", "8:orgid:mate"]);
    }
}
