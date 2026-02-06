---
id: TASK-0014
title: Per-user message colors via username hash and right-align own messages
status: Done
assignee:
  - '@claude'
created_date: '2026-02-06 00:12'
updated_date: '2026-02-06 00:35'
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
- [x] #1 Each sender's name is rendered in a unique color derived from hashing their username to a HSV hue (0-359 degrees)
- [x] #2 The same username always produces the same color across sessions
- [x] #3 Messages from the current user (app.user_name) are right-aligned in the messages pane
- [x] #4 Messages from other users remain left-aligned
- [x] #5 The color derivation uses: hash username -> truncate to u8 -> scale to 0..359 HSV hue
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add username-to-HSV-hue helper: hash username bytes, truncate to u8, scale to 0..359, convert HSV(hue,0.7,0.9) to RGB
2. Thread user_name from render() -> build_message_lines() -> render_message_card()
3. In render_message_card: use hashed color for sender name
4. In render_message_card: right-align own messages (right-pad prefix instead of left-pad)
5. Build and test
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented per-user sender colors via username_to_color() which hashes username bytes to u8, scales to 0-359 HSV hue, converts HSV(h, 0.7, 0.9) to RGB. Own messages right-aligned using 75% effective width with left margin. Own messages get subtle blue-tinted background.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added per-user message colors and right-alignment for own messages in the TUI.\n\nChanges in src/tui/messages.rs:\n- Added username_to_color(): hashes username bytes to u8, scales to 0-359 HSV hue, converts HSV(h,0.7,0.9) to RGB\n- Added hsv_to_rgb() helper for the color conversion\n- Sender names now rendered in their unique hashed color instead of plain white\n- Own messages (matching app.user_name) right-aligned using 75% effective width with left margin\n- Own messages get a subtle blue-tinted background to distinguish from others\n- Threaded user_name parameter through render() -> build_message_lines() -> render_message_card()\n\nChanges in src/tui/ui.rs:\n- Pass &app.user_name to messages::render()\n\nVerified: builds clean, 91 tests pass, visually confirmed in TUI tour.
<!-- SECTION:FINAL_SUMMARY:END -->
