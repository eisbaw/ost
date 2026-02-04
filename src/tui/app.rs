//! TUI Application state and main event loop

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use super::compose::ComposeState;
use super::messages::MessagesState;
use super::sidebar::SidebarState;
use super::ui;

/// Target frame rate for UI updates (~30 fps)
const FRAME_DURATION_MS: u64 = 33;

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
}

impl Default for App {
    fn default() -> Self {
        Self {
            should_exit: false,
            is_online: true,
            user_name: "User".to_string(),
            channel_name: "#general".to_string(),
            member_count: 12,
            connection_state: "Connected".to_string(),
            active_pane: Pane::default(),
            // Start selection on the first selectable item (skip TeamsHeader at index 0)
            sidebar: SidebarState {
                selected: 1,
                ..SidebarState::default()
            },
            messages: MessagesState::default(),
            compose: ComposeState::default(),
            show_help: false,
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

    /// Handle input events
    pub fn handle_events(&mut self) -> Result<()> {
        if event::poll(Duration::from_millis(FRAME_DURATION_MS))? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    // When help popup is visible, any key closes it.
                    if self.show_help {
                        self.show_help = false;
                        return Ok(());
                    }

                    // When the compose pane is focused, most keys are text input.
                    if self.active_pane == Pane::Compose {
                        self.handle_compose_key(key_event);
                    } else {
                        self.handle_navigation_key(key_event);
                    }
                }
                Event::Resize(_, _) => {
                    // Terminal resized - will be handled on next draw
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Handle key events when a non-compose pane is focused.
    fn handle_navigation_key(&mut self, key_event: crossterm::event::KeyEvent) {
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
                self.sidebar.toggle_expand();
                self.sidebar.clamp_selection();
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

    /// Handle key events when the compose pane is focused.
    ///
    /// In compose mode, most keys insert text. Special keys:
    /// - Tab: cycle pane (leave compose)
    /// - Esc: leave compose (go to Messages)
    /// - Enter: send message (clear input)
    /// - Ctrl+Enter: insert newline
    /// - Ctrl+U: clear compose box
    /// - Backspace: delete char before cursor
    /// - Delete: delete char at cursor
    /// - Left/Right: move cursor
    /// - Home/End: move to start/end
    fn handle_compose_key(&mut self, key_event: crossterm::event::KeyEvent) {
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
                    tracing::debug!("Message sent: {}", text);
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

    /// Render the UI
    pub fn render(&self, frame: &mut ratatui::Frame) {
        ui::render(frame, self);
    }
}

/// Run the TUI application with panic-safe terminal restore
pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = catch_unwind(AssertUnwindSafe(|| run_app(&mut terminal)));
    ratatui::restore();

    match result {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

fn run_app(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut app = App::default();

    while !app.should_exit {
        terminal.draw(|frame| app.render(frame))?;
        app.handle_events()?;
    }

    Ok(())
}
