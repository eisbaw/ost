---
id: TASK-0008
title: Replace ASCII art message borders with colorized backgrounds
status: To Do
assignee: []
created_date: '2026-02-05 23:20'
labels:
  - tui
  - ux
dependencies: []
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Messages currently use ASCII box-drawing borders around each message, consuming too much vertical and horizontal space. Replace with alternating or colored background regions to visually separate messages without wasting lines on borders. This will increase message density and readability.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Messages are visually separated using background color instead of border characters
- [ ] #2 No box-drawing borders around individual messages
- [ ] #3 Sender name and timestamp are still clearly distinguishable from message body
- [ ] #4 Message density is noticeably higher than before
<!-- AC:END -->
