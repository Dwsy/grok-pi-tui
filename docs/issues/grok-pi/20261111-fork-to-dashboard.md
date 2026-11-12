# 2026-11-11 — Pi fork-to-dashboard

## Goal

Add /fork-to-dashboard (alias /frok-to-dashbord) to fork from a selected
Pi user-message entry and show the resulting session in Agent Dashboard.
Reuse native Pager UI, Pi RPC fork, and the existing session catalog. Do not
mutate Pi JSONL directly or add a second Agent runtime.

## Invariants and limitation

- /fork remains unchanged, and the same picker supports both command targets.
- pi/session/fork creates a separate Pi session file and rebinds the single
  Pi RPC host to it. The parent session is retained on disk and projected as a
  dormant roster row; this is NOT simultaneous multi-agent execution.
- The picker selects a user-message entry (the Pi get_fork_messages contract),
  not arbitrary tool/assistant entries in /tree.
- Request/result carries an explicit to_dashboard bit so a cancelled/failed
  fork never navigates the UI. A successful fork reloads the new session and
  opens Dashboard, focusing its live row.
- Worktree flags do not apply to Pi message forks.

## Acceptance

1. /fork-to-dashboard opens the Pi fork message picker; selecting a message
   forks the session and navigates to Dashboard.
2. /fork-to-dashboard <entry-id> skips the picker and uses that Pi message id.
3. The original /fork continues to fork into the agent view.
4. Failed or cancelled fork stays in the original view, with a toast.
5. Dashboard fetches Pi's catalog so the old session is still accessible.
6. Never alter Pi session files outside official Pi APIs.
