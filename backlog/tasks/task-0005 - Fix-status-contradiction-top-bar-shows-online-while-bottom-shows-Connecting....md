---
id: TASK-0005
title: >-
  Fix status contradiction: top bar shows 'online' while bottom shows
  'Connecting...'
status: Done
assignee: []
created_date: '2026-02-05 23:11'
updated_date: '2026-02-05 23:50'
labels:
  - bug
  - tui
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
During TUI startup, the top-right of the title bar displays `o online` while the bottom status bar simultaneously shows `o Connecting...`. These two indicators contradict each other for approximately 1.6 seconds until the connection completes. The top bar should not claim 'online' while still connecting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Top bar and bottom status bar show consistent connection state during startup
- [ ] #2 While connecting, neither indicator claims 'online'
- [ ] #3 Both indicators transition to 'online'/'Connected' at the same time
<!-- AC:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed status contradiction in ui.rs render_header(). The top bar now uses app.connection_state when app.is_online is false, showing the actual connection state (e.g. "Connecting...") instead of always saying "online". Both bars now show consistent state.
<!-- SECTION:FINAL_SUMMARY:END -->
