//! Shared display columns for text rendering, cursor placement and mouse clicks.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct DisplayText {
    pub visible: String,
    pub cursor_offset: usize,
    /// Input character offset at each visible terminal cell (excluding the margin).
    pub positions: Vec<usize>,
}

pub fn display_text(input: &str, cursor_pos: usize, width: usize) -> DisplayText {
    // Match Ratatui's grapheme rendering: CJK, combining marks and joined emoji
    // must occupy the same cells in both the text and its mouse targets.
    let mut char_offset = 0;
    let mut cursor_column = 0;
    let mut column = 0;
    let tokens: Vec<_> = input
        .graphemes(true)
        .map(|grapheme| {
            let text = if grapheme == "\n" { " | " } else { grapheme };
            let cells = text.width();
            if char_offset < cursor_pos {
                let before_cursor: String =
                    grapheme.chars().take(cursor_pos - char_offset).collect();
                cursor_column = column
                    + if grapheme == "\n" {
                        cells
                    } else {
                        before_cursor.width()
                    };
            }
            let token = (text, char_offset, column, cells);
            char_offset += grapheme.chars().count();
            column += cells;
            token
        })
        .collect();
    let available = width.saturating_sub(1);
    // Start at a grapheme boundary and reserve a cell for the cursor at line end.
    let start = tokens
        .iter()
        .position(|(_, _, column, _)| cursor_column.saturating_sub(*column) < available)
        .unwrap_or(tokens.len());
    let start_column = tokens.get(start).map_or(column, |token| token.2);
    let mut visible = String::new();
    let mut positions = Vec::with_capacity(available);
    let mut end = tokens.get(start).map_or(char_offset, |token| token.1);
    for (text, position, _, cells) in tokens.iter().skip(start) {
        if positions.len() + cells > available {
            break;
        }
        visible.push_str(text);
        positions.resize(positions.len() + cells, *position);
        end = if *text == " | " {
            position + 1
        } else {
            position + text.chars().count()
        };
    }
    positions.resize(available, end);
    DisplayText {
        visible,
        cursor_offset: cursor_column
            .saturating_sub(start_column)
            .min(available.saturating_sub(1)),
        positions,
    }
}
