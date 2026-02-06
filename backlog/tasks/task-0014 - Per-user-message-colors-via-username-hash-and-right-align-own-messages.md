---
id: TASK-0014
title: Per-user message colors via username hash and right-align own messages
status: To Do
assignee: []
created_date: '2026-02-06 00:12'
labels:
  - tui
  - ux
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Messages currently use near-identical dark backgrounds making it hard to visually distinguish who said what. Two changes needed:

1. **Per-user colors**: Each sender gets a unique color derived from their username. Hash the username (e.g. with a simple hash function), truncate to a single byte, and scale that byte to 0-359 degrees on the HSV color wheel. Use this hue for the sender name color (and optionally a subtle tinted background). This gives deterministic, reproducible colors per person across sessions.

2. **Right-align own messages**: Messages sent by the current user (app.user_name) should be right-aligned in the messages pane, similar to how iMessage/WhatsApp/Signal display conversations. All other users' messages remain left-aligned. This creates a clear visual distinction between "mine" and "theirs" without needing to read the sender name.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each sender's name is rendered in a unique color derived from hashing their username to a HSV hue (0-359 degrees)
- [ ] #2 The same username always produces the same color across sessions
- [ ] #3 Messages from the current user (app.user_name) are right-aligned in the messages pane
- [ ] #4 Messages from other users remain left-aligned
- [ ] #5 The color derivation uses: hash username -> truncate to u8 -> scale to 0..359 HSV hue
<!-- AC:END -->
