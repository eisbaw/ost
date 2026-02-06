---
id: TASK-0013
title: 'Audit compose toolbar: remove or implement non-functional controls'
status: To Do
assignee: []
created_date: '2026-02-06 00:09'
labels:
  - bug
  - tui
  - ux
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The compose toolbar displays formatting and action icons (bold, italic, underline, strikethrough, link, attach, emoji, camera, mic, send) but it is unclear which of these are functional. Likely none of them are wired up -- displaying non-functional controls misleads the user into thinking the TUI supports rich text formatting, file attachments, voice recording, etc.

Audit each toolbar item:
1. Test which controls actually work by sending messages to 48:notes
2. Remove controls that have no backend implementation
3. Keep only controls that are functional (likely just the send arrow, if even that)
4. If plain text is all that's supported, simplify the toolbar to reflect that

The toolbar should only show controls that actually do something when activated.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every icon in the compose toolbar has a working action when activated, or is removed
- [ ] #2 Sending a plain text message to 48:notes via the compose box works end-to-end
- [ ] #3 No non-functional controls are displayed in the toolbar
- [ ] #4 The send button (or Enter key) successfully sends and the message appears in the channel
<!-- AC:END -->
