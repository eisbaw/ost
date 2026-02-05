---
name: kitty-tmux-control
description: Control graphical terminals via tmux for testing TUI applications. Use when you need to start a kitty window, run commands in it, capture screen output, or take rapid snapshots of terminal state. Useful for debugging TUI apps, observing startup sequences, or testing interactive programs.
allowed-tools: Bash
---

# Kitty + Tmux Control

This skill enables control of graphical terminal windows through tmux, allowing you to run interactive programs and capture their output without direct TTY access.

## Starting a Kitty Window with Tmux Session

```bash
# Start kitty with a new tmux session (runs in background)
# Use -o for geometry: initial_window_width/height in cells (append 'c') or pixels
kitty -o "initial_window_width=120c" -o "initial_window_height=40c" sh -c "tmux new-session -s SESSION_NAME" &
```

Replace `SESSION_NAME` with a descriptive name (e.g., `gacia-test`, `app-debug`).

## Checking Tmux Sessions

```bash
# List all tmux sessions
tmux list-sessions
```

## Sending Commands to Tmux Session

```bash
# Send a command and press Enter
tmux send-keys -t SESSION_NAME "your command here" Enter

# Send Ctrl+C to interrupt
tmux send-keys -t SESSION_NAME C-c

# Send other control sequences
tmux send-keys -t SESSION_NAME C-d  # Ctrl+D (EOF)
tmux send-keys -t SESSION_NAME C-z  # Ctrl+Z (suspend)
```

## Capturing Screen Output

```bash
# Capture current pane contents
tmux capture-pane -t SESSION_NAME -p

# Capture with line numbers (useful for debugging)
tmux capture-pane -t SESSION_NAME -p -J

# Capture only last N lines
tmux capture-pane -t SESSION_NAME -p | tail -20
```

## Rapid Snapshots (for TUI Debugging)

When debugging TUI applications or capturing startup sequences, take multiple snapshots in rapid succession:

```bash
# Take 20 snapshots at 0.1 second intervals
for i in $(seq 1 20); do
  echo "=== SNAPSHOT $i ($(date +%H:%M:%S.%3N)) ==="
  tmux capture-pane -t SESSION_NAME -p
  echo ""
  sleep 0.1
done
```

Adjust the count and sleep interval as needed:
- `0.05` for very fast transitions
- `0.1` for normal startup sequences
- `0.5` for slower state changes

## Running Commands via nix-shell

If the target application requires nix-shell:

```bash
# Run command inside nix-shell
tmux send-keys -t SESSION_NAME "nix-shell --run 'your-command'" Enter
```

## Complete Workflow Example

```bash
# 1. Start kitty with tmux
kitty -o "initial_window_width=120c" -o "initial_window_height=40c" sh -c "tmux new-session -s myapp" &

# 2. Wait for tmux to initialize
sleep 2

# 3. Verify session exists
tmux list-sessions

# 4. Run application
tmux send-keys -t myapp "nix-shell --run 'just run'" Enter

# 5. Capture rapid snapshots during startup
for i in $(seq 1 20); do
  echo "=== SNAPSHOT $i ==="
  tmux capture-pane -t myapp -p
  sleep 0.1
done

# 6. Stop application
tmux send-keys -t myapp C-c

# 7. Check final state
tmux capture-pane -t myapp -p | tail -30

# 8. Kill session when done
tmux kill-session -t myapp
```

## Troubleshooting

**"session not found" error**: The kitty/tmux may not have started yet. Add `sleep 2` after starting kitty.

**Empty capture output**: The pane may not have rendered yet. Try capturing after a short delay.

**Command not in PATH**: The tmux session runs in a fresh shell. Use `nix-shell --run` or source appropriate environment files.

**Capturing TUI panels**: TUI frameworks like ratatui use ANSI escape codes. The captured text will show the rendered layout with box-drawing characters.
