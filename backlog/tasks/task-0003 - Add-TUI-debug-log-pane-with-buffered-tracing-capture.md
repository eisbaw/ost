---
id: TASK-0003
title: Add TUI debug log pane with buffered tracing capture
status: Done
assignee:
  - '@claude'
created_date: '2026-02-05 22:32'
updated_date: '2026-02-05 22:59'
labels:
  - tui
  - logging
dependencies: []
references:
  - src/main.rs
  - src/tui/app.rs
  - src/tui/ui.rs
  - src/tui/mod.rs
  - src/tui/help.rs
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Tracing output (info/debug/warn from token refresh, API calls) writes to stderr during TUI startup and runtime, corrupting the ratatui alternate screen display. Need to redirect tracing to a shared ring buffer in TUI mode (keep stderr for CLI), and add a toggleable debug log pane to the TUI.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 TUI mode redirects tracing output to an in-memory ring buffer instead of stderr
- [x] #2 CLI commands still log to stderr as before
- [x] #3 New log_capture module with LogBuffer (Arc<Mutex<VecDeque<String>>>) and MakeWriter impl
- [x] #4 New debug_log module with DebugLogState, refresh/toggle/scroll, and level-colored rendering
- [x] #5 Debug log pane toggles with Ctrl+D as a bottom split pane (30% height, not modal overlay)
- [x] #6 Log buffer is drained every event loop iteration regardless of pane visibility
- [x] #7 main.rs conditionally initializes tracing subscriber (buffer for TUI, stderr for CLI)
- [x] #8 Ctrl+D shortcut added to help popup
- [x] #9 cargo build succeeds, cargo test passes, no regressions
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Create `src/tui/log_capture.rs` — LogBuffer ring buffer + MakeWriter impl + BufferWriter
2. Create `src/tui/debug_log.rs` — DebugLogState with refresh/toggle/scroll + level-colored render
3. Update `src/tui/mod.rs` — add mod declarations + pub use LogBuffer
4. Update `src/tui/app.rs` — add DebugLogState to App, change run() to accept LogBuffer, add Ctrl+D keybinding, call refresh() in event loop
5. Update `src/tui/ui.rs` — conditional layout split (70/30) when debug log visible
6. Update `src/main.rs` — conditional tracing init (buffer for TUI, stderr for CLI)
7. Update `src/tui/help.rs` — add Ctrl+D shortcut to MISC category
8. cargo fmt + cargo build + cargo test
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Technical Design

### Log Capture Architecture
- `LogBuffer`: `Arc<Mutex<VecDeque<String>>>` with capacity 500. Implements `Clone`.
- `BufferWriter`: implements `std::io::Write`. Buffers bytes in a `Vec<u8>`, flushes complete line to ring buffer on `Drop` (tracing fmt layer calls write() multiple times per event, then drops the writer).
- `LogBuffer` implements `tracing_subscriber::fmt::MakeWriter` trait — returns fresh `BufferWriter` per log event.

### Conditional Tracing Init (main.rs)
- Must check `matches!(cli.command, Commands::Tui)` BEFORE calling `.init()` — global subscriber can only be set once.
- `EnvFilter` doesn't impl `Clone`, so construct it in each branch separately.
- TUI branch: `fmt::layer().with_target(false).with_ansi(false).with_writer(log_buffer.clone())`
- CLI branch: `fmt::layer().with_target(false)` (current stderr behavior)
- The CLI command dispatch moves into the else branch.

### Debug Log Pane Layout
When visible (Ctrl+D toggle), main_area splits:
```
[content_area (Percentage(70)), debug_log_area (Percentage(30))]
```
Sidebar+messages+compose render into content_area unchanged. debug_log::render() fills debug_log_area (full width, bordered block).

### Level-Based Coloring
With `.with_ansi(false)`, fmt layer outputs plain text like:
```
2024-05-15T10:30:00Z  INFO Refreshing AAD token...
2024-05-15T10:30:01Z  WARN Skype token exchange failed: ...
```
Parse level prefix in first ~40 chars: ERROR=Red, WARN=Yellow, INFO=Green, DEBUG=DarkGray.

### Design Decisions
- Split pane (not overlay): can watch logs while using the app, unlike modal help/search
- Always drain buffer every event loop iteration: prevents unbounded mutex growth when pane is hidden
- `.with_ansi(false)`: avoids ANSI escape codes in buffer that would corrupt ratatui rendering
- `Ctrl+D`: unused key, memorable (D=Debug), safe in crossterm raw mode
- `std::sync::Mutex` (not tokio): correct choice since critical sections are sub-microsecond, never held across .await

### Verified: No eprintln!/println! in TUI path
- TUI startup: `TeamsClient::new()` → `oauth::refresh()` — only `tracing::` macros
- `eprintln!` calls in oauth.rs are only in CLI `login()` function
- `println!` calls are only in CLI command handlers

## Per-File Modification Details

### src/tui/log_capture.rs (new)
- `LogBuffer::new(capacity: usize)` constructor
- `LogBuffer::drain() -> Vec<String>` — drain all lines from ring buffer
- `BufferWriter` — internal, buffers bytes in Vec<u8>, flushes to ring buffer on Drop
- `impl MakeWriter for LogBuffer` — returns fresh BufferWriter per log event

### src/tui/debug_log.rs (new)
- `DebugLogState::new(buffer: LogBuffer)` constructor
- `refresh()` — drain ring buffer into accumulated `Vec<String>` (capped at 1000 lines)
- `toggle()` — flip visibility, auto-scroll to bottom on open
- `scroll_up(n)` / `scroll_down(n)` — manual scrolling
- `render(area, frame, state)` — bordered block, full-width, level-colored lines

### src/tui/app.rs
- Replace `App::default()` with `App::new(log_buffer: LogBuffer)` constructor
- `run()` signature becomes `pub async fn run(log_buffer: LogBuffer) -> Result<()>`
- `run_app()` signature becomes `async fn run_app(terminal, log_buffer) -> Result<()>`
- Event loop: `app.debug_log.refresh()` before `terminal.draw()`
- Global key: `Ctrl+D` → `self.debug_log.toggle()` (alongside existing `Ctrl+K`)
- When debug_log visible, route PgUp/PgDn to debug_log scroll

### src/main.rs
- Two branches: `if matches!(cli.command, Commands::Tui)` vs else
- EnvFilter constructed in each branch (not Clone-able)
- TUI branch creates LogBuffer, inits subscriber with `.with_ansi(false).with_writer(buf.clone())`, calls `tui::run(buf)`
- CLI branch keeps current init + command dispatch

## No New Dependencies Needed
tracing-subscriber already has `fmt` + `env-filter` features. `MakeWriter` trait is in `tracing_subscriber::fmt::writer`.

## Verification Steps
1. `cargo fmt && cargo build` — must compile clean
2. `cargo test` — all tests pass
3. Manual: `cargo run -- tui` — no log spew on startup, TUI renders clean
4. Manual: `Ctrl+D` toggles debug log pane, shows captured startup logs
5. Manual: `cargo run -- -v tui` — debug-level API call logs appear in pane
6. Manual: CLI commands (`cargo run -- whoami` etc.) still log to stderr as before
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Redirect tracing output to an in-memory ring buffer in TUI mode so log lines no longer corrupt the ratatui alternate screen. CLI commands continue logging to stderr unchanged.\n\nNew modules:\n- `src/tui/log_capture.rs`: LogBuffer (Arc<Mutex<VecDeque<String>>>) with MakeWriter impl for tracing-subscriber. Ring buffer capacity 500, poison-recovery on mutex.\n- `src/tui/debug_log.rs`: DebugLogState with accumulated line history (capped at 1000), toggle/scroll, level-based line coloring (ERROR=red, WARN=yellow, INFO=green, DEBUG=gray).\n\nModified:\n- `src/main.rs`: Conditional tracing init — TUI branch uses `.with_ansi(false).with_writer(log_buffer)`, CLI branch uses stderr fmt layer.\n- `src/tui/app.rs`: App::new(LogBuffer) replaces App::default(), Ctrl+D toggle, PgUp/PgDn scroll, buffer drained every event loop iteration.\n- `src/tui/ui.rs`: Conditional 70/30 vertical split when debug log visible.\n- `src/tui/help.rs`: Ctrl+D entry in MISC category.\n\nTests: 91 pass (8 new tests in log_capture + debug_log modules)."
<!-- SECTION:FINAL_SUMMARY:END -->
