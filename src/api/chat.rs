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

/// Max fenced code blocks parsed per outbound message (hostile-input
/// cap); extra fences stay prose.
pub const WIRE_FENCE_MAX_BLOCKS: usize = 50;

/// One line classified as a fence opener: (fence char, run length).
/// Any indent, ``` or ~~~ runs of ≥ 3; info strings must not contain
/// the fence char (CommonMark).
fn wire_fence_opener(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start_matches([' ', '\t']);
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let len = t.chars().take_while(|&ch| ch == c).count();
    if len < 3 {
        return None;
    }
    let rest: String = t.chars().skip(len).collect();
    if rest.trim().is_empty() {
        return Some((c, len));
    }
    if rest.contains(c) {
        return None;
    }
    Some((c, len))
}

/// A closer line: same-char run ≥ opening length + nothing but
/// whitespace after (info-carrying lines never close).
fn wire_fence_closer(line: &str, ch: char, len: usize) -> bool {
    let t = line.trim_start_matches([' ', '\t']);
    let run = t.chars().take_while(|&c| c == ch).count();
    if run < len {
        return false;
    }
    t.chars().skip(run).collect::<String>().trim().is_empty()
}

/// True when the body needs more than the legacy single-`<p>`: a fence,
/// an inline-marker candidate, or a block-marker line. Anything else
/// keeps the legacy shape bit-identical (fast path below).
fn wire_has_rich(message: &str) -> bool {
    if message.contains("**") || message.contains("__") || message.contains("~~") || message.contains('*') {
        return true;
    }
    for line in message.split('\n') {
        if wire_fence_opener(line).is_some() {
            return true;
        }
        let t = line.trim_start_matches([' ', '\t']);
        if t.starts_with("- ") || t.starts_with("> ") || t.contains('|') || wire_ordered_prefix(t).is_some() {
            return true;
        }
    }
    false
}

/// A `1. ` / `12. ` list prefix: its length in bytes, else nil-ish.
/// (Digits are ASCII, so the byte length is a char boundary.)
fn wire_ordered_prefix(t: &str) -> Option<usize> {
    let digits = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    if t[digits..].starts_with(". ") {
        Some(digits + 2)
    } else {
        None
    }
}

/// Bulleted (`- `) vs ordered (`N. `) list line, else none. `* ` bullets
/// are `**`-ambiguous, so the toolbar emits `- ` and only that parses.
fn wire_list_kind(line: &str) -> Option<char> {
    let t = line.trim_start_matches([' ', '\t']);
    if t.starts_with("- ") {
        Some('u')
    } else if wire_ordered_prefix(t).is_some() {
        Some('o')
    } else {
        None
    }
}

/// Quote line (`> `, `>`): the text after the marker.
fn wire_is_quote(line: &str) -> bool {
    let t = line.trim_start_matches([' ', '\t']);
    t.starts_with("> ") || t == ">"
}

/// Strip one list marker, returning the item text.
fn wire_strip_list(line: &str, kind: char) -> &str {
    let t = line.trim_start_matches([' ', '\t']);
    if kind == 'u' {
        &t[2..]
    } else {
        &t[wire_ordered_prefix(t).unwrap_or(2)..]
    }
}

/// Strip one quote marker.
fn wire_strip_quote(line: &str) -> &str {
    let t = line.trim_start_matches([' ', '\t']);
    let rest = &t[1..];
    rest.strip_prefix(' ').unwrap_or(rest)
}

/// Backtick spans to protect from emphasis (mirrors Swift
/// `backtickSpans`: single tick, nonempty, no newline). Byte ranges;
/// backticks are ASCII so every boundary is a char boundary.
fn wire_tick_spans(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let Some(r1) = s[i..].find('`') else { break };
        let after = i + r1 + 1;
        let Some(r2) = s[after..].find('`') else { break };
        let cc = after + r2;
        let inner = &s[after..cc];
        if !inner.is_empty() && !inner.contains('\n') {
            out.push((after, cc));
        }
        i = cc + 1;
    }
    out
}

/// Inline composer markers → HTML tags. Runs on escaped text (markers
/// are ASCII, so escaping never disturbs them); backtick spans stay
/// literal (the wire carries no `<code>`; Swift renders them mono).
/// Strict like Swift `emphMatches`: non-space edges, no newlines,
/// no overlap (`**` beats `*`).
fn wire_inline_html(s: &str) -> String {
    let ticks = wire_tick_spans(s);
    let mut claimed: Vec<(usize, usize)> = Vec::new();
    // (open_start, open_end, close_start, close_end, tag).
    let mut matches: Vec<(usize, usize, usize, usize, &str)> = Vec::new();
    for (m, tag) in [("**", "strong"), ("~~", "s"), ("__", "u"), ("*", "em")] {
        let mut i = 0;
        while i < s.len() {
            let Some(rel) = s[i..].find(m) else { break };
            let o = i + rel;
            let ao = o + m.len();
            if !s[ao..].chars().next().map(|c| !c.is_whitespace()).unwrap_or(false) {
                i = o + 1;
                continue;
            }
            let mut c = ao;
            let mut found: Option<usize> = None;
            while let Some(r2) = s[c..].find(m) {
                let cc = c + r2;
                let content = &s[ao..cc];
                let whole = (o, cc + m.len());
                let prev_ok = s[..cc].chars().next_back().map(|ch| !ch.is_whitespace()).unwrap_or(true);
                // A single marker never spans a doubled edge ("** not
                // bold **" is not italic with "*" trims); mirrors Swift.
                let doubled_edge =
                    m.len() == 1 && content.starts_with(m) && content.ends_with(m);
                let hit = ticks.iter().any(|t| whole.0 < t.1 && t.0 < whole.1)
                    || claimed.iter().any(|t| whole.0 < t.1 && t.0 < whole.1);
                if !content.is_empty() && !doubled_edge && prev_ok && !content.contains('\n') && !hit {
                    found = Some(cc);
                    break;
                }
                c = cc + 1;
            }
            if let Some(cc) = found {
                claimed.push((o, cc + m.len()));
                matches.push((o, ao, cc, cc + m.len(), tag));
                i = cc + m.len();
            } else {
                i = o + 1;
            }
        }
    }
    matches.sort_by_key(|x| x.0);
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    for (os, oe, cs, ce, tag) in matches {
        out.push_str(&s[pos..os]);
        out.push_str(&format!("<{tag}>"));
        out.push_str(&s[oe..cs]);
        out.push_str(&format!("</{tag}>"));
        pos = ce;
    }
    out.push_str(&s[pos..]);
    out
}

/// One table row split into cells: border pipes dropped, cells trimmed.
/// No backslash-escape support (v1, documented).
fn wire_split_row(line: &str) -> Vec<String> {
    let t = line.trim().trim_matches('|');
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// A delimiter cell: `:?-+:?` (at least one dash).
fn wire_is_delim_cell(cell: &str) -> bool {
    let t = cell.trim();
    let t = t.strip_prefix(':').unwrap_or(t);
    let t = t.strip_suffix(':').unwrap_or(t);
    !t.is_empty() && t.bytes().all(|b| b == b'-')
}

/// Consecutive `|`-lines forming header + delimiter + body: the table
/// HTML plus lines consumed, else `None` (lines stay prose).
fn wire_table_group(lines: &[&str]) -> Option<(String, usize)> {
    if lines.len() < 2 || !lines[0].contains('|') {
        return None;
    }
    let delim = wire_split_row(lines[1]);
    if delim.is_empty() || !delim.iter().all(|c| wire_is_delim_cell(c)) {
        return None;
    }
    let head = wire_split_row(lines[0]);
    if head.is_empty() {
        return None;
    }
    let width = head.len().max(delim.len()).max(1);
    let mut out = String::from("<table><tr>");
    let mut cells = head;
    cells.resize(width, String::new());
    for c in &cells {
        out.push_str(&format!("<th>{}</th>", wire_inline_html(&html_escape(c))));
    }
    out.push_str("</tr>");
    let mut n = 2;
    while n < lines.len() && lines[n].contains('|') {
        let mut body = wire_split_row(lines[n]);
        body.resize(width, String::new());
        out.push_str("<tr>");
        for c in &body {
            out.push_str(&format!("<td>{}</td>", wire_inline_html(&html_escape(c))));
        }
        out.push_str("</tr>");
        n += 1;
    }
    out.push_str("</table>");
    Some((out, n))
}

/// Prose segment → HTML. Block groups (lists, quotes, tables) become
/// block tags; everything else stays in legacy `<p>` paragraphs with
/// `\n` preserved — marker-free prose renders bit-identical to before.
/// Inline markers render inside every block kind.
fn render_prose_html(prose: &str) -> String {
    let lines: Vec<&str> = prose.split('\n').collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        if wire_is_quote(lines[i]) {
            let mut items = Vec::new();
            while i < lines.len() && wire_is_quote(lines[i]) {
                items.push(wire_inline_html(&html_escape(wire_strip_quote(lines[i]))));
                i += 1;
            }
            out.push_str(&format!("<blockquote>{}</blockquote>", items.join("<br/>")));
            continue;
        }
        if let Some(kind) = wire_list_kind(lines[i]) {
            let tag = if kind == 'u' { "ul" } else { "ol" };
            let mut items = String::new();
            while i < lines.len() && wire_list_kind(lines[i]) == Some(kind) {
                items.push_str(&format!(
                    "<li>{}</li>",
                    wire_inline_html(&html_escape(wire_strip_list(lines[i], kind)))
                ));
                i += 1;
            }
            out.push_str(&format!("<{tag}>{items}</{tag}>"));
            continue;
        }
        if let Some((table, n)) = wire_table_group(&lines[i..]) {
            out.push_str(&table);
            i += n;
            continue;
        }
        let mut para = String::new();
        while i < lines.len()
            && !wire_is_quote(lines[i])
            && wire_list_kind(lines[i]).is_none()
            && wire_table_group(&lines[i..]).is_none()
        {
            if !para.is_empty() {
                para.push('\n');
            }
            para.push_str(lines[i]);
            i += 1;
        }
        if !para.trim().is_empty() {
            out.push_str(&format!("<p>{}</p>", wire_inline_html(&html_escape(&para))));
        }
    }
    out
}

/// Outbound wire HTML for a composer body. Marker-free messages keep the
/// legacy single-`<p>` shape bit-identical; composer markdown
/// (COMPOSER-A: `**bold**` `*italic*` `__underline__` `~~strike~~`,
/// `- `/`1. ` lists, `> ` quotes, `|` tables) renders to Teams HTML so
/// every client shows the styling. Each fenced block becomes a `<pre>`
/// (HTML preserves its newlines/indents in every client). Fence lines
/// are consumed, info strings dropped (the wire carries no
/// highlighter), code interiors byte-exact modulo HTML-escaping.
/// Unclosed fences run to end of text (Swift `CodeBlocks` parity).
/// Inline backticks stay literal (no `<code>` on the wire; Swift
/// renders them mono).
pub fn build_message_html(message: &str) -> String {
    if !wire_has_rich(message) {
        return format!("<p>{}</p>", html_escape(message));
    }
    enum Seg {
        Prose(String),
        Code(String),
    }
    let mut segs: Vec<Seg> = Vec::new();
    let mut prose = String::new();
    let mut code: Option<(Vec<String>, char, usize)> = None;
    let mut blocks = 0;
    for line in message.split('\n') {
        if let Some((mut lines, ch, len)) = code.take() {
            if wire_fence_closer(line, ch, len) {
                segs.push(Seg::Code(lines.join("\n")));
            } else {
                lines.push(line.to_string());
                code = Some((lines, ch, len));
            }
            continue;
        }
        if blocks < WIRE_FENCE_MAX_BLOCKS {
            if let Some((ch, len)) = wire_fence_opener(line) {
                if !prose.is_empty() {
                    segs.push(Seg::Prose(std::mem::take(&mut prose)));
                }
                code = Some((Vec::new(), ch, len));
                blocks += 1;
                continue;
            }
        }
        if !prose.is_empty() {
            prose.push('\n');
        }
        prose.push_str(line);
    }
    if let Some((lines, _, _)) = code.take() {
        segs.push(Seg::Code(lines.join("\n")));
    } else if !prose.is_empty() {
        segs.push(Seg::Prose(prose));
    }
    let mut out = String::new();
    for s in segs {
        match s {
            Seg::Prose(t) => {
                // Whitespace-only prose renders blank either way; skip it
                // so whole-message fences emit a lone <pre>.
                if !t.trim().is_empty() {
                    out.push_str(&render_prose_html(&t));
                }
            }
            Seg::Code(t) => out.push_str(&format!("<pre>{}</pre>", html_escape(&t))),
        }
    }
    out
}

/// POST body for sending one chat message (captured-body seam for
/// tests: the exact JSON `chat_post` receives, minus transport).
pub fn send_message_body(message: &str) -> serde_json::Value {
    serde_json::json!({
        "content": build_message_html(message),
        "messagetype": "RichText/Html",
        "contenttype": "text"
    })
}


/// Send a message using an existing client (shared helper).
pub async fn send_message_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message: &str,
) -> Result<()> {
    let base = client.chat_service_url();
    let url = format!("{}/v1/users/ME/conversations/{}/messages", base, chat_id);

    let body = send_message_body(message);

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

#[cfg(test)]
mod tests {
    use super::*;

    // Fenced blocks go out as <pre> (other clients keep
    // indents); prose keeps the legacy single-<p> shape bit-identical.

    #[test]
    fn wire_prose_unchanged_single_p() {
        assert_eq!(build_message_html("hi"), "<p>hi</p>");
        assert_eq!(
            build_message_html("a<b>&\"'\nline2  indented"),
            "<p>a&lt;b&gt;&amp;&quot;&#39;\nline2  indented</p>"
        );
        // Inline backticks and short runs are not fences.
        assert_eq!(build_message_html("use `x` here"), "<p>use `x` here</p>");
        assert_eq!(build_message_html("a\n``\nb"), "<p>a\n``\nb</p>");
        // Info string containing the fence char: not a fence (CommonMark).
        assert_eq!(
            build_message_html("``` `x` ```\nstill prose"),
            "<p>``` `x` ```\nstill prose</p>"
        );
    }

    #[test]
    fn wire_fenced_block_byte_exact_pre() {
        // Captured wire body: exact JSON `chat_post` receives.
        let body = send_message_body("```swift\nlet x  =  1\n\tindented\n```");
        assert_eq!(body["messagetype"], "RichText/Html");
        assert_eq!(body["contenttype"], "text");
        assert_eq!(body["content"], "<pre>let x  =  1\n\tindented</pre>");
        // Escaping still applies inside <pre>; fences + info consumed.
        let body = send_message_body("```\na<b>&\"'\n```");
        assert_eq!(body["content"], "<pre>a&lt;b&gt;&amp;&quot;&#39;</pre>");
        // ~~~ fences + indented fence lines work.
        let body = send_message_body("  ~~~py\nx = 1\n  ~~~");
        assert_eq!(body["content"], "<pre>x = 1</pre>");
    }

    #[test]
    fn wire_mixed_prose_and_code_segments() {
        assert_eq!(
            build_message_html("hi\n```\ncode  x\n```\nbye"),
            "<p>hi</p><pre>code  x</pre><p>bye</p>"
        );
        // Longer runs need equally long closers; inner short runs stay code.
        assert_eq!(
            build_message_html("````\n```\ninner\n```\n````"),
            "<pre>```\ninner\n```</pre>"
        );
    }

    #[test]
    fn wire_unclosed_fence_runs_to_end() {
        // Mid-typing states stay code.
        assert_eq!(
            build_message_html("note\n```\nline1\nline2"),
            "<p>note</p><pre>line1\nline2</pre>"
        );
    }

    #[test]
    fn wire_fence_cap_leaves_extras_prose() {
        let mut msg = String::new();
        for i in 0..(WIRE_FENCE_MAX_BLOCKS + 1) {
            msg.push_str(&format!("```\nc{}\n```\n", i));
        }
        let html = build_message_html(&msg);
        assert_eq!(html.matches("<pre>").count(), WIRE_FENCE_MAX_BLOCKS);
        // 51st block never parsed: its fences stay literal prose, nothing lost.
        assert!(html.contains("```\nc50\n```"), "{}", html);
    }
    // COMPOSER-A: composer markdown renders to Teams HTML (same shape
    // other clients already send; Swift renders the same markers).
    #[test]
    fn wire_inline_emphasis_to_teams_html() {
        assert_eq!(
            build_message_html("**Budget** approved"),
            "<p><strong>Budget</strong> approved</p>"
        );
        assert_eq!(
            build_message_html("*see notes* and __sign__ plus ~~old~~"),
            "<p><em>see notes</em> and <u>sign</u> plus <s>old</s></p>"
        );
        // Strict: space edges and newlines stay literal.
        assert_eq!(build_message_html("** not bold **"), "<p>** not bold **</p>");
        assert_eq!(build_message_html("**a\nb**"), "<p>**a\nb**</p>");
        assert_eq!(build_message_html("a * b"), "<p>a * b</p>");
        // Intra-word singles still pair (documented, harmless).
        assert_eq!(build_message_html("2*3*4"), "<p>2<em>3</em>4</p>");
        // Backtick spans protect markers (no <code> on the wire).
        assert_eq!(build_message_html("`**x**`"), "<p>`**x**`</p>");
        // Escaping still applies inside emphasis.
        assert_eq!(
            build_message_html("**a<b>**"),
            "<p><strong>a&lt;b&gt;</strong></p>"
        );
        // Priority: ** beats * (same as the composer toggle + render);
        // first-closer wins, so the trailing star stays literal.
        assert_eq!(build_message_html("***x***"), "<p><strong>*x</strong>*</p>");
        // Send seam carries the styled body.
        let body = send_message_body("**hi**");
        assert_eq!(body["content"], "<p><strong>hi</strong></p>");
        assert_eq!(body["messagetype"], "RichText/Html");
    }

    #[test]
    fn wire_block_markdown_to_teams_html() {
        assert_eq!(
            build_message_html("- a\n- b"),
            "<ul><li>a</li><li>b</li></ul>"
        );
        assert_eq!(
            build_message_html("1. a\n2. b"),
            "<ol><li>a</li><li>b</li></ol>"
        );
        assert_eq!(
            build_message_html("> quoted\n> still"),
            "<blockquote>quoted<br/>still</blockquote>"
        );
        assert_eq!(
            build_message_html("| a | b |\n|---|---|\n| 1 | **2** |"),
            "<table><tr><th>a</th><th>b</th></tr><tr><td>1</td><td><strong>2</strong></td></tr></table>"
        );
        // Non-tables and mid-line numbers stay prose.
        assert_eq!(build_message_html("a|b"), "<p>a|b</p>");
        assert_eq!(build_message_html("Shop 1. tamales"), "<p>Shop 1. tamales</p>");
    }

    #[test]
    fn wire_rich_mixed_with_code() {
        assert_eq!(
            build_message_html("**hi**\n```\ncode  x\n```\n- bye"),
            "<p><strong>hi</strong></p><pre>code  x</pre><ul><li>bye</li></ul>"
        );
    }

}
