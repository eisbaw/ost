//! Mouse targets are recorded while drawing, in the same order as the widgets.

use std::ops::Range;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

pub const WHEEL_LINES: usize = 3;

/// Motion/release reports do not change this UI; filter them before drawing.
pub fn is_actionable(event: MouseEvent) -> bool {
    event.modifiers.is_empty()
        && matches!(
            event.kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Sidebar,
    SidebarItem(usize),
    Messages,
    Message(usize),
    Compose,
    ComposeCursor(usize),
    DebugLog,
    Help,
    Search,
    SearchCursor(usize),
    SearchResult(usize),
    DismissSearch,
}

#[derive(Default)]
pub struct HitMap {
    regions: Vec<(Rect, Target)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn add(&mut self, area: Rect, target: Target) {
        if !area.is_empty() {
            self.regions.push((area, target));
        }
    }

    pub fn at(&self, x: u16, y: u16) -> Option<Target> {
        self.regions
            .iter()
            .rev()
            .find(|(area, _)| area.contains(Position::new(x, y)))
            .map(|(_, target)| *target)
    }
}

/// A viewport can follow keyboard selection or scroll freely with the wheel.
pub struct Viewport {
    pub offset: usize,
    pub follow_selection: bool,
    height: usize,
    total: usize,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            offset: 0,
            follow_selection: true,
            height: 0,
            total: 0,
        }
    }
}

impl Viewport {
    pub fn prepare(&mut self, total: usize, height: usize, selected: Range<usize>) {
        self.total = total;
        self.height = height;
        if self.follow_selection && height > 0 {
            if selected.start < self.offset || selected.len() >= height {
                self.offset = selected.start;
            } else if selected.end > self.offset.saturating_add(height) {
                self.offset = selected.end.saturating_sub(height);
            }
        }
        self.offset = self.offset.min(total.saturating_sub(height));
    }

    pub fn scroll(&mut self, up: bool) {
        self.follow_selection = false;
        self.offset = if up {
            self.offset.saturating_sub(WHEEL_LINES)
        } else {
            self.offset
                .saturating_add(WHEEL_LINES)
                .min(self.total.saturating_sub(self.height))
        };
    }
}
