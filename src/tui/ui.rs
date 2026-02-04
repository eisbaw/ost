//! UI rendering for the TUI

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
    Frame,
};

use super::app::App;

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

    // Render main content area (placeholder for now)
    render_main(main_area, frame.buffer_mut());

    // Render status bar
    render_status(status_area, frame.buffer_mut(), app);
}

/// Render the header bar
fn render_header(area: Rect, buf: &mut Buffer, app: &App) {
    let title = Span::styled(
        " OST Client",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let help_indicator = Span::styled(" [?] Help ", Style::default().fg(Color::Gray));

    let (status_symbol, status_color) = status_indicator(app.is_online);
    let online_status = Span::styled(
        format!(" {} online ", status_symbol),
        Style::default().fg(status_color),
    );

    let user_name = Span::styled(
        format!(" {} ", app.user_name),
        Style::default().fg(Color::Cyan),
    );

    // Calculate spacing to right-align the right-side elements
    let left_width = " OST Client".len();
    let right_content = format!("[?] Help  {} online  {} ", status_symbol, app.user_name);
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

/// Render the main content area (placeholder)
fn render_main(area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Main Content ");

    let placeholder = Paragraph::new("Press 'q' to quit")
        .block(block)
        .style(Style::default().fg(Color::Gray));

    placeholder.render(area, buf);
}

/// Render the status bar
fn render_status(area: Rect, buf: &mut Buffer, app: &App) {
    let (conn_symbol, conn_color) = status_indicator(app.is_online);
    let connection = Span::styled(
        format!(" {} {} ", conn_symbol, app.connection_state),
        Style::default().fg(conn_color),
    );

    let sep_style = Style::default().fg(Color::DarkGray);

    let channel = Span::styled(&app.channel_name, Style::default().fg(Color::Yellow));

    let members = Span::styled(
        format!(" {} members ", app.member_count),
        Style::default().fg(Color::Gray),
    );

    let pane = Span::styled(
        format!("Tab: {} ", app.active_pane.as_str()),
        Style::default().fg(Color::Cyan),
    );

    let help_hint = Span::styled("?: help", Style::default().fg(Color::Gray));

    let status_line = Line::from(vec![
        connection,
        Span::styled(" | ", sep_style),
        channel,
        Span::styled(" | ", sep_style),
        members,
        Span::styled(" | ", sep_style),
        pane,
        Span::styled(" | ", sep_style),
        help_hint,
    ]);

    let status = Paragraph::new(status_line).style(Style::default().bg(Color::DarkGray));

    status.render(area, buf);
}
