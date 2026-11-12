# Goal Session Ownership and Resume (2026-11-11)

Status: implemented, TypeScript regression tests passing; Rust build/tests not executed by user request.

## Bugs
- Process-temp PI_GROK_GOAL_CONTROL survives session switches inside one Pi process and incorrectly influences new/resumed sessions.
- No startup rehydration despite Pi already persisting pi-grok-goal/v1 custom entries in the session JSONL.
- /goal replaces active/paused/blocked, while create_goal blocks only active.
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
