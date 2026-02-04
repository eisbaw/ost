//! Global search overlay: Ctrl+K activated search across channels, chats, and messages.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
    Frame,
};

use super::messages::MessagesState;
use super::sidebar::SidebarState;

// ---------------------------------------------------------------------------
// Search result types
// ---------------------------------------------------------------------------

/// A single search result pointing to a specific item.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// What kind of item this result points to.
    pub kind: SearchResultKind,
    /// Display label for the result (e.g., "Engineering > #general").
    pub label: String,
    /// The text snippet that matched the query.
    pub context: String,
}

/// The kind of item a search result refers to.
#[derive(Clone, Debug)]
pub enum SearchResultKind {
    /// A channel: (team index, channel index).
    Channel(usize, usize),
    /// A direct chat: (chat index).
    Chat(usize),
    /// A message: (message index).
    Message(usize),
}

// ---------------------------------------------------------------------------
// Search state
// ---------------------------------------------------------------------------

/// State for the global search overlay.
#[derive(Default)]
pub struct SearchState {
    /// Whether the search overlay is active (visible).
    pub active: bool,
    /// Current search query string.
    pub query: String,
    /// Cursor position within the query (character offset).
    pub cursor_pos: usize,
    /// Filtered search results based on the current query.
    pub results: Vec<SearchResult>,
    /// Index of the currently selected result (for navigation).
    pub selected: usize,
}

impl SearchState {
    /// Activate the search overlay (called on Ctrl+K).
    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor_pos = 0;
        self.results.clear();
        self.selected = 0;
    }

    /// Deactivate the search overlay (called on Esc).
    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.cursor_pos = 0;
        self.results.clear();
        self.selected = 0;
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self.char_to_byte(self.cursor_pos);
        self.query.insert(byte_pos, c);
        self.cursor_pos += 1;
    }

    /// Delete the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let byte_pos = self.char_to_byte(self.cursor_pos);
            let prev_byte_pos = self.char_to_byte(self.cursor_pos - 1);
            self.query.drain(prev_byte_pos..byte_pos);
            self.cursor_pos -= 1;
        }
    }

    /// Delete the character at the cursor (delete key).
    pub fn delete_at_cursor(&mut self) {
        let char_count = self.query.chars().count();
        if self.cursor_pos < char_count {
            let byte_pos = self.char_to_byte(self.cursor_pos);
            let next_byte_pos = self.char_to_byte(self.cursor_pos + 1);
            self.query.drain(byte_pos..next_byte_pos);
        }
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let char_count = self.query.chars().count();
        if self.cursor_pos < char_count {
            self.cursor_pos += 1;
        }
    }

    /// Move cursor to the start.
    pub fn move_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to the end.
    pub fn move_end(&mut self) {
        self.cursor_pos = self.query.chars().count();
    }

    /// Move selection to the previous result.
    pub fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection to the next result.
    pub fn select_next(&mut self) {
        if !self.results.is_empty() && self.selected + 1 < self.results.len() {
            self.selected += 1;
        }
    }

    /// Get the currently selected result (if any).
    pub fn selected_result(&self) -> Option<&SearchResult> {
        self.results.get(self.selected)
    }

    /// Update search results by filtering against sidebar and messages data.
    ///
    /// Called whenever the query changes.
    pub fn update_results(&mut self, sidebar: &SidebarState, messages: &MessagesState) {
        self.results.clear();
        self.selected = 0;

        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return;
        }

        // Search channels.
        for (ti, team) in sidebar.teams.iter().enumerate() {
            for (ci, channel) in team.channels.iter().enumerate() {
                let full_name = format!("{} > #{}", team.name, channel.name);
                if full_name.to_lowercase().contains(&query)
                    || channel.name.to_lowercase().contains(&query)
                {
                    self.results.push(SearchResult {
                        kind: SearchResultKind::Channel(ti, ci),
                        label: format!("# {}", full_name),
                        context: format!("Channel in {}", team.name),
                    });
                }
            }
        }

        // Search chats.
        for (ci, chat) in sidebar.chats.iter().enumerate() {
            if chat.name.to_lowercase().contains(&query) {
                let icon = if chat.is_group { "Group" } else { "Chat" };
                self.results.push(SearchResult {
                    kind: SearchResultKind::Chat(ci),
                    label: chat.name.clone(),
                    context: format!("{} conversation", icon),
                });
            }
        }

        // Search messages (sender and content).
        for (mi, msg) in messages.messages.iter().enumerate() {
            let sender_match = msg.sender.to_lowercase().contains(&query);
            let content_match = msg.content.to_lowercase().contains(&query);

            if sender_match || content_match {
                // Build a short snippet of the matching content.
                let snippet = if content_match {
                    // Take the first line of content, truncated.
                    let first_line = msg.content.lines().next().unwrap_or("");
                    let truncated: String = first_line.chars().take(50).collect();
                    if truncated.len() < first_line.len() {
                        format!("{}...", truncated)
                    } else {
                        truncated
                    }
                } else {
                    msg.content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(50)
                        .collect()
                };

                self.results.push(SearchResult {
                    kind: SearchResultKind::Message(mi),
                    label: format!("{} ({})", msg.sender, msg.timestamp),
                    context: snippet,
                });
            }
        }
    }

    /// Convert a char-based cursor position to a byte offset.
    fn char_to_byte(&self, char_pos: usize) -> usize {
        self.query
            .char_indices()
            .nth(char_pos)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len())
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Maximum number of results to display in the dropdown.
const MAX_VISIBLE_RESULTS: usize = 10;

/// Height of the search input bar (border + input + border = 3).
const SEARCH_BAR_HEIGHT: u16 = 3;

/// Render the search overlay at the top of the screen.
///
/// The overlay consists of:
/// - A search input bar spanning the full width
/// - A dropdown list of results below it
///
/// The overlay is drawn on top of existing content using Clear.
pub fn render_search_overlay(frame: &mut Frame, state: &SearchState) {
    if !state.active {
        return;
    }

    let area = frame.area();

    // The overlay starts at the top of the screen, below the header (row 1).
    // Search bar: 3 lines (border + input + border).
    // Results dropdown: up to MAX_VISIBLE_RESULTS + 2 (borders) lines.
    let results_count = state.results.len().min(MAX_VISIBLE_RESULTS);
    let results_height = if state.query.is_empty() {
        0
    } else if state.results.is_empty() {
        3 // "No results" message with borders
    } else {
        results_count as u16 + 2 // results + borders
    };

    let total_height = SEARCH_BAR_HEIGHT + results_height;
    let overlay_y = 1; // Below the header bar
    let overlay_height = total_height.min(area.height.saturating_sub(2)); // Leave room for status

    if overlay_height == 0 {
        return;
    }

    let overlay_area = Rect::new(area.x, overlay_y, area.width, overlay_height);

    // Clear the area behind the overlay.
    frame.render_widget(Clear, overlay_area);

    // Search input bar area.
    let search_bar_area = Rect::new(
        overlay_area.x,
        overlay_area.y,
        overlay_area.width,
        SEARCH_BAR_HEIGHT.min(overlay_area.height),
    );
    render_search_bar(search_bar_area, frame, state);

    // Results dropdown area (if there's room).
    if results_height > 0 && overlay_area.height > SEARCH_BAR_HEIGHT {
        let results_area = Rect::new(
            overlay_area.x + 1,
            overlay_area.y + SEARCH_BAR_HEIGHT,
            overlay_area.width.saturating_sub(2),
            overlay_area.height.saturating_sub(SEARCH_BAR_HEIGHT),
        );
        render_results_dropdown(results_area, frame.buffer_mut(), state);
    }
}

/// Render the search input bar.
fn render_search_bar(area: Rect, frame: &mut Frame, state: &SearchState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Line::from(Span::styled(
            " Search (Esc to close) ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let w = inner.width as usize;

    if state.query.is_empty() {
        // Render placeholder text.
        let placeholder = " Search channels, chats, messages...";
        let truncated: String = placeholder.chars().take(w).collect();
        let line = Line::from(Span::styled(
            truncated,
            Style::default().fg(Color::DarkGray),
        ));
        Paragraph::new(line).render(inner, frame.buffer_mut());
    } else {
        // Render query text with horizontal scrolling.
        let avail = w.saturating_sub(2); // margin
        let query_chars: Vec<char> = state.query.chars().collect();
        let query_len = query_chars.len();

        let (visible, cursor_offset) = if query_len <= avail {
            (state.query.clone(), state.cursor_pos)
        } else {
            let scroll_start = if state.cursor_pos < avail {
                0
            } else {
                state.cursor_pos - avail + 1
            };
            let end = (scroll_start + avail).min(query_len);
            let visible: String = query_chars[scroll_start..end].iter().collect();
            let cursor_offset = state.cursor_pos - scroll_start;
            (visible, cursor_offset)
        };

        let line = Line::from(Span::styled(
            format!(" {}", visible),
            Style::default().fg(Color::White),
        ));
        Paragraph::new(line).render(inner, frame.buffer_mut());

        // Set cursor position.
        let cx = inner.x + 1 + cursor_offset as u16;
        let cy = inner.y;
        frame.set_cursor_position((cx, cy));
    }

    // If query is empty, still show the cursor at the start.
    if state.query.is_empty() {
        frame.set_cursor_position((inner.x + 1, inner.y));
    }
}

/// Render the results dropdown below the search bar.
fn render_results_dropdown(area: Rect, buf: &mut Buffer, state: &SearchState) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if state.results.is_empty() {
        // "No results" message.
        let line = Line::from(Span::styled(
            " No results found",
            Style::default().fg(Color::DarkGray),
        ));
        Paragraph::new(line).render(inner, buf);
        return;
    }

    let visible_count = state.results.len().min(inner.height as usize);

    // Compute scroll offset so selected result is visible.
    let scroll_offset = if state.selected < visible_count {
        0
    } else {
        state.selected - visible_count + 1
    };

    for (row, idx) in (scroll_offset..state.results.len())
        .take(visible_count)
        .enumerate()
    {
        let result = &state.results[idx];
        let is_selected = idx == state.selected;
        let row_area = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);

        render_result_row(row_area, buf, result, is_selected);
    }
}

/// Render a single result row.
fn render_result_row(area: Rect, buf: &mut Buffer, result: &SearchResult, selected: bool) {
    let w = area.width as usize;
    if w == 0 {
        return;
    }

    let bg = if selected {
        Color::DarkGray
    } else {
        Color::Reset
    };

    let icon = match &result.kind {
        SearchResultKind::Channel(_, _) => "#",
        SearchResultKind::Chat(_) => "*",
        SearchResultKind::Message(_) => ">",
    };

    let icon_style = Style::default()
        .fg(Color::Cyan)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let label_style = if selected {
        Style::default()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White).bg(bg)
    };

    let context_style = Style::default().fg(Color::DarkGray).bg(bg);

    // Format: " # Label  -  context..."
    let prefix = format!(" {} ", icon);
    let separator = " - ";

    let prefix_len = prefix.len();
    let sep_len = separator.len();
    let label_len = result.label.len();

    let max_context = w
        .saturating_sub(prefix_len)
        .saturating_sub(label_len)
        .saturating_sub(sep_len);
    let context_truncated: String = result.context.chars().take(max_context).collect();

    let pad_len = w
        .saturating_sub(prefix_len)
        .saturating_sub(label_len)
        .saturating_sub(sep_len)
        .saturating_sub(context_truncated.len());

    let line = Line::from(vec![
        Span::styled(prefix, icon_style),
        Span::styled(result.label.clone(), label_style),
        Span::styled(separator, context_style),
        Span::styled(context_truncated, context_style),
        Span::styled(" ".repeat(pad_len), Style::default().bg(bg)),
    ]);

    Paragraph::new(line).render(area, buf);
}
