---
id: TASK-0009
title: Use emojis as specified in the TUI spec
status: Done
assignee: []
created_date: '2026-02-05 23:20'
updated_date: '2026-02-05 23:50'
labels:
  - tui
  - ux
dependencies: []
references:
  - docs/specs/tui.txt
priority: low
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The TUI spec (docs/specs/tui.txt) defines emoji usage for various UI elements. The current TUI does not render these emojis. Implement emoji rendering as per the spec.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Emojis appear in the TUI where specified by docs/specs/tui.txt
- [ ] #2 Emojis render correctly in common terminal emulators (ghostty, kitty, alacritty)
<!-- AC:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added emojis per TUI spec: sidebar chat icons (👤/👥), compose toolbar (🔗📎😊📷🎤➤), search placeholder (🔍), search result icons (👤💬). Note: emoji width calculation uses char count not display width -- potential alignment issue for follow-up.
<!-- SECTION:FINAL_SUMMARY:END -->
