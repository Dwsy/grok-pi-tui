# Goal Session Ownership and Resume (2026-11-11)

## Model-initiated resume


The model can call `goal({ action: "resume" })` when the user explicitly asks to resume the current session's goal. It shares the `/goal resume` transition and journal, including a new execution ID for a cleared goal. Restart, interruption, and ordinary messages do not authorize automatic resumption. Completed goals cannot be resumed. The tool runs inside the current turn and does not start a second turn.

Status: implemented, TypeScript regression tests passing; Rust build/tests not executed by user request.

## Budget and authorization follow-up

This Pi goal path does not reliably count and enforce token usage. `/goal ... --budget` (including `--budget=...`) and `goal({ action: "create", token_budget })` are rejected instead of accepting an unenforced limit. A recovered legacy goal with a budget cannot be resumed; clear it and create a new unbudgeted goal if desired. No hard budget is promised.

The natural-language instructions on the `goal` tool's create and resume actions are model guidance, not a programmatic authorization check. The tool remains callable by the model; session recovery itself pauses active goals and does not start a turn.

## Bugs
- Process-temp PI_GROK_GOAL_CONTROL survives session switches inside one Pi process and incorrectly influences new/resumed sessions.
- No startup rehydration despite Pi already persisting pi-grok-goal/v1 custom entries in the session JSONL.
- /goal replaces active/paused/blocked, while the goal create action blocks only active.
- Cancellation (Escape/ACP abort) can leave active continuation or a cleared state that /goal resume refuses.

## Invariants
1. Pi SessionManager's persisted custom entries are the durable SSOT. The temp JSON control file is a strictly session-tagged bridge projection.
2. Never take a different session's status/objective, including delayed bridge events. The old custom entry format is adopted only from the current Pi SessionManager log (not the process-temp control file); fork cannot import untagged parent legacy events.
3. Resuming a session or restarting with an active persisted Goal **suspends** it until an explicit /goal resume, preventing unexpected background execution. This transition is itself persisted.
4. A user interruption pauses the current Goal and can be resumed; /goal cancel is an alias for pause. /goal clear hides it but retains a restorable snapshot as long as no newer goal replaces it.
5. Neither slash nor model tools silently replace an unfinished active/paused/blocked goal. Explicit clear first.
6. Preserve the existing Pager and Goal Workspace; the adapter stays headless.

## Non-goals
- No separate SQLite store, no artificial JSONL editing, no second agent core, no budget accounting rewrite.
- Session-local checkpoint events are intentionally retained in Pi's own append-only file; no automatic eager whole-catalog scan.
- User requested no Rust / Cargo execution. Tests may be authored but compilation will not run in this task.

## Static verification (2026-11-11)
- Bun: 6 lifecycle tests, 32 assertions passed. Scenarios cover session isolation, safe restart, active/paused/blocked replacement guards, aborted/cancel resume, clear/resume, and fork legacy migration.
- Pager unit test source added for late old-goal clear when a cleared goal is resumed as a new execution ID.
- Rust unit test source added for session-bound guard state and ACP cancel pause; **not executed**.
- JS source transpiled using bun build with all external imports marked external.
- git diff --check passed. Cargo/rustc/rustfmt intentionally not invoked.
- Known limitation: persisted timeline is Pi JSONL event history, while the current Goal Workspace still caches only live GoalUpdated events; historic UI timeline replay remains separate work.

- A forked child receives a session-tagged cleared tombstone even without a Goal so that future process restarts never migrate a copied legacy parent record.
- A cleared Goal restored via /goal resume receives a new execution ID to bypass the Pager stale-update tombstone; delayed clears from older IDs are discarded.

- GoalHost now checks sessionId before applying IPC snapshots to its in-memory continuation guard/timer, preventing a stale B control file from mutating A's host state. Added a static Rust regression test for this path (not executed).

- The verifier manifest and expected_modified set now explicitly allowlist only the two Goal ACP notification/test seams; verified set equality without weakening the verifier.

## Tree navigation synchronization

Pi's `appendEntry` emits `entry_appended` through RPC. Every `session_tree` transition now writes the selected branch control first and appends a `goal_tree_restored` snapshot, including a cleared snapshot when the branch has no goal. The adapter forwards that event's own session-scoped snapshot; Pager treats it as authoritative across branch changes while retaining ordinary late-clear and late-update guards. Restored active goals remain paused.

Validation: the goal extension and Rust TUI bridge Bun tests pass. Pager regression coverage was added for historical cleared goals and restoration past a clear tombstone. Rust files were parsed with rustfmt; Rust compilation and tests were not run as requested.

## Current-branch restore, model tools, and loop-guard journaling (2026-11-13)

`session_start` restores from `sessionManager.getBranch()` instead of the whole session tree, so restarting on a selected branch cannot adopt a goal from another branch. `session_tree` writes the newly selected branch control and appends a `goal_tree_restored` snapshot, and neither path adopts an unscoped legacy entry.

`goal({ action: "get" })` reports the current control state without side effects. `goal({ action: "update" })` no longer queues `nextTurn` reminders; the `context` hook strips goal reminders from the outgoing request and appends the current control state, so pause, clear, resume, and cancellation cannot leave an older "active" instruction in queued model context.

The adapter's continuation loop guard still writes the IPC control file, and now also invokes the hidden `/__pi_grok_goal_persist <goalId>` command so the block is appended to Pi's session log. The extension journals only when the branch's durable state for the same goal is `active`, which makes duplicate invocations idempotent; a mismatched goal ID is ignored. Lifecycle hooks (`session_start`, `session_tree`, `session_before_tree`, `session_before_switch`, `agent_settled`, `context`, `session_shutdown`) reconcile a missed command.

Validation: 19 Bun tests / 150 assertions pass. Rust sources were updated but not compiled or tested; Cargo was not run as instructed.

## Unified goal tool (2026-11-13)

The four model tools `create_goal`, `get_goal`, `update_goal`, and `resume_goal` are merged into one `goal` tool. `action` selects `create`, `get`, `update`, or `resume`; only that action's parameters are used, and unknown actions fail closed with `invalid_action`. The description, `promptSnippet`, `promptGuidelines`, and per-parameter descriptions disclose actions progressively, matching the existing `todo` tool convention. State transitions, budget rejection, unfinished-goal guards, pause/clear/resume semantics, and journaling are unchanged.

Model-facing reminders and continuation directives now reference `goal({ action: ... })`. The adapter recognizes the `goal` tool for its tool-end refresh, and Pager hides `goal` tool calls from scrollback while retaining the legacy titles used by stock Grok and historical transcripts. Validation: the goal extension tests pass; Rust files were not compiled or tested.
