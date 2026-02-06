//! Messages pane: displays channel messages with reactions, replies, and attachments.

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Timelike, Weekday};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::api;

/// Default header shown when no channel is selected.
const DEFAULT_HEADER: &str = "Select a channel or chat";

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A reaction on a message (e.g., thumbs-up x3).
#[derive(Clone)]
pub struct Reaction {
    /// ASCII-safe label: "+1", "<3", "eyes", etc.
    pub label: String,
    /// How many people reacted with this.
    pub count: u32,
}

/// A file attachment on a message.
#[derive(Clone)]
pub struct Attachment {
    /// Display filename.
    pub name: String,
}

/// A single chat message.
#[derive(Clone)]
pub struct Message {
    /// Sender display name.
    pub sender: String,
    /// Timestamp string (e.g., "9:15 AM today").
    pub timestamp: String,
    /// Message body lines.
    pub content: String,
    /// Reactions below the message.
    pub reactions: Vec<Reaction>,
    /// Number of thread replies (0 = no thread).
    pub reply_count: u32,
    /// Inline thread replies (shown when expanded).
    pub replies: Vec<Message>,
    /// File attachments.
    pub attachments: Vec<Attachment>,
}

/// State for the messages pane.
pub struct MessagesState {
    /// Channel header text (e.g., "Engineering Team > #general").
    pub channel_header: String,
    /// All messages in the channel.
    pub messages: Vec<Message>,
    /// Vertical scroll offset (in rendered lines, 0 = top).
    pub scroll_offset: usize,
    /// Index of the currently selected message (for highlighting).
    pub selected: usize,
    /// Which message indices have their thread expanded.
    pub expanded_threads: Vec<bool>,
    /// Whether messages are being loaded.
    pub loading: bool,
}

impl Default for MessagesState {
    fn default() -> Self {
        Self {
            channel_header: DEFAULT_HEADER.to_string(),
            messages: Vec::new(),
            expanded_threads: Vec::new(),
            scroll_offset: 0,
            selected: 0,
            loading: false,
        }
    }
}

impl MessagesState {
    /// Update messages from API response.
    pub fn update_messages(&mut self, header: &str, api_messages: Vec<api::MessageInfo>) {
        self.channel_header = header.to_string();
        self.messages = api_messages
            .into_iter()
            .map(|m| Message {
                sender: m.sender,
                timestamp: m.timestamp,
                content: m.content,
                reactions: Vec::new(),
                reply_count: 0,
                replies: Vec::new(),
                attachments: Vec::new(),
            })
            .collect();
        let count = self.messages.len();
        self.expanded_threads = vec![true; count];
        self.scroll_offset = 0;
        // Select the last (newest) message so the view starts at the bottom.
        self.selected = count.saturating_sub(1);
        self.loading = false;
    }

    /// Move selection up by one message.
    pub fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down by one message.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.messages.len() {
            self.selected += 1;
        }
    }

    /// Toggle thread expansion for the selected message.
    pub fn toggle_thread(&mut self) {
        if self.selected < self.expanded_threads.len()
            && !self.messages[self.selected].replies.is_empty()
        {
            self.expanded_threads[self.selected] = !self.expanded_threads[self.selected];
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the messages pane into the given area.
pub fn render(area: Rect, buf: &mut Buffer, state: &MessagesState, focused: bool, user_name: &str) {
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let border_type = if focused {
        BorderType::Double
    } else {
        BorderType::Plain
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style);

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Reserve the first line for the channel header.
    let header_area = Rect::new(inner.x, inner.y, inner.width, 1);
    render_channel_header(header_area, buf, &state.channel_header);

    // Remaining space for messages.
    let messages_area = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );

    if messages_area.height == 0 {
        return;
    }

    // Show loading indicator.
    if state.loading {
        let loading_area = Rect::new(messages_area.x, messages_area.y, messages_area.width, 1);
        let line = Line::from(Span::styled(
            " Loading messages...",
            Style::default().fg(Color::DarkGray),
        ));
        Paragraph::new(line).render(loading_area, buf);
        return;
    }

    // Show empty state.
    if state.messages.is_empty() {
        let empty_area = Rect::new(messages_area.x, messages_area.y, messages_area.width, 1);
        let text = if state.channel_header == DEFAULT_HEADER {
            " Select a channel or chat to view messages"
        } else {
            " No messages yet"
        };
        let line = Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)));
        Paragraph::new(line).render(empty_area, buf);
        return;
    }

    // Pre-render all messages into a line buffer (single pass produces lines + ranges).
    let (all_lines, msg_line_ranges) = build_message_lines(state, messages_area.width as usize, user_name);
    let total_lines = all_lines.len();
    let visible_height = messages_area.height as usize;

    // Auto-scroll to keep selected message visible.
    let scroll = compute_auto_scroll(
        state.scroll_offset,
        state.selected,
        &msg_line_ranges,
        visible_height,
        total_lines,
    );

    // Render visible lines.
    for (row, line_idx) in (scroll..total_lines).take(visible_height).enumerate() {
        let y = messages_area.y + row as u16;
        if y >= messages_area.y + messages_area.height {
            break;
        }
        let line_area = Rect::new(messages_area.x, y, messages_area.width, 1);
        Paragraph::new(all_lines[line_idx].clone()).render(line_area, buf);
    }

    // Render scroll indicators.
    if total_lines > visible_height {
        let indicator_x = messages_area.x + messages_area.width.saturating_sub(1);
        if scroll > 0 {
            // Up arrow at top-right.
            let cell = &mut buf[(indicator_x, messages_area.y)];
            cell.set_char('^');
            cell.set_style(Style::default().fg(Color::DarkGray));
        }
        if scroll + visible_height < total_lines {
            // Down arrow at bottom-right.
            let bottom_y = messages_area.y + messages_area.height.saturating_sub(1);
            let cell = &mut buf[(indicator_x, bottom_y)];
            cell.set_char('v');
            cell.set_style(Style::default().fg(Color::DarkGray));
        }
    }
}

/// Render the channel header line.
fn render_channel_header(area: Rect, buf: &mut Buffer, header: &str) {
    let line = Line::from(vec![Span::styled(
        format!(" {} ", header),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )]);
    Paragraph::new(line)
        .style(Style::default().bg(Color::DarkGray))
        .render(area, buf);
}

/// Build the flat line buffer and per-message line ranges in a single pass.
fn build_message_lines(
    state: &MessagesState,
    width: usize,
    user_name: &str,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let today = Local::now().naive_local().date();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    for (msg_idx, msg) in state.messages.iter().enumerate() {
        let start = lines.len();
        let is_selected = msg_idx == state.selected;
        let thread_expanded = state
            .expanded_threads
            .get(msg_idx)
            .copied()
            .unwrap_or(false);

        // Render main message card.
        render_message_card(&mut lines, msg, width, is_selected, false, 0, msg_idx, today, user_name);

        // Render thread replies if expanded.
        if thread_expanded && !msg.replies.is_empty() {
            for reply in &msg.replies {
                render_message_card(&mut lines, reply, width, false, true, 4, msg_idx, today, user_name);
            }
        } else if msg.reply_count > 0 && !thread_expanded {
            // Show collapsed thread indicator.
            let indent = "     ";
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(
                    format!(">> {} replies (Enter to expand)", msg.reply_count),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ),
            ]));
        }

        // Blank line between top-level messages.
        lines.push(Line::from(""));

        ranges.push((start, lines.len()));
    }

    (lines, ranges)
}

/// Render a single message card (either top-level or reply) into the line buffer.
///
/// Uses colored backgrounds instead of ASCII borders. Even/odd `msg_idx`
/// alternates between two subtle background shades for visual separation.
fn render_message_card(
    lines: &mut Vec<Line<'static>>,
    msg: &Message,
    width: usize,
    is_selected: bool,
    is_reply: bool,
    indent: usize,
    msg_idx: usize,
    today: NaiveDate,
    user_name: &str,
) {
    let is_own = msg.sender == user_name;
    let indent_str: String = " ".repeat(indent);
    let reply_prefix = if is_reply { " -> " } else { "" };

    // Own (non-reply) messages use 75% width and right-align.
    let (effective_width, left_margin) = if is_own && !is_reply {
        let ew = (width * 3 / 4).max(40).min(width);
        (ew, width.saturating_sub(ew))
    } else {
        (width, 0)
    };

    // Usable content width after indent, reply prefix, and small margins.
    let prefix_len = indent + reply_prefix.len();
    let content_width = effective_width.saturating_sub(prefix_len).saturating_sub(2);

    if content_width < 10 {
        return;
    }

    // Background color: own messages get a subtle blue tint.
    let bg = if is_selected {
        if is_own {
            Color::Rgb(45, 55, 70)
        } else {
            Color::Rgb(55, 55, 70)
        }
    } else if is_reply {
        Color::Rgb(30, 30, 38)
    } else if is_own {
        Color::Rgb(30, 38, 50)
    } else if msg_idx % 2 == 0 {
        Color::Rgb(35, 35, 45)
    } else {
        Color::Rgb(42, 42, 52)
    };

    let selection_indicator = if is_selected && !is_reply { "> " } else { "  " };

    let sender_color = username_to_color(&msg.sender);
    let sender_style = Style::default()
        .fg(sender_color)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let timestamp_style = Style::default().fg(Color::DarkGray).bg(bg);
    let text_style = Style::default().fg(Color::White).bg(bg);
    let bg_style = Style::default().bg(bg);

    let formatted_ts = format_timestamp(&msg.timestamp, today);

    // Helper: build a line padded to effective_width, with optional left margin
    // for right-aligned own messages.
    let make_bg_line = |spans: Vec<Span<'static>>, used_chars: usize| -> Line<'static> {
        let pad = effective_width.saturating_sub(used_chars);
        let mut all_spans = Vec::new();
        if left_margin > 0 {
            all_spans.push(Span::raw(" ".repeat(left_margin)));
        }
        all_spans.extend(spans);
        all_spans.push(Span::styled(" ".repeat(pad), bg_style));
        Line::from(all_spans)
    };

    // Sender + timestamp line.
    let prefix = format!("{}{}{}", indent_str, reply_prefix, selection_indicator);
    let ts_gap = content_width
        .saturating_sub(msg.sender.len())
        .saturating_sub(formatted_ts.len());
    let used = prefix.len() + msg.sender.len() + ts_gap + formatted_ts.len();
    lines.push(make_bg_line(
        vec![
            Span::styled(prefix.clone(), bg_style),
            Span::styled(msg.sender.clone(), sender_style),
            Span::styled(" ".repeat(ts_gap), bg_style),
            Span::styled(formatted_ts, timestamp_style),
        ],
        used,
    ));

    // Content lines (word-wrapped).
    let wrap_width = content_width;
    let content_lines = wrap_text(&msg.content, wrap_width);
    let content_prefix = format!(
        "{}{}",
        indent_str,
        if is_reply {
            "        " // align with reply content after " -> > "
        } else {
            "    " // align with sender name after selection indicator "  "
        }
    );
    for cl in &content_lines {
        let used = content_prefix.len() + cl.len();
        lines.push(make_bg_line(
            vec![
                Span::styled(content_prefix.clone(), bg_style),
                Span::styled(cl.clone(), text_style),
            ],
            used,
        ));
    }

    // Attachments.
    for att in &msg.attachments {
        let att_text = format!("[file] {}", att.name);
        let used = content_prefix.len() + att_text.len();
        lines.push(make_bg_line(
            vec![
                Span::styled(content_prefix.clone(), bg_style),
                Span::styled(
                    att_text,
                    Style::default()
                        .fg(Color::Cyan)
                        .bg(bg)
                        .add_modifier(Modifier::DIM),
                ),
            ],
            used,
        ));
    }

    // Reactions and reply count.
    if !msg.reactions.is_empty() || msg.reply_count > 0 {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(content_prefix.clone(), bg_style));
        let mut used = content_prefix.len();

        for (i, r) in msg.reactions.iter().enumerate() {
            let r_text = format!("{} {}", r.label, r.count);
            used += r_text.len();
            spans.push(Span::styled(
                r_text,
                Style::default().fg(Color::Yellow).bg(bg),
            ));
            if i + 1 < msg.reactions.len() || msg.reply_count > 0 {
                spans.push(Span::styled("   ", bg_style));
                used += 3;
            }
        }

        if msg.reply_count > 0 {
            let reply_text = format!(">> {} replies", msg.reply_count);
            used += reply_text.len();
            spans.push(Span::styled(
                reply_text,
                Style::default().fg(Color::Cyan).bg(bg),
            ));
        }

        lines.push(make_bg_line(spans, used));
    }
}

/// Format an ISO 8601 timestamp string into a human-readable form.
///
/// - Today: "14:34"
/// - Within the last 7 days: "Mon 14:34"
/// - Older: "Jan 29"
///
/// `today` should be precomputed once per render pass to avoid redundant
/// syscalls and midnight-boundary inconsistencies.
///
/// Falls back to returning the original string if parsing fails.
fn format_timestamp(raw: &str, today: NaiveDate) -> String {
    let trimmed = raw.trim();
    let is_utc = trimmed.ends_with('Z');
    let stripped = trimmed.trim_end_matches('Z');

    let parsed = NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M:%S"));
    let naive = match parsed {
        Ok(dt) => dt,
        Err(_) => return raw.to_string(),
    };

    // Convert UTC timestamps to local time.
    let local_dt = if is_utc {
        naive.and_utc().with_timezone(&Local).naive_local()
    } else {
        naive
    };

    let ts_date = local_dt.date();

    if ts_date == today {
        format!("{:02}:{:02}", local_dt.hour(), local_dt.minute())
    } else {
        let days_ago = (today - ts_date).num_days();
        if days_ago > 0 && days_ago < 7 {
            let weekday = match ts_date.weekday() {
                Weekday::Mon => "Mon",
                Weekday::Tue => "Tue",
                Weekday::Wed => "Wed",
                Weekday::Thu => "Thu",
                Weekday::Fri => "Fri",
                Weekday::Sat => "Sat",
                Weekday::Sun => "Sun",
            };
            format!("{} {:02}:{:02}", weekday, local_dt.hour(), local_dt.minute())
        } else {
            let month = match ts_date.month() {
                1 => "Jan",
                2 => "Feb",
                3 => "Mar",
                4 => "Apr",
                5 => "May",
                6 => "Jun",
                7 => "Jul",
                8 => "Aug",
                9 => "Sep",
                10 => "Oct",
                11 => "Nov",
                12 => "Dec",
                _ => "???",
            };
            format!("{} {}", month, ts_date.day())
        }
    }
}

/// Simple word-wrapping: split content by newlines first, then wrap long lines.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![];
    }
    let mut result = Vec::new();
    for line in text.lines() {
        if line.len() <= max_width {
            result.push(line.to_string());
        } else {
            // Word wrap.
            let words: Vec<&str> = line.split_whitespace().collect();
            let mut current = String::new();
            for word in words {
                if current.is_empty() {
                    current = word.to_string();
                } else if current.len() + 1 + word.len() <= max_width {
                    current.push(' ');
                    current.push_str(word);
                } else {
                    result.push(current);
                    current = word.to_string();
                }
            }
            if !current.is_empty() {
                result.push(current);
            }
        }
    }
    result
}

/// Derive a deterministic RGB color from a username.
///
/// Hash all bytes, truncate to u8, scale to 0..359 HSV hue, convert
/// HSV(hue, 0.7, 0.9) to RGB for a vivid but readable sender-name color.
fn username_to_color(name: &str) -> Color {
    let hash: u8 = name.bytes().fold(0u8, |acc, b| acc.wrapping_add(b));
    let hue = (hash as f32 / 255.0) * 359.0;
    hsv_to_rgb(hue, 0.7, 0.9)
}

/// Convert HSV to ratatui RGB Color. h in [0,360), s and v in [0,1].
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if hp < 1.0 {
        (c, x, 0.0)
    } else if hp < 2.0 {
        (x, c, 0.0)
    } else if hp < 3.0 {
        (0.0, c, x)
    } else if hp < 4.0 {
        (0.0, x, c)
    } else if hp < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    Color::Rgb(
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// Compute scroll offset that keeps the selected message visible.
fn compute_auto_scroll(
    current_scroll: usize,
    selected: usize,
    ranges: &[(usize, usize)],
    visible_height: usize,
    total_lines: usize,
) -> usize {
    if ranges.is_empty() || total_lines <= visible_height {
        return 0;
    }

    let (sel_start, sel_end) = if selected < ranges.len() {
        ranges[selected]
    } else {
        return current_scroll;
    };

    let mut scroll = current_scroll;

    // If the message is taller than the viewport, always show its start.
    let msg_height = sel_end.saturating_sub(sel_start);
    if msg_height >= visible_height {
        scroll = sel_start;
    } else {
        // If selected message starts above the viewport, scroll up to show it.
        if sel_start < scroll {
            scroll = sel_start;
        }

        // If selected message ends below the viewport, scroll down.
        if sel_end > scroll + visible_height {
            scroll = sel_end.saturating_sub(visible_height);
        }
    }

    // Clamp.
    let max_scroll = total_lines.saturating_sub(visible_height);
    scroll.min(max_scroll)
}
