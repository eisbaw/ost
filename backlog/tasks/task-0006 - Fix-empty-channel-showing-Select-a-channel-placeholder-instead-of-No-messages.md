---
id: TASK-0006
title: >-
  Fix empty channel showing 'Select a channel' placeholder instead of 'No
  messages'
status: To Do
assignee: []
created_date: '2026-02-05 23:11'
labels:
  - bug
  - tui
  - ux
dependencies: []
priority: low
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
When selecting a channel that has no messages (e.g. 'ProjectX > #Planning'), the message area shows the generic unselected-state placeholder 'Select a channel or chat to view messages' even though the header and compose box both indicate the channel is selected. This makes it look broken rather than simply empty. An empty but selected channel should show a distinct 'No messages yet' state.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Selected channel with no messages shows 'No messages yet' or similar distinct empty state
- [ ] #2 The 'Select a channel or chat' placeholder only appears when no channel is selected
- [ ] #3 Compose box correctly reflects the selected channel name
<!-- AC:END -->
