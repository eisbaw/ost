//! Native Teams chat API (chatsvcagg / chat service)
//!
//! Uses the Skype token with `Authentication: skypetoken={token}` header,
//! bypassing Graph API which requires tenant admin consent for Chat.Read.

use anyhow::{bail, Context, Result};
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
    /// OstMac om-reactions: per-message reactions when the server sends
    /// them (Graph-like list). Absent on old payloads → no counts.
    reactions: Option<Vec<NativeReaction>>,
    /// Alternate nesting some payloads use (`properties.reactions`).
    properties: Option<MessageProperties>,
}

#[derive(Debug, Deserialize)]
struct MessageProperties {
    reactions: Option<Vec<NativeReaction>>,
}

/// One raw reaction entry. Only the type is aggregated; user/count
/// variants ride along unparsed so unknown shapes still deserialize.
#[derive(Debug, Deserialize)]
struct NativeReaction {
    #[serde(rename = "reactionType")]
    reaction_type: Option<String>,
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
// Leave chat (OstMac om-leave-block lane)
// ---------------------------------------------------------------------------
//
// Self-removal from a thread's roster: DELETE .../v1/threads/{id}/members/{mri}
// with skypetoken auth, where the member MRI is the signed-in user's own
// (`8:orgid:{oid}` from whoami). Best-effort: NOT yet verified live against
// the server (see OSTMAC-PATCHES.md §27). Targets group threads; 1:1
// threads are hidden client-side instead (the block flow).

/// Own roster MRI for an Entra object id (`8:orgid:{oid}`).
pub fn own_member_mri(oid: &str) -> String {
    format!("8:orgid:{}", oid.trim())
}

/// DELETE target for removing one member from a thread's roster.
pub fn leave_member_url(base: &str, chat_id: &str, member_mri: &str) -> String {
    format!(
        "{}/v1/threads/{}/members/{}",
        base, chat_id, member_mri
    )
}

/// Leave one chat: remove self from the thread roster. Empty ids are
/// rejected before any network; the own MRI resolves via whoami.
pub async fn leave_chat_with_client(client: &TeamsClient, chat_id: &str) -> Result<()> {
    if chat_id.trim().is_empty() {
        anyhow::bail!("empty chat_id");
    }
    let me = whoami_data(client).await?;
    if me.id.trim().is_empty() {
        anyhow::bail!("empty owner id");
    }
    let base = client.chat_service_url();
    let url = leave_member_url(&base, chat_id.trim(), &own_member_mri(&me.id));
    tracing::debug!("Leaving chat at {}", url);
    client.chat_delete(&url, None).await?;
    Ok(())
}

/// Leave one chat thread (prints to stdout).
pub async fn leave_chat(chat_id: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    leave_chat_with_client(&client, chat_id).await?;
    println!("Left chat.");
    Ok(())
}

// ---------------------------------------------------------------------------
// 1:1 chat create (om-lt5-person11: person-pick opens 1:1)
// ---------------------------------------------------------------------------

/// Chat-service 1:1 thread id for two AAD object ids (pure):
/// `19:<lo>_<hi>@unq.gbl.spaces`, ids lowercased and in ascending order.
/// The same pair always names the same thread, so an existing 1:1
/// re-opens instead of a duplicate being minted.
pub fn one_to_one_thread_id(a: &str, b: &str) -> String {
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    format!("19:{}_{}@unq.gbl.spaces", lo, hi)
}

fn looks_like_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(n, p)| p.len() == *n)
        && s.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// AAD object id for `me` or a user ref (an AAD id passes through; a UPN
/// or mail address resolves via Graph `GET /users/{ref}?$select=id`).
async fn aad_object_id(client: &TeamsClient, user: &str) -> Result<String> {
    let u = user.trim();
    if looks_like_guid(u) {
        return Ok(u.to_ascii_lowercase());
    }
    let path = if u == "me" {
        "/me?$select=id".to_string()
    } else {
        let seg: String = u
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'@' | b'.' | b'-' | b'_' => {
                    (b as char).to_string()
                }
                _ => format!("%{:02X}", b),
            })
            .collect();
        format!("/users/{}?$select=id", seg)
    };
    let v: serde_json::Value = client
        .graph_get(&path)
        .await?
        .json()
        .await
        .context("Failed to parse user id response")?;
    v["id"]
        .as_str()
        .filter(|s| looks_like_guid(s))
        .map(str::to_ascii_lowercase)
        .context("user id missing")
}

/// Chat-service body creating the 1:1 thread for two AAD ids (pure).
/// `uniquerosterthread` + `fixedRoster` make it the pair's one 1:1
/// (`19:<lo>_<hi>@unq.gbl.spaces`), never a group thread.
pub fn one_to_one_thread_body(me: &str, peer: &str) -> serde_json::Value {
    serde_json::json!({
        "members": [
            {"id": format!("8:orgid:{}", peer.trim().to_ascii_lowercase()), "role": "Admin"},
            {"id": format!("8:orgid:{}", me.trim().to_ascii_lowercase()), "role": "Admin"},
        ],
        "properties": {
            "threadType": "chat",
            "chatFilesIndexId": "2",
            "fixedRoster": "true",
            "uniquerosterthread": "true",
        },
    })
}

#[derive(Debug, Deserialize)]
struct CreatedChat {
    id: String,
    topic: Option<String>,
}

/// Parse a `POST /me/chats` 1:1 response into a chat row. Graph
/// returns no topic for 1:1s — the caller names the thread after
/// the peer. Pure so tests pin it.
pub fn parse_created_chat(value: &serde_json::Value) -> Result<ChatInfo> {
    let chat: CreatedChat = serde_json::from_value(value.clone())
        .context("Failed to parse created chat response")?;
    Ok(ChatInfo {
        id: chat.id,
        name: chat.topic.unwrap_or_default(),
        is_group: false,
        last_message_time: None,
        last_message_sender: None,
        last_message_preview: None,
    })
}

/// Create (or re-open) the 1:1 chat with `user` (AAD id or UPN) and
/// return the thread. Empty refs are rejected before any network.
/// Graph `POST /me/chats` is not a create endpoint (405) and Graph
/// `POST /chats` needs Chat.Create, which the Teams web token lacks, so
/// the thread is opened on the chat service instead: both AAD ids map to
/// the derived thread id ([`one_to_one_thread_id`]); an existing thread
/// re-opens; a first contact creates the unique-roster 1:1.
pub async fn create_one_to_one_chat_data(
    client: &TeamsClient,
    user: &str,
) -> Result<ChatInfo> {
    if user.trim().is_empty() {
        bail!("empty user");
    }
    let me = aad_object_id(client, "me").await?;
    let peer = aad_object_id(client, user).await?;
    if me == peer {
        bail!("cannot open a 1:1 chat with yourself");
    }
    let derived = one_to_one_thread_id(&me, &peer);
    let base = client.chat_service_url();
    let probe = format!("{}/v1/threads/{}?view=msnp24Equivalent", base, derived);
    let missing = match client.chat_get(&probe).await {
        Ok(_) => false,
        Err(e) => format!("{:#}", e).starts_with("HTTP 404"),
    };
    if missing {
        let resp = client
            .chat_post(&format!("{}/v1/threads", base), &one_to_one_thread_body(&me, &peer))
            .await
            .context("1:1 thread create failed")?;
        let made = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|l| l.split('?').next())
            .and_then(|l| l.rsplit('/').next())
            .map(|id| id.replace("%3A", ":").replace("%3a", ":").replace("%40", "@"))
            .unwrap_or_default();
        if !made.is_empty() && made != derived {
            bail!("1:1 thread create answered an unexpected thread");
        }
    }
    Ok(ChatInfo {
        id: derived,
        name: String::new(),
        is_group: false,
        last_message_time: None,
        last_message_sender: None,
        last_message_preview: None,
    })
}

// ---------------------------------------------------------------------------
// Group chat create
// ---------------------------------------------------------------------------

/// Chat-service thread create URL. Pure so tests pin it.
pub fn thread_create_url(base: &str) -> String {
    format!("{}/v1/threads", base.trim_end_matches('/'))
}

/// Trim, drop blanks, and de-duplicate user refs (case-insensitive,
/// first spelling wins); `self_id` is removed (it is added as the
/// creator). Pure so tests pin it.
pub fn group_chat_members(self_id: &str, users: &[String]) -> Vec<String> {
    let me = self_id.trim().to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for u in users {
        let t = u.trim();
        if t.is_empty() || t.to_lowercase() == me {
            continue;
        }
        if out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// Chat-service body creating a group chat thread for AAD object ids:
/// the creator first, then every peer, all `Admin` (Teams group chats let
/// every member manage the roster); blank topics are omitted. `peers`
/// must already be normalized ([`group_chat_members`]) and resolved to
/// object ids. Pure so tests pin it.
pub fn group_chat_create_body(
    self_id: &str,
    peers: &[String],
    topic: Option<&str>,
) -> serde_json::Value {
    let member = |u: &str| {
        serde_json::json!({
            "id": format!("8:orgid:{}", u.trim().to_ascii_lowercase()),
            "role": "Admin",
        })
    };
    let mut members = vec![member(self_id)];
    members.extend(peers.iter().map(|u| member(u)));
    let mut properties = serde_json::json!({
        "threadType": "chat",
        "chatFilesIndexId": "2",
    });
    if let Some(t) = topic.map(str::trim).filter(|t| !t.is_empty()) {
        properties["topic"] = serde_json::Value::String(t.to_string());
    }
    serde_json::json!({ "members": members, "properties": properties })
}

/// Thread id from a chat-service thread-create `Location` header
/// (`.../v1/threads/19%3A...%40thread.v2?...`), decoded; None when absent
/// or not a thread id. Pure so tests pin it.
pub fn created_thread_id(location: Option<&str>) -> Option<String> {
    let id = location?
        .split('?')
        .next()?
        .rsplit('/')
        .next()?
        .replace("%3A", ":")
        .replace("%3a", ":")
        .replace("%40", "@");
    (id.starts_with("19:") && id.contains('@')).then_some(id)
}

/// Create a group chat with `users` (AAD ids or UPNs) plus the signed-in
/// user, with an optional topic, on the chat service (`POST /v1/threads`,
/// skype token). Graph `POST /chats` needs Chat.Create, which the Teams
/// web client token does not carry (403). UPNs resolve to object ids via
/// Graph `GET /users/{ref}?$select=id`. At least one peer is required
/// (checked before any network). Returns the new thread (`is_group`
/// true) named after the topic.
/// Create a group chat with `users` (AAD ids or UPNs) plus the signed-in
/// user, with an optional topic, via Graph `POST /chats`. At least one
/// peer is required (checked before any network); the server enforces
/// its own minimums. Returns the new thread (`is_group` true).
pub async fn create_group_chat_data(
    client: &TeamsClient,
    users: &[String],
    topic: Option<&str>,
) -> Result<ChatInfo> {
    if group_chat_members("", users).is_empty() {
        bail!("no members");
    }
    let me = aad_object_id(client, "me").await?;
    let mut peers: Vec<String> = Vec::new();
    for u in group_chat_members(&me, users) {
        let oid = aad_object_id(client, &u).await?;
        if oid != me && !peers.contains(&oid) {
            peers.push(oid);
        }
    }
    if peers.is_empty() {
        bail!("no members besides self");
    }
    let resp = client
        .chat_post(
            &thread_create_url(&client.chat_service_url()),
            &group_chat_create_body(&me, &peers, topic),
        )
        .await
        .context("group chat create failed")?;
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let id = created_thread_id(location.as_deref())
        .context("group chat create answered no thread id")?;
    Ok(ChatInfo {
        id,
        name: topic.map(str::trim).unwrap_or_default().to_string(),
        is_group: true,
        last_message_time: None,
        last_message_sender: None,
        last_message_preview: None,
    })
}

// ---------------------------------------------------------------------------
// Reactions (OstMac om-reactions lane)
// ---------------------------------------------------------------------------
//
// Wire shape mirrors the Graph chatMessageReaction resource
// (`POST .../messages/{id}/reactions`, `{"reactionType": "like"}`) against
// the native chat service with skypetoken auth. Best-effort: NOT yet
// verified live against the server (see OSTMAC-PATCHES.md §17).

/// Picker emoji → Teams reaction type, in picker order.
/// (like, heart, laugh, surprised, sad, angry — the Graph-supported six.)
pub const REACTION_EMOJI: &[(&str, &str)] = &[
    ("👍", "like"),
    ("❤️", "heart"),
    ("😂", "laugh"),
    ("😮", "surprised"),
    ("😢", "sad"),
    ("😠", "angry"),
];

/// Reaction type for a picker emoji, or None when unsupported.
pub fn reaction_type_for_emoji(emoji: &str) -> Option<&'static str> {
    REACTION_EMOJI
        .iter()
        .find(|(e, _)| *e == emoji)
        .map(|(_, t)| *t)
}

/// Picker emoji for a server reaction type (case-insensitive), or None
/// when unknown. Unknown types are dropped from counts, never fatal.
pub fn emoji_for_reaction_type(reaction_type: &str) -> Option<&'static str> {
    REACTION_EMOJI
        .iter()
        .find(|(_, t)| t.eq_ignore_ascii_case(reaction_type))
        .map(|(e, _)| *e)
}

/// POST target for adding a reaction to one message.
pub fn reaction_add_url(base: &str, chat_id: &str, message_id: &str) -> String {
    format!(
        "{}/v1/users/ME/conversations/{}/messages/{}/reactions",
        base, chat_id, message_id
    )
}

/// POST body for adding a reaction.
pub fn reaction_add_body(reaction_type: &str) -> serde_json::Value {
    serde_json::json!({ "reactionType": reaction_type })
}

/// DELETE target for removing one reaction type from a message.
pub fn reaction_remove_url(
    base: &str,
    chat_id: &str,
    message_id: &str,
    reaction_type: &str,
) -> String {
    format!(
        "{}/v1/users/ME/conversations/{}/messages/{}/reactions/{}",
        base, chat_id, message_id, reaction_type
    )
}

/// Add one emoji reaction to a message. Unknown emoji is rejected before
/// any network.
pub async fn send_reaction_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message_id: &str,
    emoji: &str,
) -> Result<()> {
    let reaction_type = reaction_type_for_emoji(emoji)
        .with_context(|| format!("unsupported reaction emoji: {}", emoji))?;
    let base = client.chat_service_url();
    let url = reaction_add_url(&base, chat_id, message_id);
    let body = reaction_add_body(reaction_type);
    tracing::debug!("Adding {} reaction to {}", reaction_type, url);
    client.chat_post(&url, &body).await?;
    Ok(())
}

/// Remove one emoji reaction from a message. Unknown emoji is rejected
/// before any network.
pub async fn remove_reaction_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message_id: &str,
    emoji: &str,
) -> Result<()> {
    let reaction_type = reaction_type_for_emoji(emoji)
        .with_context(|| format!("unsupported reaction emoji: {}", emoji))?;
    let base = client.chat_service_url();
    let url = reaction_remove_url(&base, chat_id, message_id, reaction_type);
    tracing::debug!("Removing {} reaction from {}", reaction_type, url);
    client.chat_delete(&url, None).await?;
    Ok(())
}

/// Add or remove a reaction (prints to stdout). CLI entry point.
pub async fn react(chat_id: &str, message_id: &str, emoji: &str, remove: bool) -> Result<()> {
    let client = TeamsClient::new().await?;
    if remove {
        remove_reaction_with_client(&client, chat_id, message_id, emoji).await?;
        println!("Reaction removed.");
    } else {
        send_reaction_with_client(&client, chat_id, message_id, emoji).await?;
        println!("Reaction added.");
    }
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
    /// Grouped reaction counts (om-reactions). Empty when the server sent
    /// none; unknown reaction types are dropped, never fatal.
    pub reactions: Vec<ReactionCount>,
}

/// One grouped reaction count: picker emoji + number of reactors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionCount {
    pub emoji: String,
    pub count: usize,
}

/// Group raw reaction entries into per-emoji counts in canonical picker
/// order. Entries with missing/unknown types are skipped.
fn aggregate_reactions(entries: &[NativeReaction]) -> Vec<ReactionCount> {
    let mut counts = vec![0usize; REACTION_EMOJI.len()];
    for e in entries {
        let Some(t) = e.reaction_type.as_deref() else {
            continue;
        };
        if let Some(i) = REACTION_EMOJI
            .iter()
            .position(|(_, known)| known.eq_ignore_ascii_case(t))
        {
            counts[i] += 1;
        }
    }
    REACTION_EMOJI
        .iter()
        .zip(counts)
        .filter(|(_, c)| *c > 0)
        .map(|((emoji, _), count)| ReactionCount {
            emoji: emoji.to_string(),
            count,
        })
        .collect()
}

/// Reaction entries for one message: top-level `reactions` wins, then
/// `properties.reactions`. Neither present → empty.
fn message_reactions(msg: &NativeMessage) -> Vec<ReactionCount> {
    if let Some(list) = msg.reactions.as_deref() {
        return aggregate_reactions(list);
    }
    if let Some(props) = msg.properties.as_ref() {
        if let Some(list) = props.reactions.as_deref() {
            return aggregate_reactions(list);
        }
    }
    Vec::new()
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
        let reactions = message_reactions(msg);
        result.push(MessageInfo {
            id,
            sender,
            timestamp: time,
            content: text.trim().to_string(),
            raw: content.to_string(),
            reactions,
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
    fn one_to_one_thread_id_is_ordered_pair() {
        let a = "527A0000-0000-0000-0000-000000000001";
        let b = "01d90000-0000-0000-0000-000000000002";
        let want = "19:01d90000-0000-0000-0000-000000000002_527a0000-0000-0000-0000-000000000001@unq.gbl.spaces";
        assert_eq!(one_to_one_thread_id(a, b), want);
        assert_eq!(one_to_one_thread_id(b, a), want);
        assert!(looks_like_guid(b));
        assert!(!looks_like_guid("someone@example.org"));
    }

    #[test]
    fn one_to_one_thread_body_shape() {
        let b = one_to_one_thread_body(" ME-1 ", "Peer-2");
        assert_eq!(b["members"][0]["id"], "8:orgid:peer-2");
        assert_eq!(b["members"][1]["id"], "8:orgid:me-1");
        assert_eq!(b["members"][0]["role"], "Admin");
        assert_eq!(b["properties"]["threadType"], "chat");
        assert_eq!(b["properties"]["uniquerosterthread"], "true");
        assert_eq!(b["properties"]["fixedRoster"], "true");
    }

    #[test]
    fn created_chat_parse_tolerates_shapes() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"id":"19:one@unq.v1"}"#).unwrap();
        let c = parse_created_chat(&v).unwrap();
        assert_eq!(c.id, "19:one@unq.v1");
        assert_eq!(c.name, "");
        assert!(!c.is_group);
        let v: serde_json::Value =
            serde_json::from_str(r#"{"id":"19:g@t","topic":"T"}"#).unwrap();
        assert_eq!(parse_created_chat(&v).unwrap().name, "T");
        let v: serde_json::Value = serde_json::from_str(r#"{"nope":1}"#).unwrap();
        assert!(parse_created_chat(&v).is_err());
    }

    #[test]
    fn leave_url_and_mri_shape() {
        assert_eq!(own_member_mri("abc-123"), "8:orgid:abc-123");
        assert_eq!(own_member_mri("  abc-123  "), "8:orgid:abc-123");
        assert_eq!(
            leave_member_url("https://h", "19:t@thread.v2", "8:orgid:abc-123"),
            "https://h/v1/threads/19:t@thread.v2/members/8:orgid:abc-123"
        );
    }

    #[test]
    fn reaction_emoji_round_trip() {
        assert_eq!(REACTION_EMOJI.len(), 6);
        for (emoji, rtype) in REACTION_EMOJI {
            assert_eq!(reaction_type_for_emoji(emoji), Some(*rtype));
            assert_eq!(emoji_for_reaction_type(rtype), Some(*emoji));
        }
        assert_eq!(reaction_type_for_emoji("🎉"), None);
        assert_eq!(reaction_type_for_emoji(""), None);
        assert_eq!(emoji_for_reaction_type("party"), None);
        assert_eq!(emoji_for_reaction_type("LIKE"), Some("👍"));
    }

    #[test]
    fn reaction_endpoint_shapes() {
        let base = "https://h";
        assert_eq!(
            reaction_add_url(base, "19:thread", "42"),
            "https://h/v1/users/ME/conversations/19:thread/messages/42/reactions"
        );
        assert_eq!(
            reaction_add_body("like"),
            serde_json::json!({ "reactionType": "like" })
        );
        assert_eq!(
            reaction_remove_url(base, "19:thread", "42", "like"),
            "https://h/v1/users/ME/conversations/19:thread/messages/42/reactions/like"
        );
    }

    fn reacted(content: &str, reactions_json: &str, via_properties: bool) -> NativeMessage {
        let payload = if via_properties {
            format!(
                r#"{{"id":"1","messagetype":"RichText/Html","content":{},"properties":{{"reactions":{}}}}}"#,
                serde_json::to_string(content).unwrap(),
                reactions_json
            )
        } else {
            format!(
                r#"{{"id":"1","messagetype":"RichText/Html","content":{},"reactions":{}}}"#,
                serde_json::to_string(content).unwrap(),
                reactions_json
            )
        };
        serde_json::from_str(&payload).unwrap()
    }

    #[test]
    fn reactions_group_in_picker_order() {
        let msg = reacted(
            "<p>hi</p>",
            r#"[{"reactionType":"laugh"},{"reactionType":"like"},{"reactionType":"like"}]"#,
            false,
        );
        assert_eq!(
            message_reactions(&msg),
            vec![
                ReactionCount {
                    emoji: "👍".to_string(),
                    count: 2
                },
                ReactionCount {
                    emoji: "😂".to_string(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn reactions_nested_and_unknown_shapes() {
        // properties.reactions nesting works; unknown/missing types drop.
        let msg = reacted(
            "<p>hi</p>",
            r#"[{"reactionType":"Heart"},{"reactionType":"party"},{"reactionType":null},{}]"#,
            true,
        );
        assert_eq!(
            message_reactions(&msg),
            vec![ReactionCount {
                emoji: "❤️".to_string(),
                count: 1
            }]
        );
        // No reactions key at all → empty, old payloads unaffected.
        let bare: NativeMessage =
            serde_json::from_str(r#"{"id":"1","content":"<p>hi</p>"}"#).unwrap();
        assert!(message_reactions(&bare).is_empty());
    }

    #[test]
    fn group_chat_body_and_member_normalization() {
        let users = vec![
            " b@x.com ".to_string(),
            "".to_string(),
            "B@X.com".to_string(),
            "me-oid".to_string(),
            "c-oid".to_string(),
        ];
        let peers = group_chat_members("ME-OID", &users);
        assert_eq!(peers, vec!["b@x.com".to_string(), "c-oid".to_string()]);
        assert_eq!(thread_create_url("https://h/"), "https://h/v1/threads");
        let b = group_chat_create_body("ME-oid", &peers, Some("  Launch  "));
        assert_eq!(b["properties"]["threadType"], "chat");
        assert_eq!(b["properties"]["topic"], "Launch");
        let m = b["members"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["id"], "8:orgid:me-oid");
        assert_eq!(m[2]["id"], "8:orgid:c-oid");
        assert!(m.iter().all(|x| x["role"] == "Admin"));
        assert!(group_chat_create_body("me", &peers, Some("  ")).pointer("/properties/topic").is_none());
        assert!(group_chat_create_body("me", &peers, None).pointer("/properties/topic").is_none());
        assert_eq!(
            created_thread_id(Some("https://h/v1/threads/19%3Aabc%40thread.v2?x=1")).as_deref(),
            Some("19:abc@thread.v2")
        );
        assert!(created_thread_id(Some("https://h/v1/threads/")).is_none());
        assert!(created_thread_id(None).is_none());
    }
}
