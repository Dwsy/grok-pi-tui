# Grok Native TUI × Pi Verification Report

## 2026-11-11 Team message delivery and model-error terminal status

- PASS: all subagent extension suites against installed Pi 1.5.0, **98/98** tests. The seven new real-SDK offline scenarios cover root→child, child→root, peers, active/idle final-answer delivery, deferred `NEW_TASK`, and resolved provider error/abort handling. Their in-process provider scripts replace model calls, and each scenario rejects network access.
- PASS: strict/noEmit production-extension TypeScript check against installed Pi declarations; the same check including the new `delivery.test.ts` also passes. Existing test fixtures outside this scoped check were not retyped.
- PASS: bounded merge/automatic flush, duplicate coalescing, one-target failure isolation, shutdown cleanup, queue/wait ordering, nested parent resumption, and existing concurrency/UI regressions. `git diff --check` passed.
- NOT RUN: Cargo/build or native TUI/real-provider E2E. This is an extension-only change; the installed binary and active sessions were not rebuilt or hot-reloaded. A steer still waits for an already-running request/tool to reach a safe boundary.

Design, scope and local change record: [issue](issues/subagents/20261111-Team%20消息及时投递与错误终态.md), [PR record](pr/subagents/20261111-Team%20消息及时投递与错误终态.md).

Reproduce from `extensions/pi-grok-subagents`:

```bash
env PI_TEST_NODE_MODULES=/absolute/pi-1.5.0-release/node_modules \
  node --import ./test-installed-pi.mjs --test --test-reporter=spec *.test.ts
```

## 2026-11-10 iTerm2 Read image viewer

Scoped Rust formatting and `git diff --check` passed. The focused renderer test command remained in dependency compilation for over 20 minutes and did not reach tests; a stop was requested. The new image-loading and overlay lifecycle tests, composition build/check, and real iTerm2 visual acceptance are not yet verified. See [issue](issues/pager/20261110-iterm2-image-compatibility.md).

## 2026-11-09 Ondine Mermaid images and SVG

Open Image and Copy Image Path compile with pinned Ondine and produce PNG; Open SVG
produces a separately cached standalone themed SVG through the same isolated
render worker. The terminal ASCII renderer is independent. The workspace Rust
toolchain is now 1.97.0 as required by the upstream compiler.

- PASS: Rustfmt parsing/format checks, TOML/JSON parsing, exact source-identity
  seam set and pinned lockfile structure, and `git diff --check`.
- INCOMPLETE: the full static architecture verifier did not finish within its
  55-second limit. No full-verifier result is claimed.
- NOT RUN: Cargo, compilation, tests, real viewer opening, and visual acceptance,
  as requested. Focused SVG/cache and subprocess tests were added for later use.

## 2026-11-05 Pasted image original paths (issue #12)

The native image builder carries the user-visible source path in ACP image metadata (`xai.dev/imageSourcePath`), independently of the durable attachment URI. The headless adapter appends numbered, JSON-quoted paths to the Pi user prompt in `<image_metadata>`, preferring the original path and falling back to an existing local `file://` URI. Image bytes, MIME types, display numbers, chips and previews retain their existing behavior.

| Check | Result | Evidence / limits |
|---|---|---|
| Adapter prompt bridge | PASS | `./scripts/cargo-shared.sh test -p pi-grok-adapter --lib prompt_bridge::tests`: **7 passed, 0 failed, exit 0**. Original-path precedence, quoting/Unicode, display numbers, local-URI fallback, unchanged image payloads, and prompts without paths. |
| Native image handling | PASS | `./scripts/cargo-shared.sh test -p xai-grok-pager-render --lib prompt_images::tests -- --quiet`: **156 passed, 0 failed, exit 0**. Includes actual persistence, user-visible symlinks, orphan recovery and existing image handling. Initial dependency compilation exceeded the command wrapper deadline; a cached rerun confirmed normal exit. |
| Local composition binary | PASS | `./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi` and `./target/debug/grok-pi --version`: **exit 0**, `grok-pi 0.1.12-beta.1+dirty [stable]`. No global installation changes. |
| Native architecture verifier | EXISTING FAILURES | **13 passed / 8 failed, exit 1**. The failed-check set exactly matches source session `01a1a9ae-6206-7095-941e-706a9259a51d`, JSONL L153, before this change: uploaded-tree and renderer baselines, slash/dynamic-command checks, extension UI, runtime events, semantic mappings and dialog matching. |
| Source-identity seam | PASS | Baseline hashes and other manifest fields are preserved; only `xai-grok-pager-render/src/prompt_images.rs` was added to the exact modified-file seam list and its verifier counterpart. No existing failing checks were removed or relaxed. |
| Format and change scope | PASS | Two changed Rust sources pass `rustfmt --check --edition 2024`; `git diff --check` exits 0. Source hashes match the reviewed/tested versions, and comparison with pre-task snapshots confirms existing user changes are preserved. |
| Live provider / terminal interaction | NOT RUN | Source-level tests and local build only. |

Implementation and acceptance record: [Image source path metadata](issues/adapter/20261105-image-source-path-metadata.md).

## 2026-11-05 Code Mode card default expansion setting

`[ui].codemode_expanded_by_default` is an F2 Appearance → Tool output preference. It defaults OFF (existing Truncated preview); ON selects Expanded for new Code Mode cards. Changing the setting remaps only cards still on the previous default. Per-card Code Mode fold actions are pinned so later default flips do not override them, even when the global `respect_manual_folds` option is off.

| Check | Result | Evidence / limits |
|---|---|---|
| Render cache toggle | PASS | `./scripts/cargo-shared.sh test -p xai-grok-pager-render --lib codemode_expanded_by_default -- --nocapture`: **1 passed, 0 failed, exit 0**. |
| Startup cache prime | PASS | The compiled `xai_grok_pager_render` test binary ran `appearance::cache::tests::prime_seeds_all_caches`: **1 passed, 0 failed, exit 0**; verifies `UiConfig` seeds the new flag. |
| Formatting / whitespace | PASS (scoped) | Rustfmt 1.94 check passed for the touched Rust files without pre-existing formatting drift; `git diff --check` exits 0. |
| Pager state and F2 dispatch tests | WAITING | `./scripts/cargo-shared.sh test -p xai-grok-pager --lib codemode -- --nocapture` is queued behind an already-running shared-target Cargo check (`xai-grok-pager --tests` + `grok-pi`); no Pager test result is claimed yet. |
| Live F2 interaction | NOT RUN | Source and unit tests only; no interactive terminal session. |

Implementation record: [Codemode native rendering](issues/adapter/20260930-Codemode-原生渲染支持.md).

## 2026-11-03 Edit Markdown regex-backend regression

The final fix restores `syntect = "5.3"`, the dependency configuration from before upstream commit `bc7f02ed`. That upstream change disabled the default Oniguruma engine in favor of `default-fancy`. Recent Edit streaming/folding changes did not modify the hunk highlighting algorithm. No Pager rendering behavior is changed by this final fix.

Controlled A/B used the same syntect 5.3.0, two-face 0.4.5, Grok Day theme, unoptimized dev profile, and the original session's three README edit batches. Only the syntect backend feature changed; two-face's original grammar configuration remained the same.

| Input batch (JSONL line) | fancy | Restored original backend | Highlighted lines | Text/RGB/font/segmentation differences |
|---|---:|---:|---:|---:|
| English README (129) | 10.354 s | 1.085 s | 37 | 0 |
| Chinese README (137) | 6.652 s | 0.951 s | 34 | 0 |
| Chinese README (139) | 5.897 s | 0.986 s | 34 | 0 |
| Total with runner overhead | 22.904 s | 3.026 s | 105 | 0 |

- **PASS:** both A/B runs exited 0, and all 105 styled lines match exactly. The sampled slow stack ends in `fancy_regex::RegexBuilder::build`.
- **PASS:** all five Pager source files from the earlier asynchronous workaround were verified against their last task-owned hashes and restored to their original versions. The earlier worker allowlist addition was removed.
- **PASS:** `Cargo.lock` only adds `onig` / `onig_sys` plus syntect's dependency edge. No other package versions changed.
- **PASS:** current-workspace Markdown tests: **484 library tests + 1 integration regression passed, 0 failed, command exit 0**. The integration test preserves actual README old/new text and real syntax spans under a one-second bound; a direct run measured 299 ms for both syntax passes.
- **PASS:** final `git diff --check`, new-test rustfmt, exact source-seam list consistency, and lockfile scope checks.

Reproduce the final tests:

```bash
./scripts/cargo-shared.sh test -p xai-grok-markdown \
  --lib --test edit_highlight_regression -- --test-threads=1 --quiet
```
- The earlier 64-test result applied to the now-withdrawn asynchronous workaround and is superseded for final acceptance.
- **NOT RUN:** full workspace build/verification and live terminal interaction with a newly rebuilt application. The running grok-pi binary has not been replaced. The root-cause evidence identifies the backend configuration regression for this input; it does not infer which release the user last observed working.

Issue and experiment record: [Edit expansion freeze](issues/TUI/20261103-edit-expand-freeze.md).

## 2026-11-02 Subagents V2 Multitask P0

Multitask is an explicit, task-scoped skill strategy. It preserves child resource isolation and adds a semantic `waiting` report: the coordinator must then end its run to release a background slot; nested results resume its existing Pi session. `team_list` shows `WAITING_RESULT`. This is not process isolation or a hard budget/DAG scheduler.

| Check | Result | Evidence / limits |
|---|---:|---|
| Extension behavior tests | PASS | Node 24.15.0, installed Pi 1.4.0, `index.test.ts bridge.test.ts runtime.test.ts v2.test.ts`: **64 passed, 0 failed**. Includes V1/V2/off discovery with real Pi skill parsing, four-slot saturation/release, waiting/resumption, dropped completion before/after parent turn end, and failure/cancellation delivery. |
| Reproducible installed-Pi test entry | PASS | `test-installed-pi.mjs` uses Node resolver hooks and an explicit `PI_TEST_NODE_MODULES`; it changes neither production resolution nor repository dependencies. |
| Production extension TypeScript | PASS (scoped) | `tsgo`, strict/noEmit/skipLibCheck, production extension TS only; temporary config maps Pi exports to the installed 1.4.0 declarations and typebox to its declaration file. Tests excluded; this is not a full workspace typecheck. |
| Full source-checkout / test typecheck | BLOCKED (baseline/compatibility) | Default Bun imports fail before assertions because optional `pi-main` is missing generated provider data (`meta.json` / `typesafe.json`). A separate all-file typecheck against installed Pi also surfaces existing test fixture/context diagnostics (`PersistedRecord`, `map(JSON.parse)`, `ExtensionToolContext`). No upstream files or unrelated test fixtures were changed. |
| Skill authoring lint | PASS | `portable=true`, 0 errors, 0 warnings; Pi skill discovery has no format diagnostics. |
| Skill workflow exercise | PASS (planning only) | Independent read of the actual skill correctly reuses a running reviewer, chooses single-agent spawn without an unverified preset, includes explicit child contracts, and uses waiting + turn end for a queued nested dependency. No real provider task was executed. |
| Rust bundled injector | PASS | `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi subagent_extension::tests -- --nocapture`: **4 passed, 0 failed**; includes authored/bundled skill content parity, relative module materialization and preset parity. |
| Diff and new-file hygiene | PASS | `git diff --check`, new-file trailing whitespace/newline checks, design-link target checks and Node syntax check for the test resolver. |
| Local binary build | PASS | `./scripts/cargo-shared.sh build -p xai-grok-pager-bin --bin grok-pi` succeeded; `./target/debug/grok-pi --version` exited 0 (`0.1.12-beta.1+dirty [stable]`). No global installation or release promotion. |
| Native provider/UI E2E | NOT RUN | Not promoted to default-on. Local binary is rebuilt; a real provider handtest remains. Hard tree quotas, dependency locks, cascading cancellation, busy-input queue conversion and live-team restart recovery remain proposed P1 work. |

Reproduce behavior tests from `extensions/pi-grok-subagents` with an installed Pi release:

```bash
env PI_TEST_NODE_MODULES=/absolute/pi-release/node_modules \
  node --import ./test-installed-pi.mjs --test index.test.ts bridge.test.ts runtime.test.ts v2.test.ts
```

Research, design choices, rollout scope and acceptance criteria: [Multitask design record](issues/adapter/20261102-subagents-v2-multitask.md).


## 2026-11-02 Effort menu strongest-first; verb folds need two members

- Requirement 1: the model thinking-strength picker listed efforts
  weakest-first (Minimal → High). `ModelState::reasoning_effort_options_for`
  now orders the menu strongest-first (stable sort on `ReasoningEffort`,
  which derives `Ord` in declaration order = strength), and the shell
  session-config effort rows (`session_config_inputs`) sort the same way.
  Wire order, catalog default derivation (`derive_reasoning_effort_fields`),
  and token resolution are untouched. Cycling (`next_reasoning_effort`)
  follows the menu order (highest first).
- Requirement 2: `RunScan::folds` required only one member, so a single tool
  call collapsed behind a one-row header. Runs now fold at two members:
  `verb_group.rs` `folds()` is `members >= 2`; the shared predicate keeps the
  layout fold and `verb_group_range_of` aligned. Setting/docs copy updated
  (`group_tool_verbs`, 26-config-reference).
- Pager full `--lib` suite: 10011 passed, 133 failed, 4 ignored —
  byte-identical counts to the pre-change baseline documented above; every
  failure encountered during focused runs was reproduced at clean `HEAD`
  (git stash round-trip), so no regressions were introduced.
- Focused suites: verb/fold tests (`scrollback::` 1086 passed / 3
  pre-existing failures), effort/reasoning/model_state/slash (82 passed / 1
  pre-existing failure), shell `session_config` + `mvp_agent` (11 passed),
  `xai-grok-sampling-types` (all passed). Reworked/added tests:
  `rendered_verb_group_singleton_stays_unfolded`,
  `verb_group_folds_multi_member_runs_but_not_singletons`,
  `verb_group_thoughts_never_count_and_never_fold_alone`,
  `verb_group_singleton_stays_unfolded_across_expansion_attempts`,
  updated pending-input / boundary-gap / separator / nav reveal /
  expand-all-thinking / hook-popup tests, and the strongest-first menu
  assertions in `model_state` + `mvp_agent::tests`.
- Per-file rustfmt (pinned 1.94.0, edition 2024) and `git diff --check`:
  passed.
- Disk guard: ran with `CARGO_MIN_FREE_GIB=10` because the volume was below
  the default 20 GiB floor; no `cargo clean` was performed.
- Scope: source and test verification only. No installed-binary or
  model-driven terminal acceptance.


## 2026-11-02 Streaming edit preview must honor the collapse default

- Cause: `toolcall_start` pushes the Edit card with empty `raw_input`/content,
  so `path_from_title` marked `summary_untrusted` and force-expanded the card
  past `[ui].collapsed_edit_blocks`; the trusted canonical replace never
  re-applied the materialize policy (only the trusted→untrusted rising edge
  existed).
- Fix: `tool_call_to_block` no longer treats a missing path as untrusted when
  no diffs exist yet (title-fallback with real diffs and multi-file patches
  still expand); `replace_tool_block` gained an untrusted→trusted falling edge
  that re-runs `edit_default_display_mode` unless the fold is pinned.
- Pager full `--lib` suite: 10011 passed, 133 failed, 4 ignored. Failure list
  is byte-identical to the pre-fix baseline captured on the same worktree
  (133 known failures; `comm` diff empty in both directions), so the change
  introduces no regressions.
- Focused new/adjacent tests: `streaming_edit_preview_honors_collapse_until_
  summary_exists`, `title_fallback_preview_with_diffs_still_expands`,
  `replace_tool_block_untrusted_falling_edge_reapplies_default`,
  `streaming_file_diff_is_foldable_viewable_and_preserves_fold_through_
  execution`, `multi_diff_and_title_fallback_edits_default_expanded`,
  `push_applies_edit_materialize_policy`, `acp::tracker::tests` (199 passed),
  `scrollback::state::tests` (61 passed), coalesce suite (40 passed).
- Per-file rustfmt --check and `git diff --check`: passed.
- Disk guard: ran with an explicit `CARGO_MIN_FREE_GIB=12/13` floor because the
  volume was below the default 20 GiB floor; no `cargo clean` was performed.
- Scope: source and test verification only. No installed-binary or
  model-driven terminal acceptance.


## 2026-10-30 Tool argument streaming — Bash stability and live file diffs

- Pi adapter suite: 201 passed, 0 failed
  (CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh test -p pi-grok-adapter).
- Pager streaming suite: 22 passed, 0 failed
  (CARGO_MAINTENANCE=0 ./scripts/cargo-shared.sh test -p xai-grok-pager --lib streaming_ -- --nocapture).
  Covers native fold/Enter during file argument generation, preserved fold state
  at canonical execution start/completion, live collapsed +/- counts, and
  same-length Edit viewer updates without a resize. Existing Bash execution
  output streaming tests also pass.
- Entrypoint compile: check -p xai-grok-pager-bin --bin grok-pi passed;
  existing dead-code and non-shorthand-pattern warnings remain.
- Per-file rustfmt --check and git diff --check: passed.
- Architecture verifier: exact semantic seams and declared native renderer
  seams pass (8 modified, 3 added). Overall remains failed on the existing
  uploaded-tree/hash baseline and outdated monolithic slash/event/UI source
  expectations. Only the exact modular Edit viewer and its regression file
  were declared; the broad baseline was not reset.
- Scope: source and focused regression verification complete. No new installed
  binary, model-driven terminal acceptance, commit, push, or release.


## 2026-10-30 Beta candidate v0.1.12-beta.1

Focused checks for the release candidate:

- Remote TUI host, transport and Rust bridge: 15 passed.
- Pager Remote TUI layout/input/render tests: 23 passed.
- Pager Codemode viewer/render tests: 17 passed.
- Pager always-active picker search tests: 4 passed; feedback draft search bar: 1 passed.
- Pager side-by-side Edit diff tests: 7 passed.
- grok-pi binary: 97 passed, including the embedded Bash source-contract test for
  Pi turn-boundary `custom_message` injection.
- Pi adapter: 198 passed.
- Background Bash / Eval v2 production regression: passed.
- Every modified Rust source passes an individual rustfmt check; `git diff
  --check` and strict beta changelog extraction pass.

### 2026-10-30 Deferred Pi RPC startup — Slash Catalog handoff

- Live system Pi 1.4.0 RPC smoke (`PI_OFFLINE=1 pi --mode rpc --no-session`):
  `get_commands` returned 110 entries (50 extension, 9 prompt, 51 skill).
- Deferred attach now publishes the model catalog followed by a host-level
  `x.ai/commands/update` before queued `session/new` requests are routed. Pager
  seeds Welcome, dashboard, future-session bootstrap, and the active pre-session
  agent without relying on a session id; inactive session catalogs are retained.
- Pager settings/command handler tests: 21 passed; slash catalog sync tests:
  3 passed.
- grok-pi binary suite: 97 passed, including validation that every name in
  `PI_GROK_NATIVE_COMMANDS` resolves to an implemented Pager builtin or alias.
- `cargo-shared check -p xai-grok-pager-bin --bin grok-pi`, rustfmt, and
  `git diff --check`: passed.
- Broader `app::acp_handler::tests` filter: 521 passed, 5 failed. The five
  failures are in existing Subagents spawn/rebuild/activity tests, outside the
  changed files and already covered by the repository's broader known-baseline
  warning below; they are not represented as fixed by this change.

The unfiltered Pager suite is **not claimed green**. The integration-test target
`cargo test -p xai-grok-pager` is blocked by existing
`include_str!` references to missing
`crates/codegen/xai-grok-pager/docs/internal/25-enterprise.md` and
`22-environment-variables.md`. An unfiltered `--lib` run also surfaced
failures in unrelated subagent, ESC/input and selection tests; the run was
stopped before a complete summary. One ESC grace test reproduces when run
alone. The focused tests listed above pass; the broader failures remain open
and are not represented as fixed by this release.

Verification date: 2026-10-30

## 2026-10-30 Remote TUI fake-cursor APC

Both active frame projectors now remove Pi's hardware cursor marker and resolve
fake-cursor boundaries to ANSI SGR 7/27. Pager's existing highlight mapping is
unchanged. See [the Remote TUI issue](issues/adapter/20260719-Remote-TUI-实验轨-RPC-custom-帧投影.md).

- Bun projection/transport suites: 15 passed, 0 failed, including the default-off
  Rust TUI bridge. Regression fixtures failed on both paths before the fix.
- Freshly compiled grok-pi bin test executable: exit 0, 96 passed, 0 failed,
  including Remote TUI module-closure materialization and bridge source tests.
- Installed Pi 1.4.0 isolated RPC: real Input and Editor each passed submit and
  Escape scenarios (4/4), with letter/uppercase/Left handling, no cursor APC
  payloads in frames and visible SGR 7/27 cursors. No model inference or user
  configuration changes were involved.
- Production Rust check passed; direct `cargo-shared.sh build -p
  xai-grok-pager-bin --bin grok-pi` exited 0 and rebuilt the embedded extension.
- The new binary with system Pi 1.4.0 passed isolated PTY Input/Editor ×
  Enter/Escape acceptance (4/4), including letter/uppercase/Left input and
  repeated component open/close. Terminal output contained no fake-cursor
  payloads. The probe used the embedded host rather than loading a separate
  worktree copy of the remote-tui extension.
- `./build.sh` remains blocked: its optional pi-main coding-agent TypeScript
  build exited 2 because dependency outputs lack TuiMainScreen,
  setCapabilityOverrides, AgentToolCallOutcome and other required exports/types.
  Pi source was not changed and its Git status stayed clean. The direct Rust
  build and system-Pi acceptance above are separate passing checks.
- The original third-party Compaction model dialog was not separately handtested;
  these results do not establish universal third-party component compatibility.


## 2026-09-21 Remote TUI input ownership

Follow-up modified-key regression: the encoder returned `None` for modified
Press events, dropping Shift+S before it reached a component. Standalone
`cargo test -p xai-grok-pager --test remote_tui_keys` reproduced two failures
before the fix and passes all three tests after it. This integration target
links the production library without compiling the stale lib-test fixtures.
Coverage includes Shift+letter actions, Ctrl/Alt/Super presses, BackTab and
repeat/release behavior. The product build passed. A PTY probe loaded the actual
installed Shop `settingsPanel` with synthetic profiles: lower-case `s`, raw
uppercase `S`, and Kitty Shift+S each returned `save`. The probe stops at that
action and does not write any Shop configuration or call a model.

- `bun test extensions/pi-grok-remote-tui`: 13 passed. Covers lifecycle before
  the first frame, explicit child focus, letter/Escape delivery, shortcut
  isolation, native Pi non-interference, stale input rejection and partial JSONL.
- `cargo check -p xai-grok-pager-bin --bin grok-pi` through `cargo-shared.sh`:
  passed (existing warnings only).
- Focused Pager lib tests could not run: the test target has 110 unrelated
  compile errors, including missing timeline helpers and stale TasksPane/AppView
  test fixtures. No error in the newly added Remote TUI tests was reported.
- Adapter transport regression and embedded-extension materialization test each
  passed; `CARGO_MAINTENANCE=0 ./build.sh` passed.
- PTY smoke with system Pi 0.86.1 and the installed Curator: opened settings,
  changed language, saved with lower-case `s`, opened the model picker, searched
  `deepseek`, returned with Escape, force-closed with Ctrl+Shift+Escape, then
  typed into the restored composer. No model inference was requested.
- The smoke exposed out-of-order keyfile records for rapid typing and a delayed
  unread tail. Synchronous ACP enqueue plus ordered keyboard-notification
  handling and periodic draining fixed both in the repeated PTY scenario.
- The original user's exact third-party scene, live multi-terminal interaction,
  and full native-dialog combinations still require acceptance; this is not a
  claim that the entire Pager test suite or all plugin interactions pass.

The renderer exception is limited to letting native dialogs appear above remote
components in `agent_view/render.rs`; existing baseline hashes are unchanged.

Verification date: 2026-07-26
Delivered local main lineage: integration base `1a52f81af9d7f871f7067de35cc57756faf4bd31` (upstream merge `be91fe7`, upstream `47348d1`); restored WIP safety tip `3d4278d`

## Conclusion

The current delivery has passed production build, adapter unit tests, and native Grok architecture plus Pi protocol-contract verification. The entry point genuinely uses Grok Build's production Pager, not a standalone Ratatui/fallback/character-art frontend.

We cannot yet claim **all** verification is green. The Rust syntax stage of `verify.sh` depends on undeclared Python packages `tree_sitter` / `tree_sitter_rust`, which are missing from this environment. The native-source and renderer hash manifests, slash `fork`/`voice` rule, and one mock `agent_settled` completion-barrier expectation are stale and require deliberate baseline review; they were not broadened or regenerated during the merge. One Hooks SSRF test is environment-dependent here because the local resolver maps `.invalid` to an internal IPv6 address, and the same failure reproduces before the merge. Focused Pager lib tests now compile and pass. No new real-model PTY end-to-end smoke test has been added.



## 2026-08-22 Subagents V2 + Eval V2 production review

This review covers the opt-in Subagents V2 collaboration layer, the refactored bundled extension modules, and the current Eval V2 production regression surface. Subagents V2 remains **off by default** (`PI_GROK_SUBAGENTS_V2=1` opts in); passing static/unit checks does not promote it to default-on or replace the required real-model handtest.

| Verification layer | Result | Notes |
|---|---:|---|
| Subagents V2 coordinator/runtime tests | PASS | `cd extensions/pi-grok-subagents && bun test v2.test.ts runtime.test.ts` — 18 passed, 0 failed. Coverage includes scope precedence/malformed preset isolation, root/child routing, idle reactivation with child-session reuse, queued follow-up deferral, nested `FINAL_ANSWER`, atomic team rollback/roster registration, wait/interrupt, canonical record mutation, stable finished output, and queued cancellation. |
| Eval V2.1 production regression | PASS | `node extensions/pi-grok-bash/test-v2.1.mjs` — focused suite completed with `Eval v2.1 focused production regression: PASS`, including JS/Python all-mode host RPC parity, timeout/abort, concurrency/FIFO, background task limits, eval-v2-only tool isolation, regex tool search, and `display(image)` vision forwarding. |
| `grok-pi` binary unit suite | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi` — 96 passed, 0 failed. The embedded Subagents dependency-materialization test and Bash extension source test are included. |
| Stale Bash injector assertion | FIXED | The source test now requires `PYTHON_EVAL_WORKER_V2`, matching the authored `eval.ts` V2 Python worker selected by `PersistentEvalKernel`. Focused test passes. |
| Diff hygiene | PASS | `git diff --check` completed with no whitespace errors. |
| Extension TypeScript full check | BLOCKED (upstream baseline) | Direct `tsc` currently stops first at `TS2688` because the sibling Node type definitions are not visible; after making the `pi-main/node_modules/@types` path explicit, checking still stops in existing `pi-main` sources (including interactive footer `string`→`never` diagnostics, plus baseline target/declaration compatibility without overrides). No diagnostic from these runs pointed at `extensions/pi-grok-subagents`. The Bun tests above are the executable V2 source check for this increment. |
| Native real-model team E2E | PENDING | Before default-on/release promotion, handtest root→child, child→root, sibling messaging, nested spawn, idle follow-up/session reuse, nested final-answer wakeup, interrupt, concurrency queueing, preset override/disable, and V2-off surface absence with a real target model/provider. |

Reproducible commands for the V2-specific checks are also documented in `docs/usage/subagents-v2.md` and `docs/usage/subagents-v2.zh-CN.md`.

## 2026-07-26 Lossless Main Delivery

| Layer | Result | Notes |
|---|---:|---|
| Main history | PASS | ff-only from `906470c` to `1a52f81`; no rebase, squash, force update, or remote push |
| Restored WIP | PASS | 64 paths (33 modified, 31 added), zero SHA-256 or mode mismatches against safety tip `3d4278d` |
| Herdr extension | PASS | Node socket tests 2/2; Rust `grok-pi herdr` tests 3/3 |
| Model management | PASS | focused Pager model tests 12/12 |
| Product compile | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` |
| CLI smoke | PASS | `target/debug/grok-pi --help` |
| Recovery | PASS | two original stashes, combined/rebased safety branches, binary patch and manifest retained |

## 2026-07-18 Subagent Adaptation Increment

A built-in `pi-grok-subagents` extension was added: it creates, tracks, cancels, and persists a child `AgentSession` using the official Pi extension API, and hands it to the adapter through a `pi-grok-subagent/v1` custom-message bridge. The adapter only validates/dedupes and projects to the Pager-consumed `x.ai/session/update` and child-session-id-tagged ACP `SessionNotification`; the Pager body continues to reuse the existing SubagentBlock, Tasks Pane, child AgentView, and cancel UI.

| Verification layer | Result | Notes |
|---|---:|---|
| Pi custom-message bridge probe | PASS | RPC JSONL `message_start`/`message_end` both preserve `customType`, `display:false`, and structured `details` |
| Tempfile extension load | PASS | Copied the extension to a standalone tempfile and loaded it via `pi --mode rpc --extension <temp>.ts`; the hidden cancel command appears in the command catalog |
| Adapter unit tests | PASS | `cargo test -p pi-grok-adapter`: 53 passing |
| `grok-pi` binary unit tests | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi`: 7 passing |
| `grok-pi` check | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` succeeds; only a pre-existing `PiModel.reasoning` dead-code warning |
| Pager child-route lib test | BLOCKED | Focused test compilation blocked by a pre-existing unrelated Pager test config error: missing `set_voice_mode_enabled_for_test`, layout parameter drift, `ActiveModal: Debug`, `AppView` init field drift |
| Native TUI E2E with a real model | PENDING | Manual verification of spawn/progress/child view/finish/cancel/resume/replay is not yet done; static passes must not be treated as runtime acceptance |

## 2026-07-28 Subagent Configuration Increment

`/subagents` now displays the built-in profiles and edits only product-isolated
project/global Markdown overrides. Tools/models retain the existing Pager
QuestionView flow; extension/skill selection opens the existing Pi resource
manager in a non-mutating selection mode. `/subagent-message` and
`send_message_to_subagent` use Pi's official child-session prompt API for a
follow-up or a steer. None of this modifies Grok's original subagent
implementation: Pi remains responsible for child sessions, tools, models,
extensions, skills, and turn steering.

| Verification layer | Result | Notes |
|---|---:|---|
| Pi RPC extension-load probe | PASS | System Pi `0.82.1` loaded `extensions/pi-grok-subagents/index.ts`; `get_commands` returned both `subagents` and `subagent-message`. |
| Native QuestionView multi-select adapter test | PASS | `cargo test -p pi-grok-adapter product_multi_select_envelope_uses_native_checkbox_answer_shape` — 1 passing. |
| Pi resource picker adapter test | PASS | `cargo test -p pi-grok-adapter product_resource_picker_envelope_round_trips_selected_paths` — 1 passing. |
| Pager/resource-picker compile | PASS | `cargo check -p xai-grok-pager -p pi-grok-adapter` completed successfully; only pre-existing warnings remain. |
| Embedded extension source test | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi subagent_extension_source_is_a_loadable_typescript_module` — 1 passing. |
| Product compile and diff hygiene | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` and `git diff --check` completed successfully; existing warnings remain. |
| Extension TypeScript check | PARTIAL | No diagnostic originates in `extensions/pi-grok-subagents`; the full check is blocked by three pre-existing `pi-main` diagnostics: two stale provider model-catalog assertions and missing `highlight.js` declarations. |
| Pager unit test harness | BLOCKED | `cargo test -p xai-grok-pager …` currently fails before this picker test because an unrelated existing test lacks `handle_switch_model_complete` import in `app/dispatch/tests/session/lifecycle.rs`; the normal library check passes. |
| Native TUI real-model E2E | PENDING | Manually exercise built-in override/restore, `/subagents` selection/save, project shadowing, extension/skill picker apply/cancel, `/subagent-message`, spawn, and soft `max_turns` summary before release. |

## Executed Results

| Verification layer | Result | Notes |
|---|---:|---|
| Native Grok architecture audit | PASS | `grok-pi` lives in `xai-grok-pager-bin` and enters `xai_grok_pager::app::run_external` |
| Self-draw/fallback exclusion | PASS | adapter is library-only, no Ratatui/Crossterm/terminal loop; old `pi-grok-tui` does not exist |
| Grok native source integrity | PASS | 2696 files in the original tree remain SHA-256 identical; only 19 declared composition/ACP/state/command seams changed |
| Renderer/Input/Markdown integrity | PASS | 283 core files are byte-for-byte identical to the uploaded Grok source |
| Pi RPC command contract | PASS | all 13 RPC commands used by the adapter exist in the in-package Pi `rpc-types.ts` |
| Pi event contract | PASS | all 20 mapped lifecycle/stream/tool/queue/compaction/retry/UI event types are locatable in Pi source |
| Extension UI | PASS | all 9 methods exposed by Pi RPC have a native Grok UI route |
| Mock JSONL RPC | PASS | 27 interactions covering bootstrap, history, commands, stream, tool, UI response, and `agent_settled` |
| Rust tree-sitter parsing | BLOCKED | `verify.sh` does not declare or pre-check `tree_sitter` / `tree_sitter_rust`; the module is missing in the current environment |
| Shell script syntax | PASS | `build.sh`, `run-local.sh`, `run-installed.sh`, `verify.sh` pass `bash -n` |
| Patch applicability | PASS | `patch --dry-run -p1` against the uploaded original Grok tree applies cleanly for all 29 source/manifest files |
| `cargo check` | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` succeeds; only 1 pre-existing dead-code warning in the adapter |
| Adapter Rust unit tests | PASS | `cargo test -p pi-grok-adapter`: 17 passing |
| `grok-pi` binary unit tests | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi`: 1 passing |
| Pager focused lib tests | PASS | settings-modal suite: 173 passing, 1 ignored; `external_builtin_filter_accepts_aliases_and_omits_product_commands` and `slash_compact_with_context_enqueues_command` both pass |
| Local Pi npm build | PASS | `npm run build` succeeded in a Node.js `v24.15.0` environment |

Machine-readable reports:

- `crates/codegen/pi-grok-adapter/docs/native-grok-verification.json`
- `crates/codegen/pi-grok-adapter/docs/mock-pi-contract.json`
- `crates/codegen/pi-grok-adapter/docs/rust-syntax-verification.json`
- `verification-logs/cargo-status.json`
- `verification-logs/environment-status.json`
- `verification-logs/patch-status.json`

## Key Architecture Evidence

### Production Grok Pager Entry Point

`crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` performs only composition work:

1. Start `pi --mode rpc`;
2. Convert Pi JSONL RPC to ACP;
3. Construct `AcpConnection::external`;
4. Call `xai_grok_pager::app::run_external`.

This file creates no Ratatui `Terminal`, `Frame`, or Widget, and does not read Crossterm input.

### Native Component Reuse

`run_external` continues to use Grok's:

- terminal init/restore and writer thread;
- production event loop;
- PromptWidget and keyboard input;
- slash `CommandRegistry`, suggestion/dropdown;
- Markdown/code/diff/tool rendering;
- scrollback, find, copy, transcript, export;
- QuestionView;
- toast, sticky banner, terminal title;

so every visible terminal surface is Grok Pager, not a second TUI.

### Modification Boundaries

Grok-side changes are limited to:

- adding the external ACP connection/profile;
- gating product features of the external backend;
- Pi Extension UI notifications entering existing Grok surfaces;
- QuestionView gaining `initialText`/`noFreeform` semantic hints;
- merging dynamic Pi commands with allowed Grok builtins;
- `/compact <instructions>` parameter pass-through;

The renderer, input engine, Markdown engine, tool renderer, and minimal renderer bodies are not rewritten.

## Must Run On A Machine With The Toolchain

Requirements:

- Rust toolchain `1.92.0` (see `rust-toolchain.toml`);
- Node.js `22.19.0` or higher;
- Python 3 (for verification scripts);
- workspace dependencies installable.

Run:

```bash
./build.sh
./scripts/cargo-shared.sh test -p pi-grok-adapter
./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi
./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
```

Or run step by step:

```bash
./build.sh
./scripts/cargo-shared.sh test -p pi-grok-adapter
./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi
./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
```

Then build the full run chain:

```bash
./build.sh
```

## Runtime Acceptance Checklist

After a successful build, manually verify at least:

1. The screen, PromptWidget, command dropdown, Markdown, and tool cards match Grok Build Pager;
2. `/help` shows only allowed Grok local commands, merged with Pi dynamic commands;
3. Pi extension `notify`/`setStatus` no longer produce fallback text messages;
4. `select`, `confirm`, `input`, `editor` use the Grok QuestionView;
5. `/model` and `/effort` actually change the Pi model/thinking level;
6. a normal submission during the active turn enters Pi follow-up, send-now enters steer;
7. `!command` uses the Pi `bash` RPC and renders as a Grok tool card;
8. `/new`, `/compact instructions`, `/rename` take effect;
9. restarting an existing Pi session restores history, reasoning, images, and tool results;
10. minimal/fullscreen is selected via startup arguments, and the terminal restores correctly on exit.

## Upstream Integration Record

Date: 2026-07-17
Branch: `sync/upstream-98c3b24` (not yet merged back to `main`)

| Item | Result |
|---|---|
| Upstream tip | `98c3b24` (includes `8adf901`) |
| Strategy | Git merge with a common ancestor `c68e39f` plus seam fixes, **not** a blind merge onto main |
| `grok-pi` unit tests | 4/5 PASS; 1 item `--append-system-prompt` naming drift is a pre-existing main failure |
| Architecture invariants | adapter headless; Pager is the only TUI; Pi is the only core |

Known remaining blockers are the Python tree-sitter dependency, deliberate verifier/mock baseline maintenance, the resolver-dependent Hooks test, and manual real-model runtime acceptance described above.
