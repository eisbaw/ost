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
    build_message_html_opts(message, &SendOpts::none())
}

/// COMPOSER-B: [`build_message_html`] with broadcast-mention
/// substitution. Tokens become private-use sentinels before rendering
/// (escaping and inline markers never see display text), restored to
/// `<at>` tags after. `message` decides the legacy/rich path (tokens
/// carry no markers, so either input decides identically).
pub fn build_message_html_opts(message: &str, opts: &SendOpts) -> String {
    let (tagged, _) = apply_mentions(message, &opts.mentions);
    let html = render_tagged(message, &tagged);
    restore_mentions(&html, &opts.mentions)
}

/// Sentinel marking mention `idx` through escaping and rendering
/// (private-use: `html_escape` never touches them, inline markers never
/// match them, and real drafts never contain them).
fn mention_sentinel(idx: usize) -> String {
    format!("\u{E000}{idx}\u{E001}")
}

/// Substitute whole-token broadcast mentions with sentinels, returning
/// the tagged text and one matched flag per mention. Longest token
/// first. A token matches when the char before is start/whitespace/
/// punctuation and the char after is end/whitespace/punctuation
/// (composer-trigger bounds, so `x@channel` and `@channelx` never tag).
/// Case-insensitive. Fenced code blocks and inline-code spans are never
/// substituted (Swift `CodeBlocks` parity: code is never mined).
pub fn apply_mentions(markdown: &str, mentions: &[WireMention]) -> (String, Vec<bool>) {
    let mut matched = vec![false; mentions.len()];
    if mentions.is_empty() {
        return (markdown.to_string(), matched);
    }
    let mut order: Vec<usize> = (0..mentions.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(mentions[i].token.len()));
    let tick_spans = wire_tick_spans(markdown);
    // Fenced-block line ranges (same opener/closer rules + block cap as
    // the renderer, so exclusion agrees with what renders as code).
    let mut fenced: Vec<(usize, usize)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut blocks = 0;
    let mut off = 0;
    for line in markdown.split_inclusive('\n') {
        let len = line.len();
        let body = line.strip_suffix('\n').unwrap_or(line);
        if let Some((ch, flen)) = fence {
            fenced.push((off, off + len));
            if wire_fence_closer(body, ch, flen) {
                fence = None;
            }
        } else if blocks < WIRE_FENCE_MAX_BLOCKS {
            if let Some((ch, flen)) = wire_fence_opener(body) {
                fenced.push((off, off + len));
                fence = Some((ch, flen));
                blocks += 1;
            }
        }
        off += len;
    }
    // Byte scan: `@` is ASCII, so every hit starts a char boundary and
    // `pos + token.len()` (token ASCII-only below) ends one.
    let bytes = markdown.as_bytes();
    let mut hits: Vec<(usize, usize)> = Vec::new(); // (pos, mention idx)
    for &i in &order {
        let tok = mentions[i].token.to_ascii_lowercase();
        if !tok.starts_with('@') || !tok.is_ascii() || tok.len() < 2 {
            continue;
        }
        let tb = tok.as_bytes();
        let mut p = 0;
        while p + tb.len() <= bytes.len() {
            if bytes[p] == b'@' && bytes[p..p + tb.len()].eq_ignore_ascii_case(tb) {
                let end = p + tb.len();
                let prev_ok = p == 0 || {
                    let c = markdown[..p].chars().next_back().unwrap();
                    !c.is_alphanumeric() && c != '_'
                };
                let next_ok = end == bytes.len() || {
                    let c = markdown[end..].chars().next().unwrap();
                    !c.is_alphanumeric() && c != '_'
                };
                let in_code = tick_spans.iter().any(|&(a, b)| p < b && end > a)
                    || fenced.iter().any(|&(a, b)| p < b && end > a);
                if prev_ok && next_ok && !in_code {
                    hits.push((p, i));
                    matched[i] = true;
                    p = end;
                    continue;
                }
            }
            p += 1;
        }
    }
    if hits.is_empty() {
        return (markdown.to_string(), matched);
    }
    hits.sort();
    let mut out = String::with_capacity(markdown.len());
    let mut pos = 0;
    for (at, i) in hits {
        // Tokens are pairwise distinct, so hits never overlap; a stale
        // `pos` (unreachable) keeps the text intact rather than corrupt.
        if at < pos {
            continue;
        }
        out.push_str(&markdown[pos..at]);
        out.push_str(&mention_sentinel(i));
        pos = at + mentions[i].token.len();
    }
    out.push_str(&markdown[pos..]);
    (out, matched)
}

/// Restore sentinels to official mention spans (`<span itemscope
/// itemtype="…/Mention" itemid="N">…</span>` — ECHO-FIX M2: spans
/// survive the write path, `<at>` tags are scrubbed). Display text is
/// escaped (caller-supplied); the itemid is numeric, shared with the
/// entity via [`mention_itemid`]. Indices without sentinels are no-ops.
pub fn restore_mentions(html: &str, mentions: &[WireMention]) -> String {
    let mut out = html.to_string();
    for (i, m) in mentions.iter().enumerate() {
        out = out.replace(
            &mention_sentinel(i),
            &format!(
                "<span itemscope itemtype=\"{}\" itemid=\"{}\">{}</span>",
                MENTION_TYPE_URI,
                mention_itemid(m, i),
                html_escape(&m.display)
            ),
        );
    }
    out
}

fn render_tagged(message: &str, tagged: &str) -> String {
    if !wire_has_rich(message) {
        return format!("<p>{}</p>", html_escape(tagged));
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

// ---------------------------------------------------------------------------
// COMPOSER-B (C1b): importance/urgent, announcement subject, broadcast
// mentions. All additive: `SendOpts::none()` renders the legacy body
// bit-identical; each set field adds exactly one body key. Bodies never
// fail on opts (unknown importance parses to normal; bad mentions JSON
// parses to none).
// ---------------------------------------------------------------------------

/// Outbound importance (Graph `chatMessage.importance` parity: `normal`
/// is the chatsvc default and is never serialized).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WireImportance {
    #[default]
    Normal,
    Important,
    Urgent,
}

impl WireImportance {
    /// `"high"` / `"urgent"`, or `None` for normal (never on the wire).
    pub fn wire_value(self) -> Option<&'static str> {
        match self {
            WireImportance::Normal => None,
            WireImportance::Important => Some("high"),
            WireImportance::Urgent => Some("urgent"),
        }
    }

    /// Parse the Swift `MessageImportance` wire string (`""`/`"normal"`
    /// → normal; case-insensitive). Unknown stays normal: importance is
    /// cosmetic, never a send failure.
    pub fn parse(s: &str) -> WireImportance {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" | "important" => WireImportance::Important,
            "urgent" => WireImportance::Urgent,
            _ => WireImportance::Normal,
        }
    }
}

/// One mention entity for the send path (broadcast `@channel` / `@team` /
/// `@everyone`, or a person mention when the caller knows the member
/// MRI): the literal token as typed, the wire identity (numeric itemid;
/// unparseable ids fall back to the positional index), the display text,
/// the `mentionType` (`channel` / `team` / `everyone` / `person`), and
/// the entity `mri` (user `8:…` / tag id / channel thread id; empty when
/// the caller has none — the key is then omitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMention {
    pub token: String,
    pub id: String,
    pub display: String,
    pub kind: String,
    pub mri: String,
}

/// Max broadcast mentions honored per send (hostile-input cap, mirrors
/// [`WIRE_FENCE_MAX_BLOCKS`); extras are dropped, never fail.
pub const WIRE_MENTION_MAX: usize = 20;

/// Parse the Swift `mentions_json` (`[{token,id,display,kind,mri?}]`).
/// Tolerant: bad JSON or bad rows yield no mentions, never a failure;
/// a missing/blank `mri` parses to empty (the entity key is omitted).
pub fn parse_wire_mentions(json: &str) -> Vec<WireMention> {
    let t = json.trim();
    if t.is_empty() {
        return Vec::new();
    }
    let v: serde_json::Value = serde_json::from_str(t).unwrap_or(serde_json::Value::Null);
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let o = e.as_object()?;
            let token = o.get("token")?.as_str()?.trim();
            let id = o.get("id")?.as_str()?.trim();
            let display = o.get("display")?.as_str()?;
            let kind = o.get("kind")?.as_str()?.trim();
            let mri = o
                .get("mri")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if token.is_empty() || !token.starts_with('@') || id.is_empty() || kind.is_empty() {
                return None;
            }
            Some(WireMention {
                token: token.to_string(),
                id: id.to_string(),
                display: display.to_string(),
                kind: kind.to_string(),
                mri: mri.to_string(),
            })
        })
        .take(WIRE_MENTION_MAX)
        .collect()
}

/// Skype mention entity type URI (the official chatsvc write shape).
pub const MENTION_TYPE_URI: &str = "http://schema.skype.com/Mention";

/// Effective numeric itemid for one mention entity and its content tag
/// (shared so the pair always agrees). Numeric ids pass through;
/// anything else falls back to the positional index — hostile input
/// never breaks the pair.
fn mention_itemid(m: &WireMention, index: usize) -> u64 {
    m.id.trim().parse::<u64>().unwrap_or(index as u64)
}

/// Mention entities in the official write shape: a JSON STRING of
/// `[{@type, itemid:number, mri?, mentionType, displayName}]` (ECHO-FIX
/// R1 live-proven: raw arrays are dropped at write, strings persist
/// verbatim; `<at>` content tags are scrubbed, spans survive).
fn mention_entities(mentions: &[WireMention]) -> String {
    let ents: Vec<serde_json::Value> = mentions
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut e = serde_json::Map::new();
            e.insert(
                "@type".to_string(),
                serde_json::Value::String(MENTION_TYPE_URI.to_string()),
            );
            e.insert(
                "itemid".to_string(),
                serde_json::Value::from(mention_itemid(m, i)),
            );
            if !m.mri.trim().is_empty() {
                e.insert(
                    "mri".to_string(),
                    serde_json::Value::String(m.mri.trim().to_string()),
                );
            }
            e.insert(
                "mentionType".to_string(),
                serde_json::Value::String(m.kind.clone()),
            );
            e.insert(
                "displayName".to_string(),
                serde_json::Value::String(m.display.clone()),
            );
            serde_json::Value::Object(e)
        })
        .collect();
    serde_json::Value::Array(ents).to_string()
}

/// Additive send options (COMPOSER-B). `none()` is the legacy request.
#[derive(Debug, Clone, Default)]
pub struct SendOpts {
    pub importance: WireImportance,
    pub subject: Option<String>,
    pub mentions: Vec<WireMention>,
}

impl SendOpts {
    pub fn none() -> Self {
        Self::default()
    }

    /// Clone keeping only the mentions whose tokens substitute in `text
    /// (matched flags from [`apply_mentions`]), so entities never name a
    /// token the content lacks.
    pub fn matched_in(&self, text: &str) -> SendOpts {
        let hit = apply_mentions(text, &self.mentions).1;
        let mut out = self.clone();
        out.mentions = self
            .mentions
            .iter()
            .zip(hit)
            .filter(|(_, m)| *m)
            .map(|(w, _)| w.clone())
            .collect();
        out
    }

    /// Serialize the set fields onto a POST body (callers pass only
    /// matched mentions — see [`send_message_body_opts`] — so a stale
    /// token list never notifies blind).
    fn apply_to_body(&self, body: &mut serde_json::Value) {
        // ECHO-FIX (SKYPE-WRITE-SHAPE): the chatsvc write path reads
        // importance/subject from UNDER `properties` — top-level cells
        // are dropped at write. Any set opt emits the official
        // always-present block (F1/S1/M2 live-proven shapes);
        // `none()` stays the legacy body bit-identical (no key at all).
        let subject = self.subject.as_ref().map(|s| s.trim()).unwrap_or("");
        if self.importance.wire_value().is_none()
            && subject.is_empty()
            && self.mentions.is_empty()
        {
            return;
        }
        let mut props = serde_json::Map::new();
        props.insert(
            "importance".to_string(),
            serde_json::Value::String(self.importance.wire_value().unwrap_or("").to_string()),
        );
        props.insert(
            "subject".to_string(),
            serde_json::Value::String(subject.to_string()),
        );
        if !self.mentions.is_empty() {
            props.insert(
                "mentions".to_string(),
                serde_json::Value::String(mention_entities(&self.mentions)),
            );
        }
        body["properties"] = serde_json::Value::Object(props);
    }
}

/// POST body for sending one chat message (captured-body seam for
/// tests: the exact JSON `chat_post` receives, minus transport).
pub fn send_message_body(message: &str) -> serde_json::Value {
    send_message_body_opts(message, &SendOpts::none())
}

/// [`send_message_body`] with COMPOSER-B options. Entities cover only
/// the mentions whose tokens actually substituted (matched flags from
/// [`apply_mentions`]).
pub fn send_message_body_opts(message: &str, opts: &SendOpts) -> serde_json::Value {
    let filtered = opts.matched_in(message);
    let (tagged, _) = apply_mentions(message, &filtered.mentions);
    let html = restore_mentions(&render_tagged(message, &tagged), &filtered.mentions);
    let mut body = serde_json::json!({
        "content": html,
        "messagetype": "RichText/Html",
        "contenttype": "text"
    });
    filtered.apply_to_body(&mut body);
    body
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


    // COMPOSER-B: importance parses to Graph parity values, normal (and
    // unknown) serializes to nothing.
    #[test]
    fn composer_b_importance_parse_and_wire() {
        assert_eq!(WireImportance::parse(""), WireImportance::Normal);
        assert_eq!(WireImportance::parse("normal"), WireImportance::Normal);
        assert_eq!(WireImportance::parse("bogus"), WireImportance::Normal);
        assert_eq!(WireImportance::parse("HIGH"), WireImportance::Important);
        assert_eq!(WireImportance::parse("important"), WireImportance::Important);
        assert_eq!(WireImportance::parse("urgent"), WireImportance::Urgent);
        assert_eq!(WireImportance::Normal.wire_value(), None);
        assert_eq!(WireImportance::Important.wire_value(), Some("high"));
        assert_eq!(WireImportance::Urgent.wire_value(), Some("urgent"));
    }

    // COMPOSER-B: mentions JSON is tolerant (bad JSON/rows → none) and
    // capped (hostile-input parity).
    #[test]
    fn composer_b_parse_wire_mentions_tolerant() {
        assert!(parse_wire_mentions("").is_empty());
        assert!(parse_wire_mentions("not json").is_empty());
        assert!(parse_wire_mentions("{}").is_empty());
        assert!(parse_wire_mentions(r#"[{"token":"@channel"}]"#).is_empty());
        assert!(parse_wire_mentions(r#"[{"token":"channel","id":"0","display":"c","kind":"channel"}]"#)
            .is_empty());
        let one = parse_wire_mentions(
            r#"[{"token":"@channel","id":"0","display":"channel","kind":"channel"}]"#,
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].kind, "channel");
        let many: String = (0..30)
            .map(|_| r#"{"token":"@channel","id":"0","display":"c","kind":"channel"}"#)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(parse_wire_mentions(&format!("[{many}]")).len(), WIRE_MENTION_MAX);
    }

    fn composer_b_mention() -> WireMention {
        WireMention {
            token: "@channel".to_string(),
            id: "0".to_string(),
            display: "channel".to_string(),
            kind: "channel".to_string(),
            mri: "19:TESTCHAN@thread.tacv2".to_string(),
        }
    }

    fn composer_b_opts() -> SendOpts {
        SendOpts {
            importance: WireImportance::Normal,
            subject: None,
            mentions: vec![composer_b_mention()],
        }
    }

    // COMPOSER-B: token substitution is whole-token, case-insensitive,
    // and skips code (fences + inline spans).
    #[test]
    fn composer_b_apply_mentions_bounds() {
        let m = vec![composer_b_mention()];
        let (t, hit) = apply_mentions("hey @channel look", &m);
        assert_eq!(hit, vec![true]);
        assert_eq!(
            restore_mentions(&t, &m),
            "hey <span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">channel</span> look"
        );
        // Case-insensitive, all occurrences (two substitutions).
        let (t, hit) = apply_mentions("@Channel and @CHANNEL", &m);
        assert_eq!(hit, vec![true]);
        assert_eq!(t.matches(&mention_sentinel(0)).count(), 2);
        assert_eq!(
            restore_mentions(&t, &m),
            "<span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">channel</span> and <span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">channel</span>"
        );
        // Glued tokens never tag (`!` splits, but the `a` prefix blocks).
        for fixed in ["x@channel", "@channelx", "@channel_x", "a@channel!b@c", "mail@channel.com"] {
            let (_, hit) = apply_mentions(fixed, &m);
            assert_eq!(hit, vec![false], "{fixed:?}");
        }
        // Punctuation bounds do tag.
        let (_, hit) = apply_mentions("(@channel!)", &m);
        assert_eq!(hit, vec![true]);
        // Fenced blocks and inline code never tag.
        let (_, hit) = apply_mentions("```\n@channel\n```", &m);
        assert_eq!(hit, vec![false]);
        let (_, hit) = apply_mentions("`@channel` stays", &m);
        assert_eq!(hit, vec![false]);
        // Empty mention list is the identity.
        let (t, hit) = apply_mentions("hey @channel", &[]);
        assert_eq!(t.as_str(), "hey @channel");
        assert!(hit.is_empty());
    }

    // COMPOSER-B: bodies carry importance/subject/entities only when set;
    // `none()` is the legacy body bit-identical.
    // ECHO-FIX: set opts emit the official always-present `properties`
    // block (top-level cells are dropped at write — SKYPE-WRITE-SHAPE).
    #[test]
    fn composer_b_body_opts_keys() {
        let legacy = send_message_body("hey @channel");
        let plain = send_message_body_opts("hey @channel", &SendOpts::none());
        assert_eq!(legacy, plain);
        assert!(legacy.get("importance").is_none());
        assert!(legacy.get("subject").is_none());
        assert!(legacy.get("properties").is_none());

        // F1 shape: importance under properties, subject always present.
        let important = send_message_body_opts(
            "hey",
            &SendOpts {
                importance: WireImportance::Important,
                ..SendOpts::none()
            },
        );
        assert!(important.get("importance").is_none());
        assert_eq!(important["properties"]["importance"], "high");
        assert_eq!(important["properties"]["subject"], "");

        // S1 shape: filled subject, empty importance.
        let urgent_subject = send_message_body_opts(
            "hey",
            &SendOpts {
                importance: WireImportance::Urgent,
                subject: Some("Weekly sync".to_string()),
                ..SendOpts::none()
            },
        );
        assert!(urgent_subject.get("importance").is_none());
        assert!(urgent_subject.get("subject").is_none());
        assert_eq!(urgent_subject["properties"]["importance"], "urgent");
        assert_eq!(urgent_subject["properties"]["subject"], "Weekly sync");

        // Blank subjects serialize to nothing (legacy body, no block).
        let blank = send_message_body_opts(
            "hey",
            &SendOpts {
                subject: Some("   ".to_string()),
                ..SendOpts::none()
            },
        );
        assert!(blank.get("subject").is_none());
        assert!(blank.get("properties").is_none());

        // Entities cover only matched tokens (stale list → no blind notify).
        let full = send_message_body_opts("hey @channel", &composer_b_opts());
        assert_eq!(
            full["content"],
            "<p>hey <span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">channel</span></p>"
        );
        let ents: serde_json::Value =
            serde_json::from_str(full["properties"]["mentions"].as_str().unwrap()).unwrap();
        assert_eq!(ents[0]["@type"], "http://schema.skype.com/Mention");
        assert_eq!(ents[0]["itemid"], 0);
        assert_eq!(ents[0]["mri"], "19:TESTCHAN@thread.tacv2");
        assert_eq!(ents[0]["mentionType"], "channel");
        assert_eq!(ents[0]["displayName"], "channel");
        let stale = send_message_body_opts("hey all", &composer_b_opts());
        assert!(stale.get("properties").is_none());

        // Display text escapes (caller-supplied); hostile ids fall back
        // to the positional index, shared by tag and entity.
        let evil = send_message_body_opts(
            "hi @channel",
            &SendOpts {
                mentions: vec![WireMention {
                    token: "@channel".to_string(),
                    id: "0\"><b>".to_string(),
                    display: "a<b>&".to_string(),
                    kind: "channel".to_string(),
                    mri: "".to_string(),
                }],
                ..SendOpts::none()
            },
        );
        assert_eq!(
            evil["content"],
            "<p>hi <span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">a&lt;b&gt;&amp;</span></p>"
        );
        let evil_ents: serde_json::Value =
            serde_json::from_str(evil["properties"]["mentions"].as_str().unwrap()).unwrap();
        assert_eq!(evil_ents[0]["itemid"], 0);
        assert!(evil_ents[0].get("mri").is_none());
        // No sentinel leaks into any body.
        for b in [&full, &stale, &evil, &legacy] {
            assert!(
                !b["content"].as_str().unwrap_or("").contains('\u{E000}'),
                "{:?}",
                b["content"]
            );
        }
    }

    // ECHO-FIX: M2 person-mention body — string entity (numeric itemid,
    // @type, user mri) + span content, verbatim live-proven shapes.
    #[test]
    fn echofix_person_mention_body() {
        let body = send_message_body_opts(
            "hey @Jordan Fox",
            &SendOpts {
                mentions: vec![WireMention {
                    token: "@Jordan Fox".to_string(),
                    id: "0".to_string(),
                    display: "Jordan Fox".to_string(),
                    kind: "person".to_string(),
                    mri: "8:orgid:11111111-2222-3333-4444-555555555555".to_string(),
                }],
                ..SendOpts::none()
            },
        );
        assert_eq!(
            body["content"],
            "<p>hey <span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">Jordan Fox</span></p>"
        );
        assert_eq!(body["properties"]["importance"], "");
        assert_eq!(body["properties"]["subject"], "");
        let ents: serde_json::Value =
            serde_json::from_str(body["properties"]["mentions"].as_str().unwrap()).unwrap();
        assert_eq!(ents.as_array().unwrap().len(), 1);
        assert_eq!(ents[0]["itemid"], 0);
        assert_eq!(ents[0]["mentionType"], "person");
        assert_eq!(
            ents[0]["mri"],
            "8:orgid:11111111-2222-3333-4444-555555555555"
        );
    }

    // ECHO-FIX: `mentions_json` carries the entity mri (tolerant: absent
    // or blank parses to empty, never rejects the row).
    #[test]
    fn echofix_parse_wire_mri() {
        let one = parse_wire_mentions(
            r#"[{"token":"@channel","id":"0","display":"channel","kind":"channel","mri":"19:X@thread.tacv2"}]"#,
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].mri, "19:X@thread.tacv2");
        let legacy = parse_wire_mentions(
            r#"[{"token":"@channel","id":"0","display":"channel","kind":"channel"}]"#,
        );
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].mri, "");
        let blank = parse_wire_mentions(
            r#"[{"token":"@channel","id":"0","display":"channel","kind":"channel","mri":"  "}]"#,
        );
        assert_eq!(blank.len(), 1);
        assert_eq!(blank[0].mri, "");
    }

}
