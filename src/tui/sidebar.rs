//! Sidebar widget: Teams hierarchy with collapsible teams/channels and Chats list.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::api;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// A channel inside a team.
#[derive(Clone)]
pub struct Channel {
    pub name: String,
    /// The real channel/thread ID from the API.
    pub id: String,
    /// Number of unread messages (0 = no badge)
    pub unread: u32,
}

/// A team containing channels.
#[derive(Clone)]
#[allow(dead_code)]
pub struct Team {
    pub name: String,
    /// The real team ID from the API.
    pub id: String,
    pub expanded: bool,
    pub channels: Vec<Channel>,
}

/// A direct-message contact.
#[derive(Clone)]
pub struct Chat {
    pub name: String,
    /// The real chat/conversation thread ID from the API.
    pub id: String,
    /// true = group chat (shows a different icon)
    pub is_group: bool,
    /// Number of unread messages (0 = no badge, use dot for "some")
    pub unread: u32,
    /// Whether this contact is online (show presence dot)
    pub online: bool,
}

/// Sidebar state: owns the data and tracks navigation.
pub struct SidebarState {
    pub teams: Vec<Team>,
    pub chats: Vec<Chat>,
    /// Index into the flat item list (0-based)
    pub selected: usize,
    /// Whether data is still loading.
    pub loading: bool,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            teams: Vec::new(),
            chats: Vec::new(),
            selected: 0,
            loading: true,
        }
    }
}

impl SidebarState {
    /// Update teams data from API response.
    pub fn update_teams(&mut self, teams: Vec<api::TeamInfo>) {
        self.teams = teams
            .into_iter()
            .map(|t| Team {
                name: t.name,
                id: t.id,
                expanded: true,
                channels: t
                    .channels
                    .into_iter()
                    .map(|c| Channel {
                        name: c.name,
                        id: c.id,
                        unread: 0,
                    })
                    .collect(),
            })
            .collect();
        self.clamp_selection();
    }

    /// Update chats data from API response.
    pub fn update_chats(&mut self, chats: Vec<api::ChatInfo>) {
        self.chats = chats
            .into_iter()
            .map(|c| Chat {
                name: c.name,
                id: c.id,
                is_group: c.is_group,
                unread: 0,
                online: false,
            })
            .collect();
        self.clamp_selection();
    }

    /// Get the chat/channel ID of the currently selected item.
    pub fn selected_item_id(&self) -> Option<String> {
        let items = self.flat_items();
        match items.get(self.selected)? {
            SidebarItem::Channel(ti, ci) => Some(self.teams[*ti].channels[*ci].id.clone()),
            SidebarItem::Chat(ci) => Some(self.chats[*ci].id.clone()),
            _ => None,
        }
    }

    /// Get the display name of the currently selected item.
    pub fn selected_item_name(&self) -> Option<String> {
        let items = self.flat_items();
        match items.get(self.selected)? {
            SidebarItem::Channel(ti, ci) => {
                let team = &self.teams[*ti];
                let channel = &team.channels[*ci];
                Some(format!("{} > #{}", team.name, channel.name))
            }
            SidebarItem::Chat(ci) => Some(self.chats[*ci].name.clone()),
            SidebarItem::Team(ti) => Some(self.teams[*ti].name.clone()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Flat item enumeration
// ---------------------------------------------------------------------------

/// One row in the sidebar's flat list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    /// "TEAMS" section header (not selectable, but occupies a row)
    TeamsHeader,
    /// A team row (index into SidebarState.teams)
    Team(usize),
    /// A channel row (team_idx, channel_idx)
    Channel(usize, usize),
    /// "CHATS" separator
    ChatsHeader,
    /// A chat row (index into SidebarState.chats)
    Chat(usize),
}

impl SidebarState {
    /// Build a flat list of items in display order.
    pub fn flat_items(&self) -> Vec<SidebarItem> {
        let mut items = Vec::new();

        // Teams header
        items.push(SidebarItem::TeamsHeader);

        for (ti, team) in self.teams.iter().enumerate() {
            items.push(SidebarItem::Team(ti));
            if team.expanded {
                for (ci, _ch) in team.channels.iter().enumerate() {
                    items.push(SidebarItem::Channel(ti, ci));
                }
            }
        }

        // Chats separator
        items.push(SidebarItem::ChatsHeader);

        for (ci, _chat) in self.chats.iter().enumerate() {
            items.push(SidebarItem::Chat(ci));
        }

        items
    }

    /// Total number of flat items.
    pub fn item_count(&self) -> usize {
        self.flat_items().len()
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.skip_headers_up();
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let count = self.item_count();
        if count == 0 {
            return;
        }
        if self.selected < count - 1 {
            self.selected += 1;
            self.skip_headers_down();
        }
    }

    /// Toggle expand/collapse if current selection is a Team row.
    pub fn toggle_expand(&mut self) {
        let items = self.flat_items();
        if let Some(SidebarItem::Team(ti)) = items.get(self.selected) {
            self.teams[*ti].expanded = !self.teams[*ti].expanded;
        }
    }

    /// Skip non-selectable headers when moving up.
    fn skip_headers_up(&mut self) {
        let items = self.flat_items();
        while self.selected > 0 {
            match items.get(self.selected) {
                Some(SidebarItem::TeamsHeader | SidebarItem::ChatsHeader) => {
                    self.selected -= 1;
                }
                _ => break,
            }
        }
        // If we landed on TeamsHeader (index 0), move down instead
        if let Some(SidebarItem::TeamsHeader | SidebarItem::ChatsHeader) = items.get(self.selected)
        {
            self.skip_headers_down();
        }
    }

    /// Skip non-selectable headers when moving down.
    fn skip_headers_down(&mut self) {
        let items = self.flat_items();
        let count = items.len();
        while self.selected < count - 1 {
            match items.get(self.selected) {
                Some(SidebarItem::TeamsHeader | SidebarItem::ChatsHeader) => {
                    self.selected += 1;
                }
                _ => break,
            }
        }
    }

    /// Clamp selected index to valid range after structural changes.
    pub fn clamp_selection(&mut self) {
        let count = self.item_count();
        if count == 0 {
            self.selected = 0;
            return;
        }
        if self.selected >= count {
            self.selected = count - 1;
        }
        // After clamping, skip headers
        self.skip_headers_down();
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the sidebar into the given area.
pub fn render(area: Rect, buf: &mut Buffer, state: &SidebarState, focused: bool) {
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

    // Show loading indicator if data hasn't arrived yet.
    if state.loading && state.teams.is_empty() && state.chats.is_empty() {
        if inner.height > 0 && inner.width > 0 {
            let loading_area = Rect::new(inner.x, inner.y, inner.width, 1);
            let line = Line::from(Span::styled(
                " Loading...",
                Style::default().fg(Color::DarkGray),
            ));
            Paragraph::new(line).render(loading_area, buf);
        }
        return;
    }

    let items = state.flat_items();
    let available_height = inner.height as usize;

    if available_height == 0 || items.is_empty() {
        return;
    }

    // Compute scroll offset so selected item is visible.
    let scroll_offset = compute_scroll_offset(state.selected, available_height, items.len());

    for (row_idx, item_idx) in (scroll_offset..items.len())
        .take(available_height)
        .enumerate()
    {
        let item = &items[item_idx];
        let ctx = RowCtx {
            area: Rect::new(inner.x, inner.y + row_idx as u16, inner.width, 1),
            selected: item_idx == state.selected,
            pane_focused: focused,
        };

        render_item(buf, &ctx, item, state);
    }
}

/// Simple scroll offset: keep selected item visible.
fn compute_scroll_offset(selected: usize, height: usize, total: usize) -> usize {
    if total <= height {
        return 0;
    }
    if selected < height {
        return 0;
    }
    let max_offset = total.saturating_sub(height);
    let offset = selected.saturating_sub(height - 1);
    offset.min(max_offset)
}

/// Rendering context for a single sidebar row.
struct RowCtx {
    area: Rect,
    selected: bool,
    pane_focused: bool,
}

/// Style for a list item (channel or chat) based on selection and unread state.
fn item_style(selected: bool, has_unread: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else if has_unread {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    }
}

/// Style for a badge (unread count) based on selection state.
fn badge_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Yellow)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    }
}

/// Render a single sidebar item into the buffer.
fn render_item(buf: &mut Buffer, ctx: &RowCtx, item: &SidebarItem, state: &SidebarState) {
    let w = ctx.area.width as usize;
    match item {
        SidebarItem::TeamsHeader => {
            let label = if ctx.pane_focused {
                ">> TEAMS"
            } else {
                "   TEAMS"
            };
            let style = Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD);
            render_row(buf, ctx.area, label, "", style, style);
        }

        SidebarItem::Team(ti) => {
            let team = &state.teams[*ti];
            let indicator = if team.expanded {
                "\u{25BC}"
            } else {
                "\u{25B6}"
            };
            let cursor = if ctx.selected { "\u{25BA}" } else { " " };
            let label = format!("{}{} {}", cursor, indicator, team.name);

            let style = if ctx.selected {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            render_row(buf, ctx.area, &label, "", style, style);
        }

        SidebarItem::Channel(ti, ci) => {
            let channel = &state.teams[*ti].channels[*ci];
            let cursor = if ctx.selected { "\u{25BA}" } else { " " };
            let label = format!("  {}# {}", cursor, channel.name);
            let badge = if channel.unread > 0 {
                format!("{}", channel.unread)
            } else {
                String::new()
            };

            let style = item_style(ctx.selected, channel.unread > 0);
            let bstyle = if channel.unread > 0 {
                badge_style(ctx.selected)
            } else {
                style
            };

            render_row(buf, ctx.area, &label, &badge, style, bstyle);
        }

        SidebarItem::ChatsHeader => {
            // Render a separator line: " -- CHATS --------"
            let prefix = " -- CHATS ";
            let dashes = w.saturating_sub(prefix.len());
            let label = format!("{}{}", prefix, "-".repeat(dashes));
            let style = Style::default().fg(Color::DarkGray);
            render_row(buf, ctx.area, &label, "", style, style);
        }

        SidebarItem::Chat(ci) => {
            let chat = &state.chats[*ci];
            let icon = if chat.is_group { "+" } else { "*" };
            let cursor = if ctx.selected { "\u{25BA}" } else { " " };
            let label = format!("{}{} {}", cursor, icon, chat.name);
            let badge = if chat.unread > 0 {
                format!("{}", chat.unread)
            } else if chat.online {
                "*".to_string()
            } else {
                String::new()
            };

            let style = item_style(ctx.selected, chat.unread > 0);
            let bstyle = if chat.unread > 0 {
                badge_style(ctx.selected)
            } else if chat.online {
                Style::default().fg(Color::Green)
            } else {
                style
            };

            render_row(buf, ctx.area, &label, &badge, style, bstyle);
        }
    }
}

/// Render a row with left-aligned text and an optional right-aligned badge.
fn render_row(
    buf: &mut Buffer,
    area: Rect,
    left: &str,
    badge: &str,
    text_style: Style,
    badge_style: Style,
) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }

    // Truncate left text if needed, leaving room for badge + 1 space
    let badge_len = badge.len();
    let max_left = if badge_len > 0 {
        width.saturating_sub(badge_len + 1)
    } else {
        width
    };

    let left_truncated: String = left.chars().take(max_left).collect();
    let left_len = left_truncated.chars().count();

    // Padding between left text and badge
    let pad = if badge_len > 0 {
        width.saturating_sub(left_len + badge_len)
    } else {
        width.saturating_sub(left_len)
    };

    // Build the line
    let line = Line::from(vec![
        Span::styled(left_truncated, text_style),
        Span::styled(" ".repeat(pad), text_style),
        Span::styled(badge.to_string(), badge_style),
    ]);

    let row_area = Rect::new(area.x, area.y, area.width, 1);
    Paragraph::new(line).render(row_area, buf);
}
