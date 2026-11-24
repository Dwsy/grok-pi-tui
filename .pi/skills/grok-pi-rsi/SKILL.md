---
name: grok-pi-rsi
description: >-
  Repo-Specific Intelligence (RSI) guardrails distilled from deep audit of Dwsy/grok-pi-tui git history, architecture seams, and reference codebases.
  Load BEFORE editing pi-slim feature gates, adding F2 [ui] or HostFeatureManifest settings, modifying scrollback/thinking rendering, editing bundled Pi extensions, touching Herdr lifecycle code, or preparing CI/releases.
---
# grok-pi RSI — Deep Engineering Guardrails

Distilled from real regressions, multi-round CI red loops, and revert cascades in `Dwsy/grok-pi-tui`. Because **local `cargo` runs are disabled by default**, apply these static checks before committing or pushing.

---

## 1. `pi-slim` Feature-Gating & Compile-Closure Invariants

*Evidence: 4× revert cascade on `xai-grok-login` (`4a7c6f4f` → `ae9e2c05` → `892b4283` → `100fba2a`), 3-round `sampler` exit (`0a08965b`, `e9019db6`, `abcbc5bc`, `c3235875`, `e2cc9e96`), 5-round `stock-tool-dialects` loop (`41b22ffa`, `fefef3b0`, `f7dc8b9f`, `cc7e016e`), and `stock-agent`/`telemetry` cuts (`e6042077`, `7e31f61a`, `0bd9b3a4`, `c43de44f`, `e08f51db`).*

### Hard Rules
1. **Sink pure data/wire types BEFORE cutting a runtime crate.**
   - Move pure structs, traits, and wire constants down to `xai-grok-shared`, `xai-grok-sampling-types`, `xai-grok-workspace-types`, or `session/acp_types` FIRST (`e681953f`, `816f4c29`, `81f1aaa3`, `ff61b771`, `0bd9b3a4`), verify green, then gate the runtime edge.
   - **Known hard wall — do not re-attempt a quick cfg cut**: `xai-grok-login` exit is blocked at the type level by `PersistenceMsg` carrying auth and protected capabilities (`3c5c0f13`, `ba79cce4`, `81c6e220`).
2. **Verify BOTH compile profiles and feature independence.**
   - Pi profile: `xai-grok-pager-bin --bin grok-pi --no-default-features --features release-dist,sandbox-enforce` (workspace deps like `xai-grok-shell` compile with **zero** default features).
   - Stock profile: `xai-grok-pager-bin --bin xai-grok-pager --no-default-features --features stock-agent,stock-sentry,stock-mixpanel`.
   - `stock-mcp` implies `stock-agent`, **not** vice versa (`7e31f61a`): pure ACP merge helpers used by `acp_session`/`subagent`/`mvp_agent` belong under `stock-agent`, not `stock-mcp`.
   - **Forbidden crates in Pi compile closure** (asserted by `pi-slim-linux-checks.yml`): `xai-grok-sampler`, `sentry`, `xai-mixpanel`, `bm25`, `moka`, `kanal`.
3. **Static pre-push sweep (eliminates 90% of CI red rounds):**
   - **Manifest edge**: When repointing imports to a lower crate (e.g. `xai-grok-shared`), verify the consumer's `Cargo.toml` actually declares that dep (`cc7e016e`).
   - **Ungated → gated scan**: `rg` every moved/gated symbol (`crate::`, `super::`, `self::`) across both `stock-agent` and `stock-mcp` gates (`e6042077`).
   - **Test fixtures & behavior tests**: Gate `#[cfg(test)]` modules that construct `OaiCompatClient`/`SummaryGenerator` (`e2cc9e96`, `e9019db6`, `abcbc5bc`) or assert stock dialect presets (`hashline`, `codex` harness mismatch in `cc7e016e`) with `#[cfg(all(test, feature = "..."))]`.
   - **Re-exports & imports**: Drop duplicate re-exports after sinking (`E0252`, `c3235875`); verify remaining callers don't lose shared imports (`f6706dca`).
   - **Rust / TOML syntax traps**: Place `#[cfg(...)]` **before** `async fn` (`88b5d2ed`); wrap dual-branch `#[cfg]` expressions in `{ ... }` blocks (`6a1053d0`); never use `//` comments in `Cargo.toml` (`40b6f2d7`).

---

## 2. Settings & Feature Wiring — Choose the Right Path

Two distinct mechanisms exist; never mix them:

### Path A: Pi Host Features (`HostFeatureManifest`)
- Used for Pi extension toggles (`pi_herdr`, `pi_subagents`, `pi_emphasis`, `pi_working_message`, etc.).
- Driven declaratively by `extensions/pi-grok-*/grok-pi.json` + `crates/codegen/xai-grok-shell/src/host_features.rs` (`feature_spec_by_setting_key`). Do **not** hand-wire section mappings in `settings/layout.rs`.

### Path B: Native Pager `[ui]` Settings (12-File Exhaustive Checklist)
*Evidence: `6dd2e144`, `e71e263b`, `1a36fd78`, `92e0fd63`.*

1. `crates/codegen/xai-grok-shared/src/ui_config.rs` — enum (`from_canonical`, `as_canonical`, `label`) + `UiConfig` field + `Default`.
2. `crates/codegen/xai-grok-shell/src/util/config/settings_writes.rs` — `pub async fn set_<key>(...)` with canonical validation.
3. `crates/codegen/xai-grok-pager-render/src/appearance/cache.rs` — `pub use`, thread-local `Cell`, `load_*` / `set_*`, **and** seed inside `sync_ui_config_cache(ui)`.
4. `crates/codegen/xai-grok-pager/src/settings/defs.rs` — `*_CHOICES` + `SettingMeta` in `default_settings()`.
5. `crates/codegen/xai-grok-pager/src/settings/layout.rs` — map key in `section_for`.
6. `crates/codegen/xai-grok-pager/src/settings/registry.rs` — `current_value` arm **and** default-drift test assertion.
7. `crates/codegen/xai-grok-pager/src/app/actions.rs` — `Action::Set*`.
8. `crates/codegen/xai-grok-pager/src/app/dispatch/router.rs` — import + match arm.
9. `crates/codegen/xai-grok-pager/src/app/dispatch/settings/setters.rs` — `set_*_inner` (cache + `app.current_ui` + scrollback re-materialize) & `set_*` (`Effect::PersistSetting` + toast).
10. `crates/codegen/xai-grok-pager/src/app/dispatch/settings/ui.rs` — `action_for_setting_value` + `rollback_setting`.
11. `crates/codegen/xai-grok-pager/src/app/effects/helpers.rs` — `persist_setting_effect` arm.
12. `crates/codegen/xai-grok-pager/src/views/{settings_modal/state.rs,pi_settings/actions.rs}` + `crates/codegen/xai-grok-pager/src/app/dispatch/tests/settings.rs` — modal commit arms + exhaustive settings dispatch test arm.

Run `rg "<sibling_field>:" crates/codegen` to confirm no struct literal omits the new field.

---

## 3. Scrollback, Reserve Padding & Streaming Thinking Invariants

*Evidence: `e71e263b` (`thinking_stream_mode`), `scrollback/state/{mod,layout,pin_reserve}.rs`, `scrollback/blocks/thinking.rs`.*

- **Layered virtual bottom padding**: `ScrollbackState` layers two independent virtual pads on top of raw `content_height`:
  1. `thinking_hold_pad` (turn-scoped high-water floor under `ThinkingStreamMode::HeightHold`, released on `note_pin_reserve_turn_finished`, terminal width change, `clear()`, or mode switch).
  2. `pin_reserve_pad` (page-flip prompt pin reserve on top of `held_content`).
  - In incremental Case 2 height updates (`scrollback/state/mod.rs`), ALWAYS subtract **both** `pin_reserve_pad` and `thinking_hold_pad` before adding `total_delta`, then recompute `thinking_hold_pad` first and `pin_reserve_pad` second.
- **Single-line streaming previews**: Never right-truncate (`truncate_line`) a live streaming ticker, or new tokens vanish off the right edge. Use right-anchored display-width slicing (`tail_window_by_width` with `UnicodeWidthChar` for CJK safety) + dim `…` left-edge fade cue, and sanitize `<signature>...</signature>` / `**` markers (`clean_ticker_source`).

---

## 4. Bundled Pi Extensions & RPC Bootstrap Diagnosis

*Evidence: `55e904d5`, `bba61751`, `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/{*_extension.rs,extension_self_heal.rs}`.*

When Pi RPC bootstrap fails and self-heal drops an extension or relaunches with `-ne`, never trust the bisected entry filename alone:

1. **Read authoritative Pi stderr**: `{GROK_HOME}/logs/pi-rpc-stderr.log` (default `~/.grok-pi/logs/pi-rpc-stderr.log`).
2. **Test the materialized bundle in isolation** (before self-heal deletes `$TMPDIR/pi-grok-*-*`):
   ```bash
   pi -ne --mode rpc --extension <bundle>/index.ts
   ```
3. **Check transitive import mirroring**: Every relative TS import in `extensions/pi-grok-*/` must be copied by its Rust injector (`*_extension.rs`) and asserted in the injector's unit test.
4. **Separate tool-name conflicts**: `Tool "<name>" conflicts with …` comes from Pi's `resource-loader.ts` auto-discovering another extension; `-ne` separates bundle load bugs from cross-extension collisions.
5. **Headless boundary**: `pi-grok-adapter` must never import Ratatui/Crossterm or touch terminal raw mode; `grok-pi.rs` must stay composition-only (`docs/NATIVE_GROK_TUI_ALIGNMENT.md`).

---

## 5. Herdr Lifecycle, `omp-shell` & CI/Release Guardrails

*Evidence: `85cacfa9`, `0da431c5`, `d39d403b`, `7505b798`, `4709079a`, `a1e3ef01`, `d71dec40`, `2ac05062`, `b4a5db84`, `59f535c0`.*

- **Herdr authority**: Withhold session identity until the absolute session file path is materialized; preserve `session_start_source` (`startup` / `new` / `resume` / `fork`) across deferred `agent_start` and in-process session replacements; never drop fast transitions across bounded socket backlogs.
- **`omp-shell` backend**: Pins `jaq` crates and mirrors `oh-my-pi`'s `[patch.crates-io]`; changes to `BashOutput` require checking all struct literals across host & pager (`4709079a`, `9dba1b01`).
- **Online Linux CI**: Verify Rust/Cargo changes exclusively via:
  ```bash
  gh workflow run pi-slim-linux-checks.yml --repo Dwsy/grok-pi-tui --ref <branch> -f scope=fast # or all
  ```
  Non-release workflows stay `on: workflow_dispatch:` only; Shell/session tests require `RUST_MIN_STACK=8388608`.
- **Beta release preflight**: Add `## [X.Y.Z-beta.N] - YYYY-MM-DD` to `CHANGELOG.MD`, validate with `python3 scripts/extract-changelog-section.py --self-test` and `--strict`, push `main`, then tag & push `vX.Y.Z-beta.N` (beta builds macOS ARM64 prerelease by default).
- **Submodule hygiene**: Never stage `M pi-main`.

---

## 6. Reference Project Sources (参考项目源)

Consult these verified local/upstream sources directly instead of guessing:

| Reference | Location / Remote | Key Seams & Patterns to Reference |
|---|---|---|
| **Grok Build (Upstream)** | `upstream` (`https://github.com/xai-org/grok-build.git`) | Native Pager / Shell upstream baseline; inspect via `git show upstream/main:<path>` or `docs/upstream/UPSTREAM_CHANGELOG.md`. |
| **Pi Coding Agent** | `pi-main/` submodule (`https://github.com/earendil-works/pi`) | Authoritative Pi JSONL RPC (`packages/coding-agent/src/modes/rpc/`), session/tree lifecycle (`core/agent-session.ts`), and extension conflict rules (`resource-loader.ts`). Read-only for contract lookup. |
| **DeepSeek Harness (`dsh`)** | `~/.dsh/deepseek-harness` | Reasoning sanitization (`packages/client/ui-chat/src/client/reasoning-text.ts`), collapsed reasoning row & streaming preview (`chat/ReasoningRow.tsx`), turn-process folding (`conversation-nodes/turn-process.ts`), and scroll-follow (`chat/use-scroll-follow.ts`). |
| **ZCode (`zcode-pi`)** | `~/Dev/AI/zcode-pi/ZCode-main` | Right-anchored horizontal streaming reasoning ticker & left fade mask (`packages/ui/src/components/ai-elements/reasoning.tsx`), queued summary rolling (`ToolCallBlocks/QueuedSummaryContent.tsx`), and virtual scroll anchor state machine (`packages/ui/src/v4/timelineScrollAnchor.ts`). |
| **Codex (`codex`)** | `~/Dev/AI/codex` | Agent Command Center fullscreen `/resume` layout (`codex-rs/tui/src/app/agents_overview_view.rs`) and syntax-theme pastel identity colors (`codex-rs/tui/src/thread_color.rs`). |
| **Architecture Rulings** | `docs/issues/架构/` & `docs/NATIVE_GROK_TUI_ALIGNMENT.md` | Verified `pi-slim` dependency-graph boundaries (S09–S12), allowed Pager seams, and SHA-256 baseline rules. |

---

## 7. RSI Self-Maintenance Loop

After any multi-round CI red loop, revert, or production bug:
1. Distill the root cause into a 1–3 line invariant with the commit SHA and exact file seam.
2. Merge into the matching section above and prune any rule superseded by architecture changes.
3. Keep the skill dense and scannable — no chronological logs or generic advice.
