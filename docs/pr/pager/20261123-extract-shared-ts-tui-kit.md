# PR: Extract Shared TS TUI Kit (`extensions/pi-grok-tui-kit/`)

- **Issue**: `docs/issues/pager/20261123-ts-tui-kit-and-rust-modal-offload.md` (Phase 2)
- **Scope**: `extensions/pi-grok-tui-kit/`, `extensions/pi-grok-mentions/`, `extensions/pi-grok-review/`, `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/{mentions_extension.rs,review_extension.rs}`

## Summary

Extracted duplicated bordered two-pane TUI framing, pill hit-testing, width-safe CJK wrapping, highlight matching, and Nerd Font / ASCII icon helpers into a single shared TypeScript library (`extensions/pi-grok-tui-kit/`), unifying the visual design language across all TS overlays (`@session`, `@skill`, `@file`, `/review-session`, `/review-message`).

## Changes

- **New shared library `extensions/pi-grok-tui-kit/`**:
  - `frame.ts`: `TuiTheme`, `ClickRegion`, `HeaderPillSpec`, `FooterHintSpec`, `SplitFrameGeometry`, `hitRegion`, `styleBold`, `fitWidth`, `wrapTextToWidth`, `highlightTerms`, `computeSplitGeometry`, `renderHeaderRow`, `renderSearchRow`, `renderFooterRow`, `renderSplitOverlayFrame`.
  - `icons.ts`: `FileIconSpec`, `detectNerdFontSupport`, `getFileIcon` (NvChad `nvim-tree` / `nvim-web-devicons` palette with ASCII fallback).
  - `index.ts`: Barrel re-export.
  - `kit.test.ts` & `tsconfig.json`: Unit tests for CJK display width, term highlighting, bordered frame alignment & pill hit regions, and Nerd/ASCII icons.
- **Consumer refactors**:
  - `extensions/pi-grok-mentions/{file-data.ts,session-picker-tui.ts,skill-picker-tui.ts,file-picker-tui.ts}` migrated to `pi-grok-tui-kit`.
  - `extensions/pi-grok-mentions/session-picker-tui.ts` gained optional `titleBadge` / `confirmLabel` props to enable direct reuse for `/resume`.
  - `extensions/pi-grok-review/{index.ts,review-tui.ts}` migrated to `pi-grok-tui-kit`, dropping unused imports from `session-picker-tui.ts`.
- **Rust embedded bundle alignment**:
  - `mentions_extension.rs` and `review_extension.rs` now materialize sibling `pi-grok-tui-kit/{frame.ts,icons.ts,index.ts}` directories inside their `TempDir` so relative imports (`../pi-grok-tui-kit/index.ts`) resolve identically at runtime and in source.

## Verification

- `bun test --cwd extensions/pi-grok-tui-kit` (4 pass)
- `bun test --cwd extensions/pi-grok-mentions` (23 pass)
- `bun test --cwd extensions/pi-grok-review` (4 pass)
- `git diff --check` clean
- Local `cargo` commands intentionally skipped per `AGENTS.md`; Rust `include_str!` paths and `TempDir` directory layout statically verified.
