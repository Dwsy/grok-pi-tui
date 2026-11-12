---
id: 20261111-goal-workspace-ui
title: Native Goal Workspace UI redesign
status: implemented-static-unverified
created: 2026-11-11
category: pager
---

# Goal Workspace redesign

## Symptoms
The previous Goal overlay shrank to content height, truncated the objective
to a single border row, rendered only the last event, and did not scroll.

## Scope / ownership
- Pager remains the sole TUI; Pi and the adapter remain unchanged.
- The original goal_detail renderer is the fallback for narrow terminals.
- The new goal_workspace.rs uses GoalDisplayState, TodoPane, Theme, Buffer,
  native GoalUpdated notifications and AgentView input.
- The bounded timeline stores up to 256 distinct GoalUpdated events per
  live AgentView and goal ID; identical state refreshes are deduplicated.
- No fake events, no new remote commands or new agent core.
- The user requested no Rust or Cargo execution. Static review only.

## UI
- Large stable workspace; objective wraps over multiple rows.
- Overview: checklist, 6 recent events, worker and verifier information.
- Timeline: newest-first events with local timestamp and details.
- Diagnostics: full objective, budget, token baseline, classifier evidence.
- Controls: 1/2/3 tabs; Tab/Shift+Tab; arrows/h/j/k/l;
  PgUp/PgDn, Home/End, clickable tabs, mouse wheel, Esc/g/q to close.

## Boundaries
- This timeline retains only events received by the current live Pager.
  Historic events from a previous process cannot be reconstructed from
  the single last_event field; persisted/replayable timelines require a
  separate session-owned history protocol.
- grok-pi's token budget is informational; it is not a hard-stop guard.
- Rust compile/runtime checks and terminal layout review are still pending.
