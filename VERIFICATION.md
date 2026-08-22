# Grok Native TUI × Pi Verification Report

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
