---
name: grok-pi-remote-tui-rsi
description: >-
  Repo-Specific Intelligence (RSI) guardrails for grok-pi Remote TUI, bundled TS overlays (/resume, /tree, /review, @session, @skill, @file, @mcp), startup jiti performance, and Codex-style TUI layout.
  Load BEFORE modifying extensions/pi-grok-remote-tui/, extensions/pi-grok-mentions/, extensions/pi-grok-tree/, extensions/pi-grok-review/, extensions/pi-grok-tui-kit/, or Remote TUI seams in xai-grok-pager and pi-grok-adapter.
---

# grok-pi-remote-tui-rsi

Focused engineering guardrails for `grok-pi`'s Remote TUI bridge and bundled TypeScript overlays, distilled from production fixes in `Dwsy/grok-pi-tui`.

## When to Use

- Editing `extensions/pi-grok-remote-tui/` or `pi/ui/remote_tui/*` RPC/keyfile plumbing in Rust.
- Building, resizing, or debugging TS overlays (`/resume`, `/tree`, `/review`, `@session`, `@skill`, `@file`, `@mcp`).
- Fixing Remote TUI mouse scroll bleed, background tint bleed, terminal resize, or startup `jiti` latency.
- Adjusting CJK table alignment, syntax highlighting, search debounce, or Codex-style session colors.

## 1. Remote TUI Host, Surface & Resize Invariants

*Evidence: `71ef2edc`, `245eb477`, `extensions/pi-grok-remote-tui/{host,transport,layout,shared}.ts`, `xai-grok-pager/src/app/{app_view,actions,effects/mod}.rs`, `pi-grok-adapter/src/pi_adapter{,.rs,/agent.rs}`.*

1. **Render Remote TUI on both `ActiveView::Welcome` and `ActiveView::Agent`.**
   - Keep `welcome_remote_tui_rect` wired so Welcome shortcuts (`Ctrl+R`, `F3`, `/resume`) open the fullscreen TS `/resume` overlay instead of falling back to the legacy Rust picker (`71ef2edc`).
   - Route `dispatch_show_session_picker` directly to `__pi_resume_session` whenever `external_agent` is active.
2. **Isolate scroll and background tint when an overlay is open.**
   - Absorb unhandled mouse events (including macOS trackpad `ScrollLeft`/`ScrollRight` emitted alongside vertical scroll) and gate `dispatch_scroll` / `tick_scroll` while `remote_tui_id.is_some()` (`71ef2edc`).
   - In `render_remote_tui_overlay_surface`, reset covered cells and fill transparent span backgrounds with `theme.bg_base` so underlying text or tint never bleeds through.
3. **Preserve end-to-end terminal `resize` across all 3 layers.**
   - Pager (`app_view.rs`): on `Event::Resize(cols, rows)` when `self.external_agent`, emit `Effect::RemoteTuiResize { id: remote_tui_id.clone(), cols, rows }` -> `pi/ui/remote_tui/resize` (`245eb477`).
   - Adapter (`pi_adapter.rs` / `agent.rs`): always persist `{ cols, rows }` to `<meta>.viewport.json` (so overlays opened *after* a resize use live dimensions) and append keyfile `op: "resize"` if an overlay is active.
   - TS Host (`transport.ts` / `layout.ts` / `host.ts`): coalesce consecutive `resize` events, update `tui.terminal` + `process.env.PI_GROK_REMOTE_TUI_{WIDTH,ROWS}`, recompute layout, call `component.invalidate?.()`, and `pushFrame()`.
4. **Mirror every new TS module in Rust injectors.**
   - Any new runtime `.ts` file added to `extensions/pi-grok-{remote-tui,tui-kit,mentions,tree,review,config}/` MUST be embedded via `include_str!` in its corresponding Rust injector under `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/{remote_tui_extension,tui_kit,mentions_extension,tree_extension,review_extension,config_extension}.rs`, or packaged binaries will fail at runtime with `ERR_MODULE_NOT_FOUND`.

## 2. Startup Performance & `jiti` Import Traps

*Evidence: `3a5ec746`, `76b72cbb`.*

1. **Never use top-level value imports of `@earendil-works/pi-coding-agent` inside `extensions/pi-grok-*/*.ts`.**
   - Top-level value imports pull in the full coding-agent barrel (including optional peers like `lovely-mermaid`), breaking standalone `bun test` and slowing `jiti` startup.
   - Use `import type { ... }` at top level, and load runtime helpers (such as `highlightCode` and `getLanguageFromPath`) lazily via `await import("@earendil-works/pi-coding-agent").catch(() => null)` inside async command handlers (`76b72cbb`).
2. **Lazy-load heavy TUI overlay modules inside command handlers.**
   - Keep `index.ts` lightweight on `session_start`; dynamically `await import("./*-tui.ts")` only when the user invokes the command (`3a5ec746`).
3. **Avoid synchronous subprocess probes on startup.**
   - For managed Pi installs, read `~/.pi/agent/install/current-version` (`read_managed_pi_version`) instead of spawning `pi --version` (~1.9s).

## 3. TS Overlay Layout, Sizing & CJK Alignment

*Evidence: `71ef2edc`, `e5f181f8`, `76b72cbb`, `245eb477`, `extensions/pi-grok-tui-kit/frame.ts`.*

1. **Override `computeSplitGeometry`'s default `34`-row cap for 90vh overlays.**
   - `computeSplitGeometry` defaults to `heightRatio: 0.84, maxRows: 34`.
   - Standardize bordered overlays (`/tree`, `/review`, `@session`, `@skill`, `@file`, `@mcp`) to `overlayOptions: { width: "90%", maxHeight: "90%", anchor: "center" }` and pass:

     ```ts
     const termRows = this.getRows();
     const targetRows = Math.max(16, Math.floor(termRows * 0.9));
     computeSplitGeometry({ ..., rows: termRows, heightRatio: 0.9, maxRows: targetRows });
     ```

   - Keep `/resume` fullscreen (`width: "100%", maxHeight: "100%", anchor: "top-left"`).
2. **Reserve explicit column gap inside `titleBudget` before `truncateToWidth`.**
   - If `titleColW + rightColsW === innerW`, a truncated title leaves `0` columns for `gap`, and `Math.max(1, innerW - left - right)` overflows by 1 column, shifting right columns and clipping the trailing edge into `…` (`245eb477`).
   - Always subtract `titleGap` (at least `2`) from `titleBudget` before truncating.
3. **Flatten linear single-child chains in `/tree`.**
   - Only fork points (`children.length > 1`) add `├─`/`└─` connectors and increment indentation depth; single-child turns stay vertically aligned (`71ef2edc`).
   - Keep `pi-main` shortcuts complete (`Alt+←/→` branch jump, `Home/End`, `Ctrl+X` copy, `Shift+L` inline label, `Shift+T` timestamps, `Ctrl+D/T/U/L/A` and `Ctrl+O`/`Shift+Ctrl+O` filters).

## 4. Search, Debounce, Syntax Highlighting & Colors

*Evidence: `38f904a0`, `26b1dbb0`, `76b72cbb`, `245eb477`.*

1. **Debounce expensive search backends while keeping unit tests synchronous.**
   - Pass `grepDebounceMs: 180` (`rg`) and `searchDebounceMs: 150` (SQLite FTS5) from `extensions/pi-grok-mentions/index.ts`, while keeping TUI constructor defaults at `0` so existing synchronous `bun test` suites remain deterministic (`76b72cbb`).
   - Show a `searching…` indicator while pending, flush pending `rg` on `Enter`, and always clear timers on `Esc`, mode change, or mouse close (`finish()`).
2. **Handle leading `/` cleanly in fullscreen search bars.**
   - When the placeholder displays `/ type to filter...`, treat the first `/` on an empty query as `searchFocused = true` rather than appending a literal `"/"` (which renders `/ /▏`); allow a second `/` or paste to insert literal slashes (`245eb477`).
3. **Reuse Pi's `highlightCode` safely in file previews.**
   - Expand tabs before highlighting and cache highlighted lines per file (`76b72cbb`).
   - `highlightTerms` in `extensions/pi-grok-tui-kit/frame.ts` MUST split by ANSI CSI/OSC escape sequences and only run regex replacement on visible text segments so Markdown / syntax-highlighted lines are never corrupted by numeric or short search terms (e.g. `38`, `3m`).
4. **Use theme-aware Codex-style palettes and short model names.**
   - Dark terminals require high-luminance pastels (modeled on `~/Dev/AI/codex/codex-rs/tui/src/thread_color.rs`); auto-switch to richer tones on light terminals (`245eb477`).
   - Always display `shortModelName(model)` (segment after the last `/`) in both `/resume` and `@session` (`26b1dbb0`, `76b72cbb`).
5. **Keep session catalog and mention catalog hydrated.**
   - Query PSM's `message_fts` (FTS5) + `sessions` across the full archive for `/resume` and `@session`, lazily hydrate `PiAgent::resolve_session_path` on cache misses, and persist `mention_catalog` on `AppView` (`38f904a0`, `71ef2edc`).
   - **No artificial 300-row cap & 10k+ session scaling**: Avoid upfront `LEFT JOIN session_details_cache`, `ORDER BY` random index lookups, and 10k-row `fs.existsSync` sweeps on startup. Load `sessions` with `includeDetails: false` + JS ISO-timestamp sort + DB/WAL `(ino, size, mtimeMs)` catalog cache, lazily hydrate right-pane details via `loadSessionRecordDetails` (~1ms PK lookup), cache lowercase search fields in a `WeakMap`, and maintain an O(1) `hitIndexToRowIndex` table for fullscreen `clampListScroll`.
   - **Fullscreen sort invariant**: When `fullscreen === true` and `sortMode !== "threaded"`, populate `groupedRows` with flat `{ kind: "item", hitIndex, hit }` entries (never `[]`) and update the top-bar label (`Group: Project` vs `Sort: Recent` / `Sort: Relevance`).
   - **Safe JSONL mutations & `sessions-lite.db` fallback**:
     - Never write to PSM's `sessions.db`. For `appendSessionNameToJsonl`, inspect the final byte before appending (prepend `\n` if non-empty and not `0x0a`) and add a random hex suffix to `id`.
     - When PSM's `sessions.db` is absent, transparently activate `extensions/pi-grok-mentions/session-lite-db.ts` (`sessions-lite.db` with WAL, incremental `(path, mtimeMs, size)` sync, bounded 16 KB head + 32 KB tail probe, lazy `session_details_cache` hydration, corrupt-DB self-recovery, and in-memory fallback when SQLite bindings are absent).
   - **Coalesced preview wheel & bounded caches**: Use `RenderCoalescer` in `extensions/pi-grok-tui-kit/frame.ts` for right-pane wheel scrolling (`render: false` + `schedule()`, `cancel()` on every close path) and clear `previewLineCache` when target/width changes to prevent unbounded growth during divider drags.
6. **Inject resolved `@` mentions via `before_agent_start` `custom_message`, never `systemPrompt`.**
   - Return `{ message: { customType: "pi-grok-mentions/v1", display: false, content: "<referenced_objects>...</referenced_objects>" } }` using Pi-style XML tags (`<session>`, `<skill>`, `<file>`, `<folder>`, `<mcp>`) instead of mutating `event.systemPromptOptions.sections.referenced_objects`, so LLM system prompt KV cache stays intact and references persist cleanly in JSONL.

## 5. Verification Checklist

1. Run TypeScript extension tests directly (fast, no Cargo needed):

   ```bash
   bun test --timeout 15000 extensions/pi-grok-tui-kit/ extensions/pi-grok-mentions/ extensions/pi-grok-tree/ extensions/pi-grok-review/ extensions/pi-grok-config/ extensions/pi-grok-remote-tui/ extensions/pi-grok-goal/
   ```

2. Verify whitespace and diff hygiene:

   ```bash
   git diff --check
   ```

3. For any Rust change (`actions.rs`, `app_view.rs`, `effects/mod.rs`, `pi_adapter`), statically check every `match` arm, import scope, and `#[cfg]` gate (`rg "<VariantName>" crates/`), and NEVER run local `cargo` commands or stage `M pi-main`.

## 6. Completed RSI Optimizations & Next Backlog

Implemented & guarded by unit tests:
- **Remote TUI frame deduplication** (`extensions/pi-grok-remote-tui/host.ts`): `pushFrame(force = false)` short-circuits `ui.setWidget` when rendered lines match `lastPushedLines` (`show()` passes `force = true`).
- **Overlay parity for `@mcp` and `/pi-config`** (`mcp-picker-tui.ts`, `pi-grok-config/config-tui.ts`): Both use `RenderCoalescer` for right-pane wheel scrolling (`cancel()` on close) and bounded preview line caches; `/pi-config` also honors 90vh `targetRows`.
- **Tail-chunked JSONL preview** (`session-data.ts` `loadPreviewFromJsonl`): Files > 256 KB read trailing chunks (256 KB → 1 MB → 4 MB) until `maxMessages` are gathered instead of `fs.readFileSync` on the whole file.
- **Non-blocking `sessions-lite.db` background refresh** (`session-lite-db.ts` `scheduleLiteDbBackgroundSync`): Returns immediately when `sessions-lite.db` exists and runs incremental sync on the next tick.
- **Unified Split-Pane Interaction & `DividerDragController`**: All split TUI overlays (`/resume`, `@session`, `@skill`, `@file`, `@mcp`, `/tree`, `/review-*`, `/pi-config`, `/trace`, `__pi_extensions_modal`, `/goal-detail`) share `DividerDragController`, `RenderCoalescer`, and native `handleMouse(TuiMouseEvent)`.
- **Full-Session `/trace` & Custom Pi Element Rendering** (`extensions/pi-grok-review/{trace-data.ts,trace-tui.ts,tool-render-tui.ts}`): `/trace` and Left-arrow (`__pi_trace`) inspect full JSONL entries with `Pretty → Pi Element → Raw JSON` cycling; F2 opt-in `pi_custom_tool_renderer` (default OFF) routes non-builtin custom tool Enter (`__pi_tool_render`) to plugin `renderCall`/`renderResult` (with session-scoped per-tool mode memory and `getMessageRenderer` support), while built-in tools keep native Rust viewers.
- **TS Migration of Heavyweight Rust Modals (`grok-pi` / `external_agent` path)**:
  - `extensions_modal` → `extensions/pi-grok-config/extensions-modal-tui.ts` (`__pi_extensions_modal`, wired via `transcript.rs` & `config_extension.rs`).
  - `goal_detail` → `extensions/pi-grok-goal/goal-detail-tui.ts` (`__pi_goal_detail` / `/goal-detail`, wired via `router.rs` & `goal_extension.rs` with `tui_kit`).
- **Adapter / RPC & Session Index Pipeline** (`crates/codegen/pi-grok-adapter/`):
  - `psm_session_catalog.rs`: Read-only `sessions-lite.db` fallback (catalog, search, ID lookup) when PSM is absent, 3s TTL cache on PSM port probe, and `PI_CODING_AGENT_DIR` alignment.
  - `pi_adapter/session.rs`: `resolve_session_path` uses O(1) indexed SQLite point lookup before disk scan instead of loading the full catalog.
  - `pi_rpc.rs`: 64 KB stdout `BufReader` and zero-allocation response ID routing on streaming events.
- **Static Rust-injector parity test** (`extensions/pi-grok-tui-kit/kit.test.ts`): Verifies every runtime `.ts` file under `extensions/pi-grok-{tui-kit,mentions,tree,review,config,goal,remote-tui}/` is embedded via `include_str!` in its corresponding Rust injector.

Next candidates:
1. Cold-start project-first chunked indexing in `session-lite-db.ts` for users with thousands of unindexed JSONLs and no PSM.
2. Retire or feature-gate replaced Rust modal rendering paths once `grok-pi` parity is verified via `cargo check` / `cargo test`.
