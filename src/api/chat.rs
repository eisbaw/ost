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

/// Praise app id real clients stamp on the card attachment.
pub const PRAISE_APP_ID: &str = "d832a33f-28c2-4969-8ad0-4fee681dc5b4";
/// Attachment content type for the Praise Adaptive Card.
pub const ADAPTIVE_CARD_CONTENT_TYPE: &str = "application/vnd.microsoft.card.adaptive";

/// Adaptive Card object for one Praise post (pure): the badge title,
/// the "<sender> sent praise to" line, the recipient, and the note.
/// No remote image: badge art renders natively in-app, so the card
/// never depends on a CDN URL whose per-badge hash we do not know.
pub fn praise_card_content(badge: &str, recipient: &str, note: &str, sender: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "AdaptiveCard",
        "version": "1.0",
        "body": [{
            "type": "ColumnSet",
            "spacing": "large",
            "separator": true,
            "columns": [
                {"type": "Column", "width": "auto", "items": [{
                    "type": "TextBlock", "text": badge,
                    "weight": "bolder", "size": "large", "horizontalAlignment": "center"
                }]},
                {"type": "Column", "width": "stretch", "spacing": "medium", "items": [
                    {"type": "TextBlock", "wrap": true, "size": "small", "isSubtle": true,
                     "text": format!("{} sent praise to", sender)},
                    {"type": "TextBlock", "wrap": true, "spacing": "small",
                     "weight": "bolder", "size": "medium", "text": recipient},
                    {"type": "TextBlock", "spacing": "small", "wrap": true, "text": note}
                ]}
            ]
        }]
    })
}

/// POST body for one Praise send (pure, Graph-documented shape): HTML
/// content naming the attachment id plus one Adaptive Card
/// attachment. `attachment_id` is caller-owned (a fresh GUID per
/// send); empty badge/recipient is rejected by the caller.
pub fn praise_message_body(
    badge: &str,
    recipient: &str,
    note: &str,
    sender: &str,
    attachment_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "content": format!("<attachment id=\"{}\"></attachment>", attachment_id),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "attachments": [{
            "id": attachment_id,
            "contentType": ADAPTIVE_CARD_CONTENT_TYPE,
            "content": praise_card_content(badge, recipient, note, sender)
        }]
    })
}

/// Praise send with a caller-owned `clientmessageid` (same idempotency
/// contract as [`send_message_with_client_id`]).
pub async fn send_praise_with_client_id(
    client: &TeamsClient,
    chat_id: &str,
    badge: &str,
    recipient: &str,
    note: &str,
    sender: &str,
    client_message_id: &str,
) -> Result<SentMessage> {
    if badge.trim().is_empty() {
        anyhow::bail!("empty praise badge");
    }
    if recipient.trim().is_empty() {
        anyhow::bail!("empty praise recipient");
    }
    if chat_id.trim().is_empty() {
        anyhow::bail!("empty chat id");
    }
    let url = format!(
        "{}/v1/users/ME/conversations/{}/messages",
        client.chat_service_url(),
        chat_id
    );
    let body = praise_message_body(
        badge,
        recipient,
        note,
        sender,
        &uuid::Uuid::new_v4().to_string(),
    );
    post_with_receipt(client, &url, body, client_message_id).await
}

// STICKERS: native sticker send + detect.
//
// The official client sends a sticker as an AMS-hosted image plus a
// chatsvc `RichText/Html` message (Burp capture, Turle 2022):
//   content: <p><readonly aria-label="L" contenteditable="false"
//     itemtype="http://schema.skype.com/Sticker" title="L"><img
//     src="{view}" width="{w}" height="{h}" itemscope="image/png"
//     itemtype="http://schema.skype.com/AMSImage" alt="{label}"
//     id="{object_id}" itemid="{object_id}" href="{view}"
//     target-src="{view}"></readonly></p>
//   messagetype RichText/Html, contenttype text,
//   amsreferences [object_id].
// The image bytes reach AMS first (Netwoven walkthrough + gist agree):
//   create: POST {asyncgw}/v1/objects, `Authorization: skype_token …`,
//     {"type": "pish/image","permissions":{conv:["read"]},"filename":…}
//     → {"id": object_id}
//   upload: PUT {asyncgw}/v1/objects/{id}/content/imgpsh (octet-stream).
// View: https://{cc}-api.asm.skype.com/v1/objects/{id}/views/imgo.
// Region map (observed): amer chat ↔ us-prod.asyncgw + us-api.asm
// (Netwoven); emea chat ↔ eu-api.asm (Turle Burp). eu-prod.asyncgw is
// INFERRED (euno-prod.asyncgw sighted in media tests) — the trial must
// verify it; unknown regions bail naming the host.
// ---------------------------------------------------------------------------

/// `itemtype` the official client stamps on sticker wrappers.
pub const STICKER_ITEMTYPE: &str = "http://schema.skype.com/Sticker";

/// Largest sticker file accepted (mirrors the simple-upload cap in
/// files.rs: stickers are small PNGs, never multi-MB).
pub const STICKER_IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Largest sticker edge the wire carries (official clients send
/// ~300px; hostile/odd files never become giant bubbles).
pub const STICKER_EDGE_MAX: u32 = 2048;

/// Fallback alt text when the caller passes a blank label.
pub const STICKER_DEFAULT_LABEL: &str = "Sticker";

/// Effective label: trimmed, or the default when blank.
pub fn sticker_effective_label(label: &str) -> String {
    let t = label.trim();
    if t.is_empty() {
        STICKER_DEFAULT_LABEL.to_string()
    } else {
        t.to_string()
    }
}

/// Guard the sticker POST args. Returns the effective label.
/// Object ids are server-minted (`0-…`); anything outside the id
/// charset or any non-https view URL is refused before any network.
fn check_sticker_args(
    object_id: &str,
    view_url: &str,
    label: &str,
    width: u32,
    height: u32,
) -> Result<String> {
    let oid = object_id.trim();
    if oid.is_empty() {
        bail!("empty sticker object id");
    }
    if !oid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("bad sticker object id");
    }
    let view = view_url.trim();
    let parsed = url::Url::parse(view).map_err(|_| anyhow::anyhow!("bad sticker view url"))?;
    if parsed.scheme() != "https" || parsed.host_str().unwrap_or("").is_empty() {
        bail!("bad sticker view url");
    }
    if width == 0 || height == 0 || width > STICKER_EDGE_MAX || height > STICKER_EDGE_MAX {
        bail!("bad sticker dimensions");
    }
    Ok(sticker_effective_label(label))
}

/// Sticker HTML for one AMS object (exact official-client shape).
pub fn sticker_content(
    object_id: &str,
    view_url: &str,
    label: &str,
    width: u32,
    height: u32,
) -> String {
    let label = sticker_effective_label(label);
    let e = html_escape(&label);
    let v = html_escape(view_url.trim());
    let oid = object_id.trim();
    format!(
        "<p><readonly aria-label=\"{e}\" contenteditable=\"false\" itemtype=\"http://schema.skype.com/Sticker\" title=\"{e}\"><img src=\"{v}\" width=\"{width}\" height=\"{height}\" itemscope=\"image/png\" itemtype=\"http://schema.skype.com/AMSImage\" alt=\"{e}\" id=\"{oid}\" itemid=\"{oid}\" href=\"{v}\" target-src=\"{v}\"></readonly></p>"
    )
}

/// POST body for one sticker message (captured-body seam for tests:
/// the exact JSON `chat_post` receives, minus transport).
pub fn sticker_message_body(
    object_id: &str,
    view_url: &str,
    label: &str,
    width: u32,
    height: u32,
) -> serde_json::Value {
    serde_json::json!({
        "content": sticker_content(object_id, view_url, label, width, height),
        "messagetype": "RichText/Html",
        "contenttype": "text",
        "amsreferences": [object_id.trim()],
        "properties": {"importance": "", "subject": ""}
    })
}

/// AMS create-object body for one sticker upload.
pub fn sticker_create_body(conversation_id: &str, filename: &str) -> serde_json::Value {
    let name = filename.trim();
    serde_json::json!({
        "type": "pish/image",
        "permissions": {conversation_id.trim(): ["read"]},
        "filename": if name.is_empty() { "sticker.png" } else { name }
    })
}

/// Server-minted object id from a create answer (pure).
pub fn parse_sticker_object_id(body: &serde_json::Value) -> Result<String> {
    match body.get("id").and_then(|v| v.as_str()).map(str::trim) {
        Some(id) if !id.is_empty() => Ok(id.to_string()),
        _ => bail!("sticker create answer named no object id"),
    }
}

/// PNG dimensions (width, height) from raw bytes: 8-byte signature +
/// `IHDR` first, big-endian words at 16..24. Anything else → None
/// (stickers are PNGs; GIF/JPEG never reach the wire as stickers).
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

/// True when raw HTML carries the official sticker wrapper
/// (case-insensitive; plain `<img>` photos are not stickers).
pub fn is_sticker_html(html: &str) -> bool {
    html.to_ascii_lowercase().contains("skype.com/sticker")
}

/// Sticker alt text from raw HTML: the `<img>` alt inside the sticker
/// wrapper, else its `aria-label`/`title`. Entity-decoded, trimmed;
/// None when absent, blank, or not a sticker at all.
pub fn sticker_label(html: &str) -> Option<String> {
    if !is_sticker_html(html) {
        return None;
    }
    let lower = html.to_ascii_lowercase();
    // Restrict mining to the readonly sticker span when present, so a
    // quoted photo's alt never poses as the sticker label.
    let scope = match lower.find("<readonly") {
        Some(s) => match lower[s..].find("</readonly>") {
            Some(e) => &html[s..s + e],
            None => &html[s..],
        },
        None => html,
    };
    attr_value(scope, "alt")
        .or_else(|| attr_value(scope, "aria-label"))
        .or_else(|| attr_value(scope, "title"))
        .map(|v| decode_entities(&v))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// First `name="…"` / `name='…'` / bare value in `tag`
/// (case-insensitive name; mirrors MessageRender's attr mining).
/// Mined on the original bytes so values keep their case (a lowercased
/// scratch copy would lowercase the label too).
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let want = name.as_bytes();
    if want.is_empty() || want.len() > bytes.len() {
        return None;
    }
    let mut i = 0;
    while i + want.len() <= bytes.len() {
        if bytes[i..i + want.len()].eq_ignore_ascii_case(want) {
            // Name must stand alone: `title` must not match `xtitle`.
            let before_ok = i == 0 || {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'-' || p == b'_' || p == b':')
            };
            let mut j = i + want.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if before_ok && j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() {
                    // Split points are ASCII delimiters, so every slice
                    // below lands on a char boundary.
                    if bytes[j] == b'"' || bytes[j] == b'\'' {
                        let q = bytes[j];
                        let start = j + 1;
                        let mut k = start;
                        while k < bytes.len() && bytes[k] != q {
                            k += 1;
                        }
                        return Some(tag[start..k].to_string());
                    }
                    let start = j;
                    while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>' {
                        j += 1;
                    }
                    if start < j {
                        return Some(tag[start..j].to_string());
                    }
                }
            }
            i = (j + 1).max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// The entity decoding `strip_html` applies, standalone for labels.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Upload sticker bytes to AMS: create-object then PUT content.
/// Returns the server-minted object id (pure guards first: no network
/// on empty/oversize input or a blank conversation id).
pub async fn upload_sticker_image_data(
    client: &TeamsClient,
    conversation_id: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<String> {
    let cid = conversation_id.trim();
    if cid.is_empty() {
        bail!("empty conversation id");
    }
    if bytes.is_empty() {
        bail!("empty sticker image");
    }
    if bytes.len() as u64 > STICKER_IMAGE_MAX_BYTES {
        bail!("sticker image too large");
    }
    let base = client.asyncgw_base()?;
    let create_url = format!("{base}/v1/objects");
    let answer = client
        .ams_post_json(&create_url, &sticker_create_body(cid, filename))
        .await?;
    let object_id = parse_sticker_object_id(&answer)?;
    let put_url = format!("{base}/v1/objects/{object_id}/content/imgpsh");
    client.ams_put_bytes(&put_url, bytes).await?;
    Ok(object_id)
}

/// View URL for one AMS object on this client's region host.
pub fn sticker_view_url(client: &TeamsClient, object_id: &str) -> Result<String> {
    let host = client.asm_view_host()?;
    Ok(format!(
        "https://{host}/v1/objects/{}/views/imgo",
        object_id.trim()
    ))
}

/// Post one sticker message with a caller-owned `clientmessageid`
/// (retries reuse it — same idempotency contract as text sends).
pub async fn send_sticker_message_with_client_id(
    client: &TeamsClient,
    chat_id: &str,
    object_id: &str,
    view_url: &str,
    label: &str,
    width: u32,
    height: u32,
    client_message_id: &str,
) -> Result<SentMessage> {
    if chat_id.trim().is_empty() {
        bail!("empty chat id");
    }
    let label = check_sticker_args(object_id, view_url, label, width, height)?;
    let url = format!(
        "{}/v1/users/ME/conversations/{}/messages",
        client.chat_service_url(),
        chat_id.trim()
    );
    post_with_receipt(
        client,
        &url,
        sticker_message_body(object_id, view_url, &label, width, height),
        client_message_id,
    )
    .await
}

/// Send a local PNG file as a sticker: probe dims, upload to AMS,
/// post the sticker message. PNG-only (probe fails otherwise).
pub async fn send_sticker(chat_id: &str, png_path: &str, label: &str) -> Result<()> {
    let bytes = std::fs::read(png_path)
        .with_context(|| format!("cannot read sticker file {png_path}"))?;
    let (width, height) =
        png_dimensions(&bytes).context("sticker file must be a PNG (dimensions unreadable)")?;
    let client = TeamsClient::new().await?;
    let filename = std::path::Path::new(png_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sticker.png");
    let object_id = upload_sticker_image_data(&client, chat_id, filename, &bytes).await?;
    let view = sticker_view_url(&client, &object_id)?;
    let sent = send_sticker_message_with_client_id(
        &client,
        chat_id,
        &object_id,
        &view,
        label,
        width,
        height,
        &new_client_message_id(),
    )
    .await?;
    match sent.id {
        Some(id) => println!("Sticker sent ({id})."),
        None => println!("Sticker sent."),
    }
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

    // PRAISE: the card body carries badge/recipient/note in the
    // Graph-documented attachment shape; the attachment id matches
    // the content's <attachment> tag.
    #[test]
    fn praise_body_attachment_shape() {
        let id = "11111111-2222-3333-4444-555555555555";
        let b = praise_message_body("Awesome", "Jane", "Nice work", "Sam", id);
        assert_eq!(b["messagetype"], "RichText/Html");
        assert_eq!(b["content"], format!("<attachment id=\"{}\"></attachment>", id));
        let atts = &b["attachments"];
        assert_eq!(atts[0]["id"], id);
        assert_eq!(atts[0]["contentType"], ADAPTIVE_CARD_CONTENT_TYPE);
        let card = &atts[0]["content"];
        assert_eq!(card["type"], "AdaptiveCard");
        let flat = format!("{}", card);
        for want in ["Awesome", "Jane", "Nice work", "Sam sent praise to"] {
            assert!(flat.contains(want), "missing {} in {}", want, flat);
        }
    }
}

#[cfg(test)]
mod sticker_tests {
    use super::*;

    // -- stickers -----------------------------------------------------------

    const STICKER_HTML: &str = "<p><readonly aria-label=\"Party!\" contenteditable=\"false\" itemtype=\"http://schema.skype.com/Sticker\" title=\"Party!\"><img src=\"https://us-api.asm.skype.com/v1/objects/0-test-1/views/imgo\" width=\"100\" height=\"80\" itemscope=\"image/png\" itemtype=\"http://schema.skype.com/AMSImage\" alt=\"Party!\" id=\"0-test-1\" itemid=\"0-test-1\" href=\"https://us-api.asm.skype.com/v1/objects/0-test-1/views/imgo\" target-src=\"https://us-api.asm.skype.com/v1/objects/0-test-1/views/imgo\"></readonly></p>";

    #[test]
    fn sticker_body_matches_official_wire_shape() {
        let view = "https://us-api.asm.skype.com/v1/objects/0-test-1/views/imgo";
        let b = sticker_message_body("0-test-1", view, "Party!", 100, 80);
        assert_eq!(b["messagetype"], "RichText/Html");
        assert_eq!(b["contenttype"], "text");
        assert_eq!(b["amsreferences"], serde_json::json!(["0-test-1"]));
        assert_eq!(b["properties"], serde_json::json!({"importance": "", "subject": ""}));
        assert_eq!(b["content"].as_str().unwrap(), STICKER_HTML);
        // Label is the alt text: hostile markup never lands raw.
        let evil = sticker_message_body("0-test-1", view, "<b>\"x\"</b>", 100, 80);
        let content = evil["content"].as_str().unwrap();
        assert!(content.contains("alt=\"&lt;b&gt;&quot;x&quot;&lt;/b&gt;\""));
        assert!(!content.contains("<b>"));
        // Blank label falls back; surrounding space trims.
        assert!(sticker_content("0-a", view, "  ", 10, 10).contains("alt=\"Sticker\""));
    }

    #[test]
    fn sticker_create_body_shape_and_filename_default() {
        let b = sticker_create_body("19:s@thread.v2", "party.png");
        assert_eq!(
            b,
            serde_json::json!({"type": "pish/image", "permissions": {"19:s@thread.v2": ["read"]}, "filename": "party.png"})
        );
        assert_eq!(sticker_create_body("19:s@thread.v2", "  ")["filename"], "sticker.png");
        assert_eq!(sticker_effective_label("  hi  "), "hi");
        assert_eq!(sticker_effective_label("   "), STICKER_DEFAULT_LABEL);
        assert!(parse_sticker_object_id(&serde_json::json!({"id": " 0-x "})).unwrap() == "0-x");
        assert!(parse_sticker_object_id(&serde_json::json!({})).is_err());
        assert!(parse_sticker_object_id(&serde_json::json!({"id": " "})).is_err());
    }

    #[test]
    fn png_dimensions_reads_ihdr_rejects_junk() {
        // 24-byte minimal PNG: sig + len + IHDR + 334x250.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&334u32.to_be_bytes());
        png.extend_from_slice(&250u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((334, 250)));
        assert_eq!(png_dimensions(b"GIF89a..."), None);
        assert_eq!(png_dimensions(&png[..10]), None);
        let mut zero = png.clone();
        zero[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(png_dimensions(&zero), None);
    }

    #[test]
    fn sticker_detect_and_label_mining() {
        assert!(is_sticker_html(STICKER_HTML));
        assert!(is_sticker_html(&STICKER_HTML.to_uppercase().replace("SKYPE.COM/STICKER", "skype.com/sticker")));
        assert!(!is_sticker_html("<p><img src=\"https://h/x.png\"></p>"));
        assert!(!is_sticker_html("<p>hi</p>"));
        assert_eq!(sticker_label(STICKER_HTML).as_deref(), Some("Party!"));
        // aria-label fallback; entity-decoded; quoted-photo alts ignored.
        let aria = "<p><readonly itemtype=\"http://schema.skype.com/Sticker\" aria-label=\"Go &amp; win\"><img src=\"x\"></readonly></p>";
        assert_eq!(sticker_label(aria).as_deref(), Some("Go & win"));
        let title = "<readonly itemType=\"http://schema.skype.com/Sticker\" TITLE=\"Tada\"><img src=\"x\"></readonly>";
        assert_eq!(sticker_label(title).as_deref(), Some("Tada"));
        assert_eq!(sticker_label("<p><img src=\"x\" alt=\"photo\"></p>"), None);
        // Quoted photo alt outside the span never poses as the label.
        let mixed = format!("<p><img src=\"q\" alt=\"quoted\"></p>{STICKER_HTML}");
        assert_eq!(sticker_label(&mixed).as_deref(), Some("Party!"));
    }
}
