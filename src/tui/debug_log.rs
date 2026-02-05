//! Debug log pane for displaying captured tracing output in the TUI.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use super::log_capture::LogBuffer;

/// Maximum number of accumulated lines to keep in the debug log.
///
/// This is larger than the ring buffer capacity (500) to provide more scroll
/// history for display purposes. The ring buffer acts as backpressure on writes,
/// while this limit controls how much history the user can scroll through.
const MAX_ACCUMULATED_LINES: usize = 1000;

/// State for the debug log pane.
pub struct DebugLogState {
    /// The ring buffer receiving log output from tracing.
    buffer: LogBuffer,
    /// Accumulated log lines (drains from buffer each refresh).
    lines: Vec<String>,
    /// Whether the debug log pane is visible.
    pub visible: bool,
    /// Scroll offset (0 = viewing most recent lines at bottom).
    scroll_offset: usize,
}

impl DebugLogState {
    /// Create a new debug log state with the given log buffer.
    pub fn new(buffer: LogBuffer) -> Self {
        Self {
            buffer,
            lines: Vec::new(),
            visible: false,
            scroll_offset: 0,
        }
    }

    /// Drain new lines from the ring buffer into accumulated lines.
    ///
    /// Call this every event loop iteration to prevent unbounded mutex growth.
    pub fn refresh(&mut self) {
        let new_lines = self.buffer.drain();
        if !new_lines.is_empty() {
            self.lines.extend(new_lines);
            // Cap accumulated lines.
            if self.lines.len() > MAX_ACCUMULATED_LINES {
                let excess = self.lines.len() - MAX_ACCUMULATED_LINES;
                self.lines.drain(..excess);
                // Adjust scroll offset to stay valid.
                self.scroll_offset = self.scroll_offset.saturating_sub(excess);
            }
        }
    }

    /// Toggle visibility of the debug log pane.
    ///
    /// When opening, auto-scroll to the bottom (most recent logs).
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            // Auto-scroll to bottom.
            self.scroll_offset = 0;
        }
    }

    /// Scroll up by n lines (toward older logs).
    ///
    /// Clamps to prevent scrolling past the oldest line.
    pub fn scroll_up(&mut self, n: usize) {
        let max_offset = self.lines.len().saturating_sub(1);
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(max_offset);
    }

    /// Scroll down by n lines (toward newer logs).
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Get the current line count.
    #[cfg(test)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// Render the debug log pane.
pub fn render(area: Rect, buf: &mut Buffer, state: &DebugLogState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Debug Log ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let visible_lines = inner.height as usize;
    let total_lines = state.lines.len();

    // Calculate which lines to show.
    // scroll_offset=0 means show the last `visible_lines` lines.
    // scroll_offset=N means show lines ending N lines earlier.
    let end_idx = total_lines.saturating_sub(state.scroll_offset);
    let start_idx = end_idx.saturating_sub(visible_lines);

    let lines_to_show: Vec<Line> = state.lines[start_idx..end_idx]
        .iter()
        .map(|line| colorize_log_line(line))
        .collect();

    let para = Paragraph::new(lines_to_show);
    para.render(inner, buf);
}

/// Parse a log line and colorize based on level prefix.
///
/// tracing-subscriber fmt layer outputs lines like:
/// "2024-01-15T10:30:00Z  INFO token refresh succeeded"
/// "2024-01-15T10:30:00Z DEBUG fetching chats"
/// "2024-01-15T10:30:00Z  WARN connection slow"
/// "2024-01-15T10:30:00Z ERROR failed to connect"
fn colorize_log_line(line: &str) -> Line<'static> {
    // Look for level indicators in the line.
    let color = if line.contains(" ERROR ") || line.contains("ERROR:") {
        Color::Red
    } else if line.contains(" WARN ") || line.contains("WARN:") {
        Color::Yellow
    } else if line.contains(" INFO ") || line.contains("INFO:") {
        Color::Green
    } else if line.contains(" DEBUG ") || line.contains("DEBUG:") {
        Color::DarkGray
    } else if line.contains(" TRACE ") || line.contains("TRACE:") {
        Color::DarkGray
    } else {
        Color::White
    };

    Line::from(Span::styled(line.to_owned(), Style::default().fg(color)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_log_refresh() {
        let buffer = LogBuffer::new();
        buffer.push("line 1".to_string());
        buffer.push("line 2".to_string());

        let mut state = DebugLogState::new(buffer.clone());
        assert_eq!(state.line_count(), 0);

        state.refresh();
        assert_eq!(state.line_count(), 2);

        // Add more lines.
        buffer.push("line 3".to_string());
        state.refresh();
        assert_eq!(state.line_count(), 3);
    }

    #[test]
    fn test_debug_log_toggle() {
        let buffer = LogBuffer::new();
        let mut state = DebugLogState::new(buffer);

        assert!(!state.visible);
        state.toggle();
        assert!(state.visible);
        state.toggle();
        assert!(!state.visible);
    }

    #[test]
    fn test_debug_log_scroll() {
        let buffer = LogBuffer::new();
        // Add some lines so we have something to scroll.
        for i in 0..20 {
            buffer.push(format!("line {}", i));
        }
        let mut state = DebugLogState::new(buffer);
        state.refresh(); // Pull lines into state.

        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 5);

        state.scroll_down(3);
        assert_eq!(state.scroll_offset, 2);

        state.scroll_down(10);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_debug_log_scroll_clamps() {
        let buffer = LogBuffer::new();
        // Add only 5 lines.
        for i in 0..5 {
            buffer.push(format!("line {}", i));
        }
        let mut state = DebugLogState::new(buffer);
        state.refresh();

        // Try to scroll up way past the content.
        state.scroll_up(100);
        // Should be clamped to max (line_count - 1 = 4).
        assert_eq!(state.scroll_offset, 4);
    }
}
