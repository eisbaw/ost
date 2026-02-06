//! UI rendering for the TUI

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Frame,
};

use super::app::{App, Pane};
use super::compose;
use super::debug_log;
use super::help;
use super::messages;
use super::search;
use super::sidebar;

/// Percentage of main area height allocated to content when debug log is visible.
/// The remainder goes to the debug log pane.
const DEBUG_LOG_CONTENT_PERCENT: u16 = 70;

/// Returns status indicator symbol and color based on online state
fn status_indicator(is_online: bool) -> (&'static str, Color) {
    if is_online {
        ("*", Color::Green)
    } else {
        ("o", Color::Red)
    }
}

/// Main render function
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Layout: header (1 line) + main content + status bar (1 line)
    let [header_area, main_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // Render header bar
    render_header(header_area, frame.buffer_mut(), app);

    // If debug log is visible, split main area vertically: content + debug log
    let (content_main_area, debug_log_area) = if app.debug_log.visible {
        let [content, debug] = Layout::vertical([
            Constraint::Percentage(DEBUG_LOG_CONTENT_PERCENT),
            Constraint::Percentage(100 - DEBUG_LOG_CONTENT_PERCENT),
        ])
        .areas(main_area);
        (content, Some(debug))
    } else {
        (main_area, None)
    };

    // Split content_main_area: sidebar (22 cols) + content
    let [sidebar_area, content_area] =
        Layout::horizontal([Constraint::Length(22), Constraint::Fill(1)]).areas(content_main_area);

    // Render sidebar
    sidebar::render(
        sidebar_area,
        frame.buffer_mut(),
        &app.sidebar,
        app.active_pane == Pane::Sidebar,
    );

    // Split content area: messages (fill) + compose box (4 lines)
    let [messages_area, compose_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(compose::COMPOSE_HEIGHT),
    ])
    .areas(content_area);

    // Render messages pane
    messages::render(
        messages_area,
        frame.buffer_mut(),
        &app.messages,
        app.active_pane == Pane::Messages,
        &app.user_name,
    );

    // Render compose box
    compose::render(
        compose_area,
        frame,
        &app.compose,
        &app.channel_name,
        app.active_pane == Pane::Compose,
    );

    // Render debug log pane if visible
    if let Some(debug_area) = debug_log_area {
        debug_log::render(debug_area, frame.buffer_mut(), &app.debug_log);
    }

    // Render status bar
    render_status(status_area, frame.buffer_mut(), app);

    // Render search overlay (on top of main content, below help popup)
    if app.search.active {
        search::render_search_overlay(frame, &app.search);
    }

    // Render help popup overlay (on top of everything else)
    if app.show_help {
        help::render_help_popup(frame);
    }
}

/// Render the header bar
fn render_header(area: Rect, buf: &mut Buffer, app: &App) {
    let title_text = " OST Client ";
    let title = Span::styled(
        title_text,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let help_indicator = Span::styled(" [?] Help ", Style::default().fg(Color::Gray));

    let (status_symbol, status_color) = status_indicator(app.is_online);
    let status_text = if app.is_online {
        "online".to_string()
    } else {
        app.connection_state.clone()
    };
    let online_status = Span::styled(
        format!(" {} {} ", status_symbol, status_text),
        Style::default().fg(status_color),
    );

    let user_name = Span::styled(
        format!(" {} ", app.user_name),
        Style::default().fg(Color::Cyan),
    );

    // Calculate spacing to right-align the right-side elements
    let left_width = title_text.len();
    let right_content = format!("[?] Help  {} {}  {} ", status_symbol, status_text, app.user_name);
    let right_width = right_content.len();
    let padding_width = area.width.saturating_sub((left_width + right_width) as u16) as usize;
    let padding = Span::raw(" ".repeat(padding_width));

    let header_line = Line::from(vec![
        title,
        padding,
        help_indicator,
        online_status,
        user_name,
    ]);

    let header = Paragraph::new(header_line).style(Style::default().bg(Color::DarkGray));

    header.render(area, buf);
}

/// Render the status bar
fn render_status(area: Rect, buf: &mut Buffer, app: &App) {
    // If there's a status message, show it prominently.
    if let Some(ref msg) = app.status_message {
        let style = if app.status_is_error {
            Style::default().fg(Color::Red).bg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Green).bg(Color::DarkGray)
        };
        let line = Line::from(Span::styled(format!(" {} ", msg), style));
        Paragraph::new(line)
            .style(Style::default().bg(Color::DarkGray))
            .render(area, buf);
        return;
    }

    let (conn_symbol, conn_color) = status_indicator(app.is_online);
    let connection = Span::styled(
        format!(" {} {} ", conn_symbol, app.connection_state),
        Style::default().fg(conn_color),
    );

    let sep_style = Style::default().fg(Color::DarkGray);

    let channel_display = if app.channel_name.is_empty() {
        "(none)".to_string()
    } else {
        app.channel_name.clone()
    };
    let channel = Span::styled(channel_display, Style::default().fg(Color::Yellow));

    let pane = Span::styled(
        format!("Tab: {} ", app.active_pane.as_str()),
        Style::default().fg(Color::Cyan),
    );

    let help_hint = Span::styled("?: help", Style::default().fg(Color::Gray));

    let search_hint = Span::styled("C-k: search", Style::default().fg(Color::Gray));

    let status_line = Line::from(vec![
        connection,
        Span::styled(" | ", sep_style),
        channel,
        Span::styled(" | ", sep_style),
        pane,
        Span::styled(" | ", sep_style),
        help_hint,
        Span::styled(" | ", sep_style),
        search_hint,
    ]);

    let status = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));

    status.render(area, buf);
}
