//! Help popup overlay: shows all keyboard shortcuts organized by category.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Popup dimensions.
const POPUP_WIDTH: u16 = 84;
const POPUP_HEIGHT: u16 = 30;

/// A shortcut entry: key binding and its description.
struct Shortcut {
    key: &'static str,
    desc: &'static str,
}

/// A category of shortcuts with a title.
struct Category {
    title: &'static str,
    shortcuts: &'static [Shortcut],
}

const NAVIGATION: Category = Category {
    title: "NAVIGATION",
    shortcuts: &[
        Shortcut {
            key: "Up/Down",
            desc: "Move within pane",
        },
        Shortcut {
            key: "Left/Right",
            desc: "Switch between panes",
        },
        Shortcut {
            key: "Tab",
            desc: "Cycle focus forward",
        },
        Shortcut {
            key: "Shift+Tab",
            desc: "Cycle focus backward",
        },
        Shortcut {
            key: "g g",
            desc: "Jump to top",
        },
        Shortcut {
            key: "G",
            desc: "Jump to bottom",
        },
        Shortcut {
            key: "/",
            desc: "Search in current view",
        },
        Shortcut {
            key: "Ctrl+K",
            desc: "Global search",
        },
    ],
};

const PANES: Category = Category {
    title: "PANES",
    shortcuts: &[
        Shortcut {
            key: "1",
            desc: "Teams/Channels pane",
        },
        Shortcut {
            key: "2",
            desc: "Chat/Messages pane",
        },
        Shortcut {
            key: "3",
            desc: "Compose pane",
        },
    ],
};

const VIEWS: Category = Category {
    title: "VIEWS",
    shortcuts: &[
        Shortcut {
            key: "F1",
            desc: "Activity",
        },
        Shortcut {
            key: "F2",
            desc: "Chats",
        },
        Shortcut {
            key: "F3",
            desc: "Teams",
        },
        Shortcut {
            key: "F4",
            desc: "Calendar",
        },
        Shortcut {
            key: "F5",
            desc: "Calls",
        },
    ],
};

const MESSAGING: Category = Category {
    title: "MESSAGING",
    shortcuts: &[
        Shortcut {
            key: "Enter",
            desc: "Send message / Open thread",
        },
        Shortcut {
            key: "Ctrl+Enter",
            desc: "New line in compose",
        },
        Shortcut {
            key: "Esc",
            desc: "Cancel / Close popup",
        },
        Shortcut {
            key: "Ctrl+U",
            desc: "Clear compose box",
        },
    ],
};

const ACTIONS: Category = Category {
    title: "ACTIONS",
    shortcuts: &[
        Shortcut {
            key: "r",
            desc: "Reply to message",
        },
        Shortcut {
            key: "e",
            desc: "Edit your message",
        },
        Shortcut {
            key: "d",
            desc: "Delete message (confirm)",
        },
        Shortcut {
            key: "+",
            desc: "Add reaction",
        },
        Shortcut {
            key: "@",
            desc: "Mention user",
        },
        Shortcut {
            key: "Ctrl+P",
            desc: "Attach file",
        },
    ],
};

const MISC: Category = Category {
    title: "MISC",
    shortcuts: &[
        Shortcut {
            key: "Ctrl+R",
            desc: "Refresh",
        },
        Shortcut {
            key: "Ctrl+N",
            desc: "New chat",
        },
        Shortcut {
            key: "Ctrl+T",
            desc: "New channel post",
        },
        Shortcut {
            key: "Ctrl+D",
            desc: "Toggle debug log",
        },
        Shortcut {
            key: "Ctrl+,",
            desc: "Settings",
        },
        Shortcut {
            key: "q",
            desc: "Quit (confirm)",
        },
        Shortcut {
            key: "?",
            desc: "Toggle this help",
        },
    ],
};

/// Render the help popup overlay centered on screen.
///
/// Clears the area behind the popup and draws a bordered box with all
/// keyboard shortcuts organized in a two-column layout.
pub fn render_help_popup(frame: &mut Frame) {
    let area = frame.area();

    // Calculate centered popup area, clamped to terminal size.
    let popup_w = POPUP_WIDTH.min(area.width.saturating_sub(2));
    let popup_h = POPUP_HEIGHT.min(area.height.saturating_sub(2));

    let popup_area = centered_rect(popup_w, popup_h, area);

    // Clear the background behind the popup.
    frame.render_widget(Clear, popup_area);

    // Outer block with title and footer.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Line::from(vec![
            Span::styled(
                " HELP ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("(? to close) ", Style::default().fg(Color::Gray)),
        ]))
        .title_bottom(Line::from(Span::styled(
            " Press any key to close ",
            Style::default().fg(Color::Gray),
        )));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Split inner area into two columns.
    let [left_col, right_col] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);

    // Left column: Navigation, Panes, Views
    let left_lines = build_column_lines(&[&NAVIGATION, &PANES, &VIEWS]);
    let left_para = Paragraph::new(left_lines);
    frame.render_widget(left_para, inset(left_col, 1, 1));

    // Right column: Messaging, Actions, Misc
    let right_lines = build_column_lines(&[&MESSAGING, &ACTIONS, &MISC]);
    let right_para = Paragraph::new(right_lines);
    frame.render_widget(right_para, inset(right_col, 1, 1));
}

/// Build the lines for one column of categories.
fn build_column_lines<'a>(categories: &[&Category]) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();

    for (cat_idx, cat) in categories.iter().enumerate() {
        if cat_idx > 0 {
            // Blank line between categories.
            lines.push(Line::from(""));
        }

        // Category title.
        lines.push(Line::from(Span::styled(
            cat.title,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));

        // Separator line under title.
        let sep_len = 36;
        let sep: String = "\u{2500}".repeat(sep_len);
        lines.push(Line::from(Span::styled(
            sep,
            Style::default().fg(Color::DarkGray),
        )));

        // Shortcut entries.
        for sc in cat.shortcuts.iter() {
            let key_width = 12;
            let key_display = format!("{:<width$}", sc.key, width = key_width);
            lines.push(Line::from(vec![
                Span::styled(key_display, Style::default().fg(Color::Yellow)),
                Span::styled(sc.desc, Style::default().fg(Color::Gray)),
            ]));
        }
    }

    lines
}

/// Return a centered sub-rect of the given size within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

/// Inset a rect by the given horizontal and vertical margins.
fn inset(area: Rect, h: u16, v: u16) -> Rect {
    Rect::new(
        area.x + h,
        area.y + v,
        area.width.saturating_sub(h * 2),
        area.height.saturating_sub(v * 2),
    )
}
