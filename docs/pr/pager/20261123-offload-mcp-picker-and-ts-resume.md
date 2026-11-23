# PR: Offload `@mcp` Interactive Picker and Active-Agent `/resume` to TS TUI

- **Issue**: `docs/issues/pager/20261123-ts-tui-kit-and-rust-modal-offload.md` (Phase 3)
- **Scope**: `extensions/pi-grok-mentions/{mcp-picker-tui.ts,mcp-picker.test.ts,index.ts}`, `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/mentions_extension.rs`, `crates/codegen/xai-grok-pager/src/app/{dispatch/session/load.rs,dispatch/tests/session/load.rs,acp_handler/mod.rs}`

## Summary

Completed the `@mention` interactive picker quartet (`@session`, `@skill`, `@file`, `@mcp`) and offloaded active-agent `/resume` in `grok-pi` from the legacy Rust `ActiveModal::SessionPicker` to the shared bordered TS `SessionPickerTui` built on `pi-grok-tui-kit`.

## Changes

- **Interactive `@mcp` Picker (`extensions/pi-grok-mentions/mcp-picker-tui.ts`)**:
  - Built on `pi-grok-tui-kit` (`computeSplitGeometry`, `renderSplitOverlayFrame`, `hitRegion`, `highlightTerms`, `wrapTextToWidth`).
  - Left pane lists Pi-registered MCP servers with tool counts, transport (`http` / `stdio` / `tools`), and endpoint/command.
  - Right pane shows server metadata plus a scrollable, search-highlighted list of all registered tools and descriptions.
  - Supports `All` / `With Tools` filter pills (`Tab` or mouse click), live multi-term search across servers and tools, mouse click/double-click confirm, and wheel scrolling.
  - Registered `Ctrl+O` (`mcp picker`, `__pi_mention_mcp_picker`) on the `mcp` provider in `extensions/pi-grok-mentions/index.ts` and embedded `mcp-picker-tui.ts` in `mentions_extension.rs`.
- **TS `/resume` Offload (`__pi_resume_session`)**:
  - Registered `__pi_resume_session` in `extensions/pi-grok-mentions/index.ts`, reusing `createSessionPickerTui` with `titleBadge: "(/resume)"` and `confirmLabel: "resume"`.
  - In `grok-pi`, selection emits `emitMentionInsert(ctx, "resume", chosen.id, chosen.title, chosen.cwd)`; in standalone Pi, falls back to `ctx.switchSession(chosen.path)`.
  - `dispatch_show_session_picker` in `crates/codegen/xai-grok-pager/src/app/dispatch/session/load.rs` routes active-agent external `/resume` (`external_agent && has_agent`) to `Effect::MentionAction { kind: "resume", command: "__pi_resume_session", query: "" }`, while keeping the native Welcome-screen picker intact when no agent RPC channel exists yet.
  - `handle_pi_ui_mention_insert` in `crates/codegen/xai-grok-pager/src/app/acp_handler/mod.rs` intercepts `kind == "resume"` and dispatches `Action::LoadSession(id, cwd, false)` into `app.pending_effects` so the Pager performs a full session load and transcript replay.
  - Updated `external_resume_offloads_to_ts_session_picker_when_agent_active` in `crates/codegen/xai-grok-pager/src/app/dispatch/tests/session/load.rs`.

## Verification

- `bun test --cwd extensions/pi-grok-tui-kit` (4 pass)
- `bun test --cwd extensions/pi-grok-mentions` (27 pass, including 4 new `mcp-picker.test.ts` cases)
- `bun test --cwd extensions/pi-grok-review` (4 pass)
- `git diff --check` clean
- Local `cargo` commands intentionally skipped per `AGENTS.md`; Rust dispatch, ACP handler, and `include_str!` paths statically verified.
