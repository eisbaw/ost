---
name: tui-ux-tour
description: Run a UX walking tour of the Teams TUI application via kitty+tmux. Launches the TUI, takes rapid snapshots during startup, navigates all major UI areas (sidebar, messages, compose, help, debug log, search), captures screenshots at each step, then reports all glitches and UX bugs found. Creates a backlog task for each issue. Use this as a quality assurance check after TUI changes.
allowed-tools: Bash, Read, Glob, Grep, Write
---

# TUI UX Walking Tour

Automated UX quality assurance check for the Teams TUI. This skill launches the TUI in a tmux session inside kitty (with explicit 120x40 cell geometry for reproducibility), walks through all major UI areas, captures snapshots, identifies glitches and UX bugs, and creates backlog tasks for each issue found.

## Related

- **Skill: `kitty-tmux-control`** -- Provides the underlying kitty/tmux terminal control primitives used by this skill (session creation, key sending, pane capture, rapid snapshots).
- **Agent: `ratatui-tui-expert`** -- Use this agent when a bug found during the tour needs to be investigated or fixed in the ratatui rendering code. It has access to the TUI spec (`docs/specs/tui.txt`), ratatui source reference, and this skill via its `skills: kitty-tmux-control` config.

## Prerequisites

- The project must be built first (`cargo build` or `just build`)
- kitty and tmux must be available
- A valid login session must exist (the TUI connects to Teams APIs)

## Session Name

Always use `tui-ux-tour` as the tmux session name. Kill any existing session before starting.

## Procedure

Follow these steps in order. Do NOT send any chat messages during the tour.

### Phase 1: Build and Launch

```bash
# Kill any previous tour session
tmux kill-session -t tui-ux-tour 2>/dev/null
sleep 0.2

# Build
nix-shell --run "cargo build" 2>&1 | tail -3

# Start kitty with tmux (120x40 cell geometry for reproducible captures)
kitty -o "initial_window_width=120c" -o "initial_window_height=40c" sh -c "tmux new-session -s tui-ux-tour" &
sleep 2

# Verify session
tmux list-sessions | grep tui-ux-tour
```

### Phase 2: Startup Snapshots (Critical)

Launch the TUI and immediately take rapid snapshots to catch startup glitches:

```bash
# Launch TUI
tmux send-keys -t tui-ux-tour "nix-shell --run './target/debug/teams-cli tui'" Enter

# Take 30 snapshots at 100ms intervals (covers first 3 seconds)
for i in $(seq 1 30); do
  echo "=== SNAPSHOT $i ($(date +%H:%M:%S.%3N)) ==="
  tmux capture-pane -t tui-ux-tour -p
  echo ""
  sleep 0.1
done
```

Save the output to a file for analysis. Check for:
- **Layout glitches**: Partial renders, misaligned borders, overlapping text
- **Truncated text**: Title bar, status bar, sidebar entries cut off
- **Contradictory indicators**: Status bar vs title bar disagreements
- **Flicker**: Different content between consecutive snapshots that shouldn't change
- **Missing content**: Empty areas that should have placeholder text

### Phase 3: Wait for Full Load

```bash
# Wait for sidebar to populate (typically 5-10 seconds after launch)
sleep 8
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Sidebar shows teams and channels (not still "Loading...")
- User name appears in title bar
- Connection status shows "Connected"

### Phase 4: Navigate Sidebar

Test sidebar navigation:

```bash
# Navigate down through sidebar items
tmux send-keys -t tui-ux-tour Down Down Down Down
sleep 0.5
tmux capture-pane -t tui-ux-tour -p

# Select a channel (Enter)
tmux send-keys -t tui-ux-tour Enter
sleep 2
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Cursor indicator (`>`) moves correctly
- Selected channel loads messages in the right pane
- Header updates to show channel name
- Compose box placeholder updates to show channel name

### Phase 5: Team Collapse/Expand

```bash
# Navigate to a team header and toggle collapse
# (team headers show triangle indicators)
tmux send-keys -t tui-ux-tour Up Up Up  # Navigate to team header
sleep 0.3
tmux send-keys -t tui-ux-tour Enter     # Toggle collapse
sleep 0.5
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Team toggles between expanded (down-triangle) and collapsed (right-triangle)
- Child channels appear/disappear correctly
- No layout shift or flicker

### Phase 6: Focus Cycling (Tab)

```bash
# Tab through all panes: sidebar -> messages -> compose -> sidebar
tmux send-keys -t tui-ux-tour Tab
sleep 0.3
tmux capture-pane -t tui-ux-tour -p  # Should show messages focused

tmux send-keys -t tui-ux-tour Tab
sleep 0.3
tmux capture-pane -t tui-ux-tour -p  # Should show compose focused

tmux send-keys -t tui-ux-tour Tab
sleep 0.3
tmux capture-pane -t tui-ux-tour -p  # Should show sidebar focused again
```

Check that:
- Focused pane has double border (heavy lines)
- Unfocused panes have single border (thin lines)
- Status bar shows current focus pane name
- No visual artifacts during transition

### Phase 7: Message Scrolling

```bash
# Focus messages pane and scroll
tmux send-keys -t tui-ux-tour Tab  # to messages
sleep 0.3
tmux send-keys -t tui-ux-tour Up Up Up Up Up
sleep 0.5
tmux capture-pane -t tui-ux-tour -p

tmux send-keys -t tui-ux-tour Down Down Down Down Down
sleep 0.5
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Messages scroll smoothly
- Scroll indicators (arrows) appear/disappear correctly
- No overlapping or clipped message content

### Phase 8: Help Overlay

```bash
tmux send-keys -t tui-ux-tour Tab  # back to sidebar
sleep 0.3
tmux send-keys -t tui-ux-tour '?'
sleep 0.5
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Help overlay renders over the main UI
- All keybinding sections are visible
- Border/title of overlay is correct
- Close instruction is visible

```bash
# Close help
tmux send-keys -t tui-ux-tour Escape
sleep 0.3
tmux capture-pane -t tui-ux-tour -p
```

Check that UI restores cleanly after closing help.

### Phase 9: Debug Log

```bash
tmux send-keys -t tui-ux-tour C-d
sleep 0.5
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Debug log pane appears at the bottom
- Log entries are visible and formatted
- Main UI shrinks to accommodate the log pane
- No overlapping content

```bash
# Close debug log
tmux send-keys -t tui-ux-tour C-d
sleep 0.3
tmux capture-pane -t tui-ux-tour -p
```

### Phase 10: Search Overlay

```bash
tmux send-keys -t tui-ux-tour C-k
sleep 0.5
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Search overlay appears at the top
- Input field is visible and focused
- Close instruction is visible

```bash
# Close search
tmux send-keys -t tui-ux-tour Escape
sleep 0.3
tmux capture-pane -t tui-ux-tour -p
```

### Phase 11: Chat Navigation

```bash
# Navigate to a chat in the sidebar
# First go to sidebar
tmux send-keys -t tui-ux-tour Tab  # ensure sidebar focus
sleep 0.3
# Navigate to CHATS section and select one
for i in $(seq 1 20); do tmux send-keys -t tui-ux-tour Down; done
sleep 0.3
tmux send-keys -t tui-ux-tour Enter
sleep 2
tmux capture-pane -t tui-ux-tour -p
```

Check that:
- Chat messages load in the messages pane
- Header shows chat name
- Compose box placeholder shows chat name
- Messages have sender, timestamp, and content

### Phase 12: Cleanup

```bash
# Quit TUI
tmux send-keys -t tui-ux-tour q
sleep 1

# Verify it exited
tmux capture-pane -t tui-ux-tour -p

# Kill session
tmux kill-session -t tui-ux-tour
```

## What to Look For (Bug Categories)

When analyzing snapshots, classify issues into these categories:

### Rendering Bugs
- Text truncation (characters cut off at pane edges)
- Border misalignment (box-drawing characters don't connect)
- Overlapping text from adjacent panes
- Partial renders (incomplete frame captured)

### State Inconsistencies
- Contradictory indicators (e.g., "online" in one place, "Connecting" in another)
- Stale content (header says one thing, content shows another)
- Wrong placeholder text for the current state

### Navigation Bugs
- Focus indicator not moving correctly
- Selected item not highlighted
- Selection not triggering content load
- Keyboard shortcuts not working

### Layout Bugs
- Panes not resizing correctly when overlays open/close
- Content not reflowing after state changes
- Scroll position lost after pane switch

### UX Issues
- Ambiguous empty states (is it loading or empty?)
- Missing loading indicators
- Missing feedback for user actions
- Confusing or contradictory text

## Reporting

After the tour, summarize findings as a numbered list with:
1. **Bug title** (concise)
2. **Phase** where it was found
3. **Category** (from above)
4. **Evidence** (which snapshot, what was shown)
5. **Severity** (high/medium/low)

Then create a separate backlog task for each bug found using the Backlog.md MCP tools:
- Title: concise bug description
- Description: detailed explanation with reproduction context
- Labels: `bug`, `tui`, and optionally `ux`
- Priority: based on severity
- Acceptance criteria: what "fixed" looks like
