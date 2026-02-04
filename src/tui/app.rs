//! TUI Application state and main event loop

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::DefaultTerminal;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

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

    /// Handle input events
    pub fn handle_events(&mut self) -> Result<()> {
        if event::poll(Duration::from_millis(FRAME_DURATION_MS))? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    match key_event.code {
                        KeyCode::Char('q') => {
                            self.should_exit = true;
                        }
                        KeyCode::Tab => {
                            self.next_pane();
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
                        KeyCode::Down | KeyCode::Char('j')
                            if self.active_pane == Pane::Messages =>
                        {
                            self.messages.select_next();
                        }
                        KeyCode::Enter if self.active_pane == Pane::Messages => {
                            self.messages.toggle_thread();
                        }
                        _ => {}
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
