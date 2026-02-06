---
id: TASK-0015
title: Desktop notifications for messages mentioning me
status: To Do
assignee: []
created_date: '2026-02-06 00:13'
labels:
  - tui
  - feature
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
When a new message arrives that mentions the current user (by @name or @mention), show a desktop notification via the system notification daemon (e.g. notify-send on Linux). This lets the user keep the TUI in the background and still be alerted when someone needs their attention.

Should detect mentions in incoming messages from the trouter/websocket event stream and trigger a desktop notification with the sender name, channel/chat name, and a snippet of the message content.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A desktop notification appears when a new message mentions the current user
- [ ] #2 Notification shows sender name, channel/chat name, and message snippet
- [ ] #3 Notifications only fire for messages from others, not the user's own messages
- [ ] #4 Works with notify-send or equivalent Linux notification mechanism
<!-- AC:END -->
