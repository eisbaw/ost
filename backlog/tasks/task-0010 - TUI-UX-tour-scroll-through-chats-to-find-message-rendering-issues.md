---
id: TASK-0010
title: 'TUI UX tour: scroll through chats to find message rendering issues'
status: To Do
assignee: []
created_date: '2026-02-05 23:23'
labels:
  - tui
  - ux
  - qa
dependencies: []
references:
  - docs/specs/tui.txt
  - .claude/skills/tui-ux-tour/SKILL.md
priority: medium
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Run a targeted TUI UX walking tour focused on message rendering quality. Open several chats and channels, scroll through message history, and look for rendering problems with rich content: markdown formatting (bold, italic, headers, lists), inline code, code blocks, tables, links, mentions, emojis, long URLs, and multi-line messages. Create a new backlog task for each rendering issue found.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 At least 5 different chats/channels scrolled through using the tui-ux-tour skill
- [ ] #2 Each rendering issue found is filed as a separate backlog task with screenshot evidence
- [ ] #3 Rich content types checked: markdown, inline code, code blocks, tables, links, mentions, emojis
- [ ] #4 Tour summary documents which content types render correctly and which do not
<!-- AC:END -->
