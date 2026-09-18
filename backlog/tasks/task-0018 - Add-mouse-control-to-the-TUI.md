---
id: TASK-0018
title: Add mouse control to the TUI
status: Done
assignee:
  - '@codex'
created_date: '2026-09-17 15:42'
updated_date: '2026-09-17 16:04'
labels:
  - tui
  - ux
dependencies: []
references:
  - src/tui/app.rs
  - src/tui/ui.rs
  - src/tui/sidebar.rs
  - src/tui/messages.rs
  - /home/victor/Github/slk/internal/ui/reducer_mouse.go
  - /home/victor/Github/waha-tui/src/views/ChatListManager.ts
documentation:
  - README.md
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Make OST usable with mouse clicks and wheel scrolling, following the interaction patterns in the local slk and waha-tui clients. Support the existing Teams, chats, messages, compose, search, and help views while retaining keyboard navigation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Mouse capture is enabled during the TUI session and disabled on normal exit, error, and panic; keyboard controls remain usable.
- [x] #2 Left-clicking a visible sidebar item expands or collapses a team or opens the intended channel or chat, including after scrolling, resizing, or list updates; section headers and blank rows do not activate conversations.
- [x] #3 The mouse wheel scrolls the sidebar, messages, search results, or debug log under the pointer with bounded offsets and without unexpectedly switching conversations or moving keyboard focus.
- [x] #4 Left-clicking messages selects the displayed message and supports the existing thread toggle; clicking compose focuses it and positions the text cursor correctly for visible text, including Unicode and horizontally scrolled input.
- [x] #5 Search results can be selected and opened by clicking, help can be opened and dismissed by clicking, and overlays consume mouse input so underlying controls are not activated.
- [x] #6 README and in-app help describe mouse controls, and automated interaction tests verify coordinate mapping, scrolling, resize, empty content, overlay isolation, and keyboard compatibility.
- [x] #7 The project compiles with the checked-in SDP dictionary; resolve the pre-existing hard-coded array-length mismatch without changing the dictionary bytes or compression behavior.
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 Run cargo test and required formatting and lint checks; record any pre-existing or environment blockers accurately.
- [x] #2 Review the diff and verify terminal mouse cleanup and interaction behavior with offline fixtures.
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add mouse capture to the existing terminal lifecycle, including cleanup on errors and panics, and dispatch mouse events alongside keyboard events.
2. Share the rendered pane rectangles and visible row/message mappings with mouse handling. Keep mappings correct after resize, team expansion, list refresh, wrapped messages, and debug-pane toggles.
3. Implement left-click pane focus, sidebar activation, message selection and thread toggles, and compose cursor placement using display widths and horizontal scroll position. Reuse existing navigation and backend actions.
4. Add independent, bounded wheel scrolling (three lines per notch) to the pane under the pointer. Keep keyboard navigation able to reveal its selected item without wheel-induced snap-back.
5. Route mouse input through the topmost overlay: clickable and scrollable search results, clickable help entry and dismissal, and no click-through to underlying panes. Search clicks on chats/channels should actually load their messages.
6. Add offline interaction tests using rendered terminal buffers and an inert backend command channel. Cover scrolled/reordered lists, wrapped and tall messages, Unicode compose text, small/resized terminals, empty/loading content, overlays, and keyboard navigation. Update README/help and run cargo test, cargo fmt -- --check, cargo clippy --all-targets --all-features, and git diff --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Inspected the current OST event loop, renderer, sidebar/message scroll calculations, compose editor, search overlay, terminal cleanup, and task TASK-0013. Compared slk mouse routing and waha-tui chat click mapping. OST currently handles only Event::Key, and message/sidebar scrolling is computed from selection during rendering.
The Backlog CLI was absent from PATH; bunx --yes backlog.md provides version 1.52.0 without repository dependency changes. Cargo, rustfmt, and clippy are available.
Implementation plan is ready for the approval required by AGENTS.md line 368; application code has not been changed.

User approved the implementation plan. Starting mouse dispatch, render-derived hit regions, and independent viewport scrolling.

The initial cargo test --locked failed before mouse changes were compiled: sdp_compress.rs declares a 20623-byte dictionary but the checked-in file is 20476 bytes. Added the minimal build prerequisite to AC: accept the existing bytes as a slice; preserve dictionary content and compression logic.

Implemented hit regions recorded from the rendered widgets, independent bounded wheel scrolling, sidebar activation, message/thread selection, compose/search cursor positioning, and overlay isolation. Ignored motion/release events are filtered before redraw.
The original 91 tests passed after the dictionary declaration fix. New offline tests exercise visible rows, reorder, wrapping, Unicode, small terminals, overlays, and keyboard behavior. Default-feature Clippy passes with existing warnings; all-features Clippy is blocked by missing ALSA development files (alsa.pc).
Added a PTY fixture and script to verify real SGR mouse reports and terminal restoration on normal exit, error, and panic.

Validation complete: cargo test --locked passed 108 tests (17 new interaction tests); the PTY fixture is intentionally ignored in the ordinary suite and passed separately in normal, error, and panic modes via python3 tests/tui_mouse_pty.py. The PTY checks actual SGR click/wheel parsing, all mouse capture modes being disabled, alternate screen exit, and restoration of terminal attributes.
Cargo fmt -- --check, cargo clippy --locked --all-targets, and git diff --check passed. Clippy reports existing repository warnings; the new mouse, text-input and test modules add none. The required all-features attempt remains environment-blocked by missing alsa.pc; audio/video behavior was not tested.
Self-review confirmed current-frame targets after resize/backend responses, no network side effects in fixtures, and unchanged SDP dictionary bytes.

cargo build --locked also passed; the updated executable is target/debug/teams-cli.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Add mouse navigation and scrolling to the OST TUI. Users can click teams, channels, chats, messages, compose text, search results, and help; the wheel scrolls the pane under the pointer without changing keyboard focus. Rendered hit regions track scrolling, wrapping, resizing, and refreshed data, and overlays prevent click-through.

Shared text layout handles Unicode graphemes and horizontal scrolling. Mouse capture is released on normal exit, errors, and panics, and unused motion events are filtered before redraw. README and in-app help document the controls.

A pre-existing build error declared the embedded SDP dictionary as 20623 bytes although the committed file is 20476 bytes. Use a byte slice without changing dictionary bytes or compression logic.

Validation: 108 unit tests passed, including 17 new offline interaction tests; the separate PTY fixture passed normal/error/panic cases. Formatting, default-feature Clippy, and diff checks passed. All-features Clippy could not complete because this environment lacks ALSA development files (alsa.pc); optional audio/video features remain unverified.
<!-- SECTION:FINAL_SUMMARY:END -->
