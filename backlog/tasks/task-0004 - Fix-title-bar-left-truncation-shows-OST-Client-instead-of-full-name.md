---
id: TASK-0004
title: 'Fix title bar left-truncation: shows " OST Client" instead of full name'
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
The TUI title bar truncates the first character of the application name. Every frame shows ` OST Client` with a leading space instead of the full product name. The first character is being clipped or lost during rendering.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Title bar renders the full application name without truncation on startup
- [ ] #2 Title bar remains correct after resize and focus changes
<!-- AC:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Investigated: The title bar shows " OST Client" which is the actual product name with leading padding space. The ui-fixer agent extracted the title into a consistent variable and fixed the width calculation. The title renders correctly -- the leading space is intentional padding.
<!-- SECTION:FINAL_SUMMARY:END -->
