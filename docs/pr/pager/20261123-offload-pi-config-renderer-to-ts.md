# PR: Offload `/pi-config` Resource Manager UI to TS Renderer Backed by Rust `PiResourceCatalog` API

- **Issue**: `docs/issues/pager/20261123-offload-extensions-and-tree-modals-to-ts.md` (Phase 3)
- **Scope**: `crates/codegen/xai-grok-pager/src/{pi_resource_config.rs,app/dispatch/settings/ui.rs,app/acp_handler/mod.rs}`, `crates/codegen/xai-grok-pager-bin/src/bin/{grok-pi.rs,grok_pi/config_extension.rs}`, `extensions/pi-grok-config/{index.ts,config-data.ts,config-tui.ts,config.test.ts,tsconfig.json}`

## Summary

Offloaded `/pi-config` (Pi resource management UI) in `grok-pi` to a standalone TS renderer (`extensions/pi-grok-config/`) while strictly preserving Rust (`pi_resource_config.rs`) as the single source of truth for resource discovery, project trust enforcement, and `settings.json` persistence.

## Changes

- **Rust Authoritative API (`pi_resource_config.rs`)**:
  - `to_ui_snapshot_json()` / `write_ui_snapshot_file()`: Exports discovered resources (`Extensions`, `Skills`, `Prompts`, `Themes`), scopes, origins, effective enablement, and `projectTrusted` flag to a per-process temp JSON snapshot.
  - `apply_ui_mutation(&Value)`: Applies `global` (`set_global_enabled`) or `project` (`set_project_override`) mutations requested by TS, preserving all trust & scope invariants.
- **Rust Dispatch & Callback Bridge**:
  - `dispatch_open_pi_config` (`dispatch/settings/ui.rs`): When `app.external_agent` is active, writes the catalog snapshot and invokes `__pi_config_manager` via `Effect::MentionAction`.
  - `handle_pi_ui_mention_insert` (`acp_handler/mod.rs`): Intercepts `kind == "pi_config_mutate"`, executes `catalog.apply_ui_mutation`, refreshes the snapshot file, and surfaces a confirmation/error toast.
  - `grok_pi/config_extension.rs` + `grok-pi.rs`: Materializes `pi-grok-config/` alongside `tui_kit::materialize_tui_kit`.
- **Standalone TS Renderer (`extensions/pi-grok-config/`)**:
  - Built on `pi-grok-tui-kit` (`renderSplitOverlayFrame`, `ListController`, `getFileIcon`, `hitRegion`).
  - Supports `[All]`/`[Extensions]`/`[Skills]`/`[Prompts]`/`[Themes]` tabs, `[Global]`/`[Project]` scope toggle (`Ctrl+G` or mouse click), live search, right-pane metadata & bounded file preview, `Space`/`Enter`/double-click toggle or override cycling, and full mouse support.

## Verification

- `bun test --cwd extensions/pi-grok-tui-kit` (7 pass)
- `bun test --cwd extensions/pi-grok-mentions` (28 pass)
- `bun test --cwd extensions/pi-grok-review` (4 pass)
- `bun test --cwd extensions/pi-grok-tree` (2 pass)
- `bun test --cwd extensions/pi-grok-config` (2 pass)
- `git diff --check` clean; local `cargo` skipped per `AGENTS.md`.
