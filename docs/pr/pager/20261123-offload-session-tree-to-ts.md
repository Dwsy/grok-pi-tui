# PR: Offload `/tree` Session Branch Explorer to Standalone TS Plugin (`extensions/pi-grok-tree/`)

- **Issue**: `docs/issues/pager/20261123-offload-extensions-and-tree-modals-to-ts.md` (Phase 2)
- **Scope**: `extensions/pi-grok-tree/{index.ts,tree-data.ts,tree-tui.ts,tree.test.ts,tsconfig.json}`, `crates/codegen/xai-grok-pager-bin/src/bin/{grok-pi.rs,grok_pi/tree_extension.rs}`, `crates/codegen/xai-grok-pager/src/app/{dispatch/session/tree.rs,acp_handler/mod.rs}`

## Summary

Migrated the `/tree` interactive session branch explorer UI in `grok-pi` to a standalone TypeScript plugin (`extensions/pi-grok-tree/`) built on `pi-grok-tui-kit` (`renderSplitOverlayFrame` + `ListController`), while keeping Rust in charge of the authoritative `Action::NavigateSessionTree` RPC and transcript reload.

## Changes

- **Standalone TS Plugin (`extensions/pi-grok-tree/`)**:
  - `tree-data.ts`: Parses Pi session JSONL branches, attaches `label` entries, traces the active root→leaf path, flattens into box-drawn DFS rows, and filters across `Messages` (default), `All`, and `Labeled` modes plus multi-term search.
  - `tree-tui.ts`: Bordered two-pane TUI powered by `pi-grok-tui-kit` + `ListController` with active branch (`●`) / current leaf (`★`) indicators, full wrapped preview on the right, `Enter` / double-click jump, `Ctrl+S` jump with branch summary, and full mouse support.
  - `index.ts`: Registers `__pi_session_tree`, returning `{ kind: "tree_navigate", id: entryId, title: summarize ? "summarize" : "" }` over `__pi_grok_mention_insert__` in `grok-pi` (and calling `ctx.navigateTree` in standalone Pi).
- **Rust Wiring**:
  - `tree_extension.rs` materializes `pi-grok-tree/` alongside `super::tui_kit::materialize_tui_kit` and is registered in `grok-pi.rs`.
  - `dispatch_show_session_tree` in `dispatch/session/tree.rs` routes to `Effect::MentionAction { kind: "tree", command: "__pi_session_tree", query: "" }`.
  - `handle_pi_ui_mention_insert` in `acp_handler/mod.rs` intercepts `kind == "tree_navigate"` and dispatches `Action::NavigateSessionTree` into `app.pending_effects`.

## Verification

- `bun test --cwd extensions/pi-grok-tui-kit` (7 pass)
- `bun test --cwd extensions/pi-grok-mentions` (28 pass)
- `bun test --cwd extensions/pi-grok-review` (4 pass)
- `bun test --cwd extensions/pi-grok-tree` (2 pass)
- `git diff --check` clean; local `cargo` skipped per `AGENTS.md`.
