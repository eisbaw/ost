//! TUI Application state and main event loop

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio_stream::StreamExt;

use super::backend::{Backend, BackendCommand, BackendResponse};
use super::compose::ComposeState;
use super::messages::MessagesState;
use super::search::SearchState;
use super::sidebar::SidebarState;
use super::ui;

/// Active pane in the TUI
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Sidebar,
    Messages,
    Compose,
}

impl Pane {
    pub fn as_str(&self) -> &'static str {
        match self {
            Pane::Sidebar => "sidebar",
            Pane::Messages => "messages",
            Pane::Compose => "compose",
        }
    }
}

/// Application state
pub struct App {
    /// Whether the app should exit
    pub should_exit: bool,
    /// Online status (for display)
    pub is_online: bool,
    /// Current user name
    pub user_name: String,
    /// Current channel name
    pub channel_name: String,
    /// Member count
    #[allow(dead_code)]
    pub member_count: u32,
    /// Connection state description
    pub connection_state: String,
    /// Active pane
    pub active_pane: Pane,
    /// Sidebar state (teams/channels/chats + navigation)
    pub sidebar: SidebarState,
    /// Messages pane state
    pub messages: MessagesState,
    /// Compose box state
    pub compose: ComposeState,
    /// Whether the help popup is visible
    pub show_help: bool,
    /// Global search overlay state
    pub search: SearchState,
    /// The chat/channel ID currently being viewed.
    pub current_chat_id: Option<String>,
    /// Status message shown in the status bar (errors, info).
    pub status_message: Option<String>,
    /// Whether the status message is an error.
    pub status_is_error: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            should_exit: false,
            is_online: false,
            user_name: "Loading...".to_string(),
            channel_name: "".to_string(),
            member_count: 0,
            connection_state: "Connecting...".to_string(),
            active_pane: Pane::default(),
            sidebar: SidebarState::default(),
            messages: MessagesState::default(),
            compose: ComposeState::default(),
            show_help: false,
            search: SearchState::default(),
            current_chat_id: None,
            status_message: None,
            status_is_error: false,
        }
    }
}

impl App {
    /// Cycle to the next pane.
    fn next_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Sidebar => Pane::Messages,
            Pane::Messages => Pane::Compose,
            Pane::Compose => Pane::Sidebar,
        };
    }

    /// Cycle to the previous pane (reverse of next_pane).
    fn prev_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Sidebar => Pane::Compose,
            Pane::Messages => Pane::Sidebar,
            Pane::Compose => Pane::Messages,
        };
    }

    /// Handle a crossterm event.
    pub fn handle_event(&mut self, event: Event, backend: &Backend) {
        if let Event::Key(key_event) = event {
            if key_event.kind != KeyEventKind::Press {
                return;
            }

            // When help popup is visible, any key closes it.
            if self.show_help {
                self.show_help = false;
                return;
            }

            // Clear status message on any keypress.
            self.status_message = None;

            // When search overlay is active, route all keys to search handler.
            if self.search.active {
                self.handle_search_key(key_event);
                return;
            }

            // Ctrl+K activates global search from any mode.
            if key_event.code == KeyCode::Char('k')
                && key_event.modifiers.contains(KeyModifiers::CONTROL)
            {
                self.search.activate();
                return;
            }

            // When the compose pane is focused, most keys are text input.
            if self.active_pane == Pane::Compose {
                self.handle_compose_key(key_event, backend);
            } else {
                self.handle_navigation_key(key_event, backend);
            }
        }
    }

    /// Handle key events when a non-compose pane is focused.
    fn handle_navigation_key(&mut self, key_event: crossterm::event::KeyEvent, backend: &Backend) {
        match key_event.code {
            KeyCode::Char('q') => {
                self.should_exit = true;
            }
            KeyCode::Tab => {
                self.next_pane();
            }
            KeyCode::BackTab => {
                self.prev_pane();
            }
            KeyCode::Right => {
                self.next_pane();
            }
            KeyCode::Left => {
                self.prev_pane();
            }
            // Direct pane jump with number keys
            KeyCode::Char('1') => {
                self.active_pane = Pane::Sidebar;
            }
            KeyCode::Char('2') => {
                self.active_pane = Pane::Messages;
            }
            KeyCode::Char('3') => {
                self.active_pane = Pane::Compose;
            }
            // Sidebar-specific keys (only when sidebar is focused)
            KeyCode::Up | KeyCode::Char('k') if self.active_pane == Pane::Sidebar => {
                self.sidebar.move_up();
            }
            KeyCode::Down | KeyCode::Char('j') if self.active_pane == Pane::Sidebar => {
                self.sidebar.move_down();
            }
            KeyCode::Enter if self.active_pane == Pane::Sidebar => {
                self.handle_sidebar_enter(backend);
            }
            // Messages pane keys
            KeyCode::Up | KeyCode::Char('k') if self.active_pane == Pane::Messages => {
                self.messages.select_previous();
            }
            KeyCode::Down | KeyCode::Char('j') if self.active_pane == Pane::Messages => {
                self.messages.select_next();
            }
            KeyCode::Enter if self.active_pane == Pane::Messages => {
                self.messages.toggle_thread();
            }
            // Help popup toggle (available from any non-compose pane)
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
            }
            _ => {}
        }
    }

    /// Handle Enter key on a sidebar item.
    ///
    /// If the selected item is a team, toggle expand/collapse.
    /// If it's a channel or chat, load its messages.
    fn handle_sidebar_enter(&mut self, backend: &Backend) {
        let items = self.sidebar.flat_items();
        let item = match items.get(self.sidebar.selected) {
            Some(item) => *item,
            None => return,
        };

        match item {
            super::sidebar::SidebarItem::Team(_) => {
                self.sidebar.toggle_expand();
                self.sidebar.clamp_selection();
            }
            super::sidebar::SidebarItem::Channel(_, _) | super::sidebar::SidebarItem::Chat(_) => {
                if let Some(id) = self.sidebar.selected_item_id() {
                    let name = self.sidebar.selected_item_name().unwrap_or_default();
                    self.current_chat_id = Some(id.clone());
                    self.channel_name = name.clone();
                    self.messages.loading = true;
                    self.messages.channel_header = name;
                    self.messages.messages.clear();
                    backend.send(BackendCommand::LoadMessages {
                        chat_id: id,
                        limit: 50,
                    });
                }
            }
            _ => {}
        }
    }

    /// Handle key events when the compose pane is focused.
    fn handle_compose_key(&mut self, key_event: crossterm::event::KeyEvent, backend: &Backend) {
        let modifiers = key_event.modifiers;
        let code = key_event.code;

        match (code, modifiers) {
            // Tab always cycles pane focus.
            (KeyCode::Tab, _) => {
                self.next_pane();
            }
            // Shift+Tab cycles backward.
            (KeyCode::BackTab, _) => {
                self.prev_pane();
            }
            // Esc leaves compose and goes to Messages pane.
            (KeyCode::Esc, _) => {
                self.active_pane = Pane::Messages;
            }
            // Ctrl+Enter inserts a newline.
            (KeyCode::Enter, m) if m.contains(KeyModifiers::CONTROL) => {
                self.compose.insert_newline();
            }
            // Enter sends the message.
            (KeyCode::Enter, _) => {
                if let Some(text) = self.compose.send() {
                    if let Some(ref chat_id) = self.current_chat_id {
                        backend.send(BackendCommand::SendMessage {
                            chat_id: chat_id.clone(),
                            message: text,
                        });
                    } else {
                        self.status_message =
                            Some("No chat selected. Select a channel or chat first.".to_string());
                        self.status_is_error = true;
                    }
                }
            }
            // Ctrl+U clears the compose box.
            (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.compose.clear();
            }
            // Backspace deletes character before cursor.
            (KeyCode::Backspace, _) => {
                self.compose.backspace();
            }
            // Delete removes character at cursor.
            (KeyCode::Delete, _) => {
                self.compose.delete();
            }
            // Arrow keys for cursor movement.
            (KeyCode::Left, _) => {
                self.compose.move_left();
            }
            (KeyCode::Right, _) => {
                self.compose.move_right();
            }
            (KeyCode::Home, _) => {
                self.compose.move_home();
            }
            (KeyCode::End, _) => {
                self.compose.move_end();
            }
            // Regular character input.
            (KeyCode::Char(c), m) => {
                // Only insert if no modifiers or just shift (for uppercase).
                if m.is_empty() || m == KeyModifiers::SHIFT {
                    self.compose.insert_char(c);
                }
            }
            _ => {}
        }
    }

    /// Handle key events when the search overlay is active.
    fn handle_search_key(&mut self, key_event: crossterm::event::KeyEvent) {
        let code = key_event.code;
        let modifiers = key_event.modifiers;

        match (code, modifiers) {
            // Esc closes the search overlay.
            (KeyCode::Esc, _) => {
                self.search.deactivate();
            }
            // Up arrow navigates results.
            (KeyCode::Up, _) => {
                self.search.select_previous();
            }
            // Down arrow navigates results.
            (KeyCode::Down, _) => {
                self.search.select_next();
            }
            // Enter selects the current result.
            (KeyCode::Enter, _) => {
                self.apply_search_selection();
            }
            // Backspace deletes character before cursor.
            (KeyCode::Backspace, _) => {
                self.search.backspace();
                self.search.update_results(&self.sidebar, &self.messages);
            }
            // Delete removes character at cursor.
            (KeyCode::Delete, _) => {
                self.search.delete_at_cursor();
                self.search.update_results(&self.sidebar, &self.messages);
            }
            // Left/Right move cursor.
            (KeyCode::Left, _) => {
                self.search.move_left();
            }
            (KeyCode::Right, _) => {
                self.search.move_right();
            }
            (KeyCode::Home, _) => {
                self.search.move_home();
            }
            (KeyCode::End, _) => {
                self.search.move_end();
            }
            // Regular character input.
            (KeyCode::Char(c), m) => {
                // Only insert if no modifiers or just shift (for uppercase).
                if m.is_empty() || m == KeyModifiers::SHIFT {
                    self.search.insert_char(c);
                    self.search.update_results(&self.sidebar, &self.messages);
                }
            }
            _ => {}
        }
    }

    /// Apply the currently selected search result: navigate to the matching item.
    fn apply_search_selection(&mut self) {
        use super::search::SearchResultKind;

        let result = match self.search.selected_result() {
            Some(r) => r.kind.clone(),
            None => {
                self.search.deactivate();
                return;
            }
        };

        match result {
            SearchResultKind::Channel(team_idx, channel_idx) => {
                // Expand the team if collapsed, then select the channel in sidebar.
                if !self.sidebar.teams[team_idx].expanded {
                    self.sidebar.teams[team_idx].expanded = true;
                }
                // Find the flat index of this channel in the sidebar.
                let items = self.sidebar.flat_items();
                for (idx, item) in items.iter().enumerate() {
                    if let super::sidebar::SidebarItem::Channel(ti, ci) = item {
                        if *ti == team_idx && *ci == channel_idx {
                            self.sidebar.selected = idx;
                            break;
                        }
                    }
                }
                self.active_pane = Pane::Sidebar;
            }
            SearchResultKind::Chat(chat_idx) => {
                // Select the chat in the sidebar.
                let items = self.sidebar.flat_items();
                for (idx, item) in items.iter().enumerate() {
                    if let super::sidebar::SidebarItem::Chat(ci) = item {
                        if *ci == chat_idx {
                            self.sidebar.selected = idx;
                            break;
                        }
                    }
                }
                self.active_pane = Pane::Sidebar;
            }
            SearchResultKind::Message(msg_idx) => {
                // Select the message in the messages pane.
                if msg_idx < self.messages.messages.len() {
                    self.messages.selected = msg_idx;
                }
                self.active_pane = Pane::Messages;
            }
        }

        self.search.deactivate();
    }

    /// Handle a response from the async backend.
    fn handle_backend_response(&mut self, response: BackendResponse, backend: &Backend) {
        match response {
            BackendResponse::Teams(Ok(teams)) => {
                self.sidebar.update_teams(teams);
                self.sidebar.loading = false;
                self.close_stale_search();
                // If this is the first data load and we have teams, select the first
                // selectable item (skip TeamsHeader).
                if self.sidebar.selected == 0 {
                    self.sidebar.clamp_selection();
                }
            }
            BackendResponse::Teams(Err(e)) => {
                self.set_error(format!("Failed to load teams: {:#}", e));
                self.sidebar.loading = false;
            }
            BackendResponse::Chats(Ok(chats)) => {
                self.sidebar.update_chats(chats);
                self.sidebar.loading = false;
                self.close_stale_search();
            }
            BackendResponse::Chats(Err(e)) => {
                self.set_error(format!("Failed to load chats: {:#}", e));
                self.sidebar.loading = false;
            }
            BackendResponse::Messages { chat_id, result } => {
                // Only apply if this is still the chat we're looking at.
                if self.current_chat_id.as_deref() == Some(&chat_id) {
                    match result {
                        Ok(msgs) => {
                            let header = self.messages.channel_header.clone();
                            self.messages.update_messages(&header, msgs);
                            self.close_stale_search();
                        }
                        Err(e) => {
                            self.messages.loading = false;
                            self.set_error(format!("Failed to load messages: {:#}", e));
                        }
                    }
                }
            }
            BackendResponse::MessageSent(Ok(())) => {
                self.status_message = Some("Message sent".to_string());
                self.status_is_error = false;
                // Reload messages for the current chat.
                if let Some(ref chat_id) = self.current_chat_id {
                    backend.send(BackendCommand::LoadMessages {
                        chat_id: chat_id.clone(),
                        limit: 50,
                    });
                }
            }
            BackendResponse::MessageSent(Err(e)) => {
                self.set_error(format!("Failed to send message: {:#}", e));
            }
            BackendResponse::UserInfo(Ok(info)) => {
                self.user_name = info.display_name;
            }
            BackendResponse::UserInfo(Err(e)) => {
                self.set_error(format!("Failed to load user info: {:#}", e));
            }
            BackendResponse::Presence(Ok(presence)) => {
                let is_online = presence.availability != "Offline"
                    && presence.availability != "PresenceUnknown";
                self.is_online = is_online;
                self.connection_state = if is_online {
                    "Connected".to_string()
                } else {
                    format!("Status: {}", presence.availability)
                };
            }
            BackendResponse::Presence(Err(e)) => {
                tracing::debug!("Failed to load presence: {:#}", e);
                // Presence failure is non-critical; don't show error in status bar.
                self.connection_state = "Connected".to_string();
                self.is_online = true;
            }
            BackendResponse::ClientError(msg) => {
                self.connection_state = "Not authenticated".to_string();
                self.is_online = false;
                self.sidebar.loading = false;
                self.set_error(format!("Auth: {}", msg));
            }
        }
    }

    /// Close the search overlay if it's open.
    ///
    /// Called when backend data arrives to prevent stale search result indices
    /// from pointing at the wrong sidebar/message items.
    fn close_stale_search(&mut self) {
        if self.search.active {
            self.search.deactivate();
        }
    }

    /// Set an error status message.
    fn set_error(&mut self, msg: String) {
        self.status_message = Some(msg);
        self.status_is_error = true;
    }

    /// Render the UI
    pub fn render(&self, frame: &mut ratatui::Frame) {
        ui::render(frame, self);
    }
}

/// Run the TUI application with terminal restore on exit.
///
/// Sets up a panic hook so the terminal is always restored even on panic.
pub async fn run() -> Result<()> {
    // Install a panic hook that restores the terminal before printing the panic.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();
    let res = run_app(&mut terminal).await;
    ratatui::restore();
    res
}

async fn run_app(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut app = App::default();
    let mut backend = Backend::start();
    let mut events = EventStream::new();

    // Fire initial data loads.
    backend.send(BackendCommand::LoadTeams);
    backend.send(BackendCommand::LoadChats { limit: 50 });
    backend.send(BackendCommand::LoadUserInfo);
    backend.send(BackendCommand::LoadPresence);

    while !app.should_exit {
        terminal.draw(|frame| app.render(frame))?;

        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        app.handle_event(event, &backend);
                    }
                    Some(Err(e)) => {
                        tracing::error!("Event stream error: {:#}", e);
                    }
                    None => {
                        // Event stream ended.
                        break;
                    }
                }
            }
            maybe_response = backend.recv() => {
                match maybe_response {
                    Some(response) => {
                        app.handle_backend_response(response, &backend);
                    }
                    None => {
                        // Backend channel closed.
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
