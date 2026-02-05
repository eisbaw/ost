---
id: TASK-0001
title: Implement TUI chat client
status: Done
assignee:
  - '@claude'
created_date: '2026-02-04 20:27'
updated_date: '2026-02-04 22:28'
labels:
  - tui
  - frontend
dependencies: []
references:
  - docs/specs/tui.txt
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Build a terminal UI chat client (Microsoft Teams style) based on the ASCII mockup specification. The TUI should support Teams/channels navigation, threaded messaging, and be fully keyboard-driven with self-documenting help.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Left pane displays Teams hierarchy and Chats list with unread badges
- [x] #2 Main pane shows threaded messages with reactions and reply counts
- [x] #3 Compose box with formatting toolbar
- [x] #4 Arrow key navigation between widgets with clear focus indication
- [x] #5 Help popup toggled with ? key showing all keyboard shortcuts
- [x] #6 Status bar shows connection state, current channel, and active pane
- [x] #7 Global search via Ctrl+K
<!-- AC:END -->
