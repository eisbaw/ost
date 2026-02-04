//! Messages pane: displays channel messages with reactions, replies, and attachments.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

// ---------------------------------------------------------------------------
// Data model (mock / demo)
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
}

impl Default for MessagesState {
    fn default() -> Self {
        let messages = mock_messages();
        let count = messages.len();
        Self {
            channel_header: "Engineering Team > #general".to_string(),
            expanded_threads: vec![true; count],
            messages,
            scroll_offset: 0,
            selected: 0,
        }
    }
}

/// Build mock messages matching the TUI spec.
fn mock_messages() -> Vec<Message> {
    vec![
        Message {
            sender: "Sarah Chen".to_string(),
            timestamp: "9:15 AM today".to_string(),
            content: "Hey team! Just pushed the new authentication module to\n\
                      staging. Can someone help review? @Alex @Jordan"
                .to_string(),
            reactions: vec![
                Reaction {
                    label: "+1".to_string(),
                    count: 3,
                },
                Reaction {
                    label: "<3".to_string(),
                    count: 1,
                },
            ],
            reply_count: 2,
            replies: vec![
                Message {
                    sender: "Alex Rivera".to_string(),
                    timestamp: "9:22 AM".to_string(),
                    content: "Looks good! Found one edge case - what happens\n\
                              when the token expires mid-session?"
                        .to_string(),
                    reactions: vec![],
                    reply_count: 0,
                    replies: vec![],
                    attachments: vec![],
                },
                Message {
                    sender: "Sarah Chen".to_string(),
                    timestamp: "9:25 AM".to_string(),
                    content: "Good catch! Added refresh handler. See a3f2d1".to_string(),
                    reactions: vec![],
                    reply_count: 0,
                    replies: vec![],
                    attachments: vec![],
                },
            ],
            attachments: vec![Attachment {
                name: "auth-module-v2.zip".to_string(),
            }],
        },
        Message {
            sender: "Jordan Lee".to_string(),
            timestamp: "9:45 AM today".to_string(),
            content: "Reminder: Sprint planning at 2pm!\n\
                      Please update your tickets before the meeting."
                .to_string(),
            reactions: vec![
                Reaction {
                    label: "+1".to_string(),
                    count: 5,
                },
                Reaction {
                    label: "eyes".to_string(),
                    count: 2,
                },
            ],
            reply_count: 0,
            replies: vec![],
            attachments: vec![],
        },
        Message {
            sender: "Alex Rivera".to_string(),
            timestamp: "10:02 AM today".to_string(),
            content: "Has anyone looked at the CI pipeline? Builds are\n\
                      taking 15+ minutes since yesterday."
                .to_string(),
            reactions: vec![Reaction {
                label: "+1".to_string(),
                count: 2,
            }],
            reply_count: 1,
            replies: vec![Message {
                sender: "Jordan Lee".to_string(),
                timestamp: "10:10 AM".to_string(),
                content: "Yeah, I noticed. The new integration tests added\n\
                          ~8 min. Working on parallelizing them."
                    .to_string(),
                reactions: vec![],
                reply_count: 0,
                replies: vec![],
                attachments: vec![],
            }],
            attachments: vec![],
        },
        Message {
            sender: "Sarah Chen".to_string(),
            timestamp: "10:30 AM today".to_string(),
            content: "FYI: Updated the API docs for the auth endpoints.\n\
                      Please review when you get a chance."
                .to_string(),
            reactions: vec![],
            reply_count: 0,
            replies: vec![],
            attachments: vec![
                Attachment {
                    name: "api-docs-v3.pdf".to_string(),
                },
                Attachment {
                    name: "changelog.md".to_string(),
                },
            ],
        },
    ]
}

impl MessagesState {
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
pub fn render(area: Rect, buf: &mut Buffer, state: &MessagesState, focused: bool) {
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

    // Pre-render all messages into a line buffer (single pass produces lines + ranges).
    let (all_lines, msg_line_ranges) = build_message_lines(state, messages_area.width as usize);
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
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
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
        render_message_card(&mut lines, msg, width, is_selected, false, 0);

        // Render thread replies if expanded.
        if thread_expanded && !msg.replies.is_empty() {
            for reply in &msg.replies {
                render_message_card(&mut lines, reply, width, false, true, 4);
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
fn render_message_card(
    lines: &mut Vec<Line<'static>>,
    msg: &Message,
    width: usize,
    is_selected: bool,
    is_reply: bool,
    indent: usize,
) {
    let indent_str: String = " ".repeat(indent);
    let reply_prefix = if is_reply { " -> " } else { "" };

    // Card width: available width minus indent and margins.
    let card_inner_width =
        width
            .saturating_sub(indent)
            .saturating_sub(if is_reply { 6 } else { 2 });

    if card_inner_width < 10 {
        // Too narrow to render anything useful.
        return;
    }

    let border_style = if is_selected {
        Style::default().fg(Color::Yellow)
    } else if is_reply {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Gray)
    };

    let sender_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let timestamp_style = Style::default().fg(Color::DarkGray);

    // Top border of card.
    let top_border = format!(
        "{}{}+-{}-+",
        indent_str,
        reply_prefix,
        "-".repeat(card_inner_width.saturating_sub(2))
    );
    lines.push(Line::from(Span::styled(top_border, border_style)));

    // Sender line: "| sender              timestamp |"
    let sender_ts_pad = card_inner_width
        .saturating_sub(msg.sender.len())
        .saturating_sub(msg.timestamp.len())
        .saturating_sub(2);
    let sender_line_prefix = format!("{}{}", indent_str, if is_reply { "    " } else { "" });
    lines.push(Line::from(vec![
        Span::raw(sender_line_prefix.clone()),
        Span::styled("| ".to_string(), border_style),
        Span::styled(format!(" {}", msg.sender), sender_style),
        Span::raw(" ".repeat(sender_ts_pad)),
        Span::styled(format!("{} ", msg.timestamp), timestamp_style),
        Span::styled("|".to_string(), border_style),
    ]));

    // Blank line inside card.
    let blank_inner = format!(
        "{}{}| {} |",
        indent_str,
        if is_reply { "    " } else { "" },
        " ".repeat(card_inner_width.saturating_sub(2))
    );
    lines.push(Line::from(Span::styled(blank_inner.clone(), border_style)));

    // Content lines (word-wrapped).
    let content_width = card_inner_width.saturating_sub(2);
    let content_lines = wrap_text(&msg.content, content_width);
    for cl in &content_lines {
        let pad = content_width.saturating_sub(cl.len());
        lines.push(Line::from(vec![
            Span::raw(format!(
                "{}{}",
                indent_str,
                if is_reply { "    " } else { "" }
            )),
            Span::styled("| ".to_string(), border_style),
            Span::raw(format!("{}{}", cl, " ".repeat(pad))),
            Span::styled(" |".to_string(), border_style),
        ]));
    }

    // Attachments.
    for att in &msg.attachments {
        // Blank line before attachment.
        lines.push(Line::from(Span::styled(blank_inner.clone(), border_style)));

        let att_text = format!("[file] {}", att.name);
        let att_pad = content_width.saturating_sub(att_text.len());
        lines.push(Line::from(vec![
            Span::raw(format!(
                "{}{}",
                indent_str,
                if is_reply { "    " } else { "" }
            )),
            Span::styled("| ".to_string(), border_style),
            Span::styled(
                att_text,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
            ),
            Span::raw(" ".repeat(att_pad)),
            Span::styled(" |".to_string(), border_style),
        ]));
    }

    // Reactions and reply count.
    if !msg.reactions.is_empty() || msg.reply_count > 0 {
        // Blank line before reactions.
        lines.push(Line::from(Span::styled(blank_inner.clone(), border_style)));

        let mut reaction_spans: Vec<Span<'static>> = Vec::new();
        reaction_spans.push(Span::raw(format!(
            "{}{}",
            indent_str,
            if is_reply { "    " } else { "" }
        )));
        reaction_spans.push(Span::styled("| ".to_string(), border_style));

        let mut reaction_text_len: usize = 0;

        for (i, r) in msg.reactions.iter().enumerate() {
            let r_text = format!("{} {}", r.label, r.count);
            reaction_text_len += r_text.len();
            reaction_spans.push(Span::styled(r_text, Style::default().fg(Color::Yellow)));
            if i + 1 < msg.reactions.len() || msg.reply_count > 0 {
                reaction_spans.push(Span::raw("   "));
                reaction_text_len += 3;
            }
        }

        if msg.reply_count > 0 {
            let reply_text = format!(">> {} replies", msg.reply_count);
            reaction_text_len += reply_text.len();
            reaction_spans.push(Span::styled(reply_text, Style::default().fg(Color::Cyan)));
        }

        let reaction_pad = content_width.saturating_sub(reaction_text_len);
        reaction_spans.push(Span::raw(" ".repeat(reaction_pad)));
        reaction_spans.push(Span::styled(" |".to_string(), border_style));

        lines.push(Line::from(reaction_spans));
    }

    // Bottom border.
    let bottom_border = format!(
        "{}{}+-{}-+",
        indent_str,
        if is_reply { "    " } else { "" },
        "-".repeat(card_inner_width.saturating_sub(2))
    );
    lines.push(Line::from(Span::styled(bottom_border, border_style)));
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
