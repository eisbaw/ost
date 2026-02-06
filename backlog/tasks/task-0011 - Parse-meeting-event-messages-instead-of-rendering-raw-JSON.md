---
id: TASK-0011
title: Parse meeting/event messages instead of rendering raw JSON
status: To Do
assignee: []
created_date: '2026-02-06 00:06'
labels:
  - bug
  - tui
  - api
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Meeting event messages from Teams API are rendered as raw JSON blobs like {"scopeId":"...","callId":"..."} in the messages pane. These should be parsed and displayed as human-readable meeting cards showing meeting title, time, and status. The "Play" suffix appearing on meeting titles (e.g. "ProjectX Qualification Activities and StatusPlay") should also be cleaned up or displayed as a proper action button label.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Meeting event messages display meeting title, time, and organizer instead of raw JSON
- [ ] #2 No raw JSON blobs visible in any channel's message list
- [ ] #3 Meeting title does not show a trailing 'Play' suffix as part of the title text
<!-- AC:END -->
