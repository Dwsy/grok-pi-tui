# Changelog

All notable changes to **grok-pi** (Pi agent core in Grok Build's production TUI) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Opt-in **Subagents V2 team collaboration** with configurable agent definitions, built-in implementation/research/review teams, team-aware runtime and transport, configuration UI, compatibility tests, and bilingual usage/architecture documentation.
- Eval Bridge v2 language selector: `[ui].pi_eval_v2_language = "js" | "py" | "all"` (default `js`, restart required), with Python and JavaScript sharing the same host-RPC, skills, completion, store/load, and task contracts.
- Eval v2 background-task parity with Bash: explicit background mode, automatic foreground-to-background promotion at the shared max-wait cap, `get_task_output` / `wait_tasks` / `kill_task`, bounded model output, and full-output spill files.
- Live Eval v2 display-mode controls, wired through settings and a native slash-command path so presentation changes can take effect without rebuilding the extension runtime.
- Stable message timestamps in the timeline, with full dates exposed on hover while preserving consistent timestamps as rows are re-rendered.
- Configurable Todo V1/V2 runtimes with cross-version migration and compatibility coverage, allowing the host to evolve Todo behavior without discarding existing task state.
- Manifest-backed extension UI registration: bundled Pi extensions can declare their F2 category/section/order metadata in `grok-pi.json`, and the host imports those registrations into the native settings surface.
- CLI leader-management commands for controlling the grok-pi leader lifecycle from the command line.

### Fixed

- Recap and `/btw` bridge traffic no longer leaks into the agent loop context: both extensions deliver summaries, deltas, and answers as `appendEntry` custom entries (never custom messages), the adapter parses the resulting `entry_appended` events, and a `context` hook strips legacy sendMessage-era entries from LLM context in old sessions.
- `tools.describe(name)` now renders nested tool schemas deeply instead of collapsing `properties` to `[Object]` in the JavaScript REPL.
- Bash `timeout: 0` now consistently means “no timeout”, matching the Eval v2 escape-hatch semantics and avoiding task validation failures from nested `tool.bash(...)` calls.
- Eval v2 background promotion immediately replaces the foreground kernel so subsequent cells are not blocked by the promoted task; task waits and timeout defaults are also hardened against foreground/background races.
- Eval v2 host-tool projection and palette startup now preserve the injected tool catalog correctly, including input-only tool arguments needed by native Pager rendering.
- The Pi Bash extension no longer registers `F2` for the Eval v2 injected-tool editor, so the extension cannot steal the host-reserved native Settings shortcut.
- PSM session database discovery, resume, and search paths now resolve correctly across supported platforms instead of assuming one platform-specific layout.
- Steering rows are held until a safe-point dispatch, preventing queued steering input from crossing adapter turn boundaries at an unsafe time.
- Model provider IDs containing `/` are resolved correctly rather than being misparsed as provider/model separators.
- Large-paste confirmation is rendered as the native prompt dropdown, and blocking question cards are cancelled with their owning turns instead of surviving teardown.
- Write/Edit hover popups remain open while the pointer is inside them and support scrolling, avoiding accidental dismissal while inspecting long tool details.
- Response streams are closed before turn teardown, preventing late stream state from racing completion; session review navigation was expanded around the same Pager lifecycle work.
- Local builds no longer require an already installed Pi runtime, removing an unnecessary bootstrap dependency from clean development environments.

### Changed

- Eval v2 `agent()` is explicitly a blocking leaf; `background=true` fails fast and concurrent leaf agents use `parallel([...])`.
- Eval v2 task status uses the same Pager-native task channel and shared wait/output limits as enhanced Bash.
- Bundled high-value Pi bridges (auth, BTW, loop, recap, Remote TUI, rollback, shortcuts, subagents, and related runtime code) were split into focused TypeScript modules with dedicated host wrappers, reducing monolithic extension entrypoints while preserving behavior.
- grok-pi startup and native settings are now driven by host/extension manifests, with runtime configuration, extension self-healing, host-feature registration, and Pi subagent transport separated from the main binary wiring.
- Session review navigation, block viewing, timeline interaction, and tool-detail inspection were refined for longer interactive sessions.
- Local Cargo development builds use faster incremental settings and shared-target maintenance, while repository reference material is consolidated under `docs/` with links updated to the new layout.

## [0.1.0] - 2026-08-21

Range: `v0.0.18` → `v0.1.0` (2026-08-18 → 2026-08-21). Tag `v0.1` points to the same release commit.

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Highlights

- **Pi built-in tool orchestration** — F2 and resource policy now coordinate Bash, Eval, Eval v2, Todo, and related injected tool surfaces instead of relying on ad-hoc extension admission.
- **Eval Bridge v2 foundation** — landed the host-RPC design, bundle verification harness, managed Bash/Eval task runtime, and native Eval display improvements.
- **Interactive subagents** — Pager subagent rows gained richer lifecycle projection and interactive session behavior while preserving Pi-owned execution.
- **Sharper file and skill workflows** — file search/preview, inline `SKILL.md` viewing, prompt/paste handling, and related Pi file interactions were expanded.

### Added

- Eval Bridge v2 design record, bundled runtime modules, demo/verification harness, host-tool bridge, task management, prompt helpers, and documentation.
- F2 controls and startup wiring for Pi built-in Bash/Eval/Todo/tool policies.
- Native Todo extension and grok-pi config skill support.
- Interactive subagent session surfaces, lifecycle metadata, and background/idle-barrier handling.
- Inline `SKILL.md` preview via Ctrl-L and additional file-search/paste interaction paths.

### Fixed

- Pi Bash extension injection now materializes the complete module dependency closure, preventing startup failures such as missing `./eval-tasks.ts`.
- Remote-TUI navigation no longer repeats held navigation keys over the bridge.
- Session/subagent projection, prompt dispatch, and tool rendering were hardened for the expanded Pi-owned runtime surfaces.

### Changed

- Bash and Eval cards use clearer native Pager formatting and status presentation.
- Session-info usage presentation and several settings/modal surfaces were polished alongside the new tool runtime controls.
- Repository guidance documents the authoritative Pi RPC bootstrap/extension-failure diagnosis flow.

## [0.0.18] - 2026-08-18

Range: `v0.0.17` → `v0.0.18` (2026-08-16 → 2026-08-18).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Added

- Richer adapter cache metrics and session information used by the Context/Usage surfaces.
- Goal-host state and notification handling needed for more complete native goal/status projection.
- Release automation support for tag monitoring.

### Fixed

- Context/Usage modal rendering, cache-graph interaction, and session-event presentation were hardened across Pager and adapter boundaries.
- User/session event blocks and scrollback state handling were refined for the newer session metadata projections.

### Changed

- Context cache graph, usage modal, ACP session metadata, and goal/status plumbing were expanded together to keep native Pager surfaces aligned with Pi-owned session state.

## [0.0.17] - 2026-08-16

Range: `v0.0.16` → `v0.0.17` (2026-08-15 → 2026-08-16).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Fixed

- The Linux ARM64 release build is now compiled natively on an arm64-hosted
  runner instead of cross-compiling from a shared 16 GB x64 runner. Cross
  compiling the large `release-dist` profile (`codegen-units=1` + thin LTO)
  there OOM-killed the job (exit 143), leaving release `v0.0.16` without a
  Linux ARM64 asset.

## [0.0.16] - 2026-08-15

Range: `v0.0.15` → `v0.0.16` (2026-08-09 → 2026-08-15).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Highlights

- **Resilient Pi runtime** — grok-pi can recover an unexpectedly exited or wedged Pi RPC child, reattach the active session, and settle background Bash rows that would otherwise remain orphaned.
- **Native grok-pi settings** — F2 now opens a tabbed, searchable grok-pi settings panel backed by the same settings registry and actions as the upstream modal.
- **Safer upstream integration** — Grok Build is synchronized through `e5fd481` while restoring Pi-Grok test adaptations, Edit coalescing, and the `/fork` picker input seam that were lost during merge resolution.

### Added

- Tabbed F2 settings grouped by category and section, with cross-tab search and pinned setting descriptions.
- A substantially expanded bundled Pi theme catalog with refreshed theme assets.

### Fixed

- Unexpected Pi RPC exits and heartbeat stalls now enter bounded automatic recovery; intentional teardowns still bypass recovery.
- Background Bash terminal state is projected out of band and de-duplicated so completed/orphaned tasks do not animate forever after RPC loss.
- Cancellation idle probes re-check ownership after `await`, preventing a stale cancel snapshot from clearing a newly started Goal successor turn.
- `/fork` picker keyboard and mouse routing is restored, including Enter/Esc, `j`/`k`, Ctrl+C cancellation escape, and Ctrl+Q pass-through.
- Upstream-extracted Pager tests retain grok-pi adaptations, and Edit coalescing call sites are restored.
- Build seams are updated for the upstream theme, event-loop, settings, and workflow API changes introduced by the sync.

### Changed

- Integrated Grok Build updates through `e5fd481` while preserving the documented Pi-Grok seams and product isolation rules.

## [0.0.15] - 2026-08-09

Range: `v0.0.14` → `v0.0.15` (2026-07-31 → 2026-08-09).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Highlights

- **Durable BTW history** — successful `/btw` answers persist as Pi-owned custom entries and `/btw-history` projects the active branch into native Pager scrollback without another model call.
- **Readable user prompts** — grok-pi user messages render as Markdown by default, with an immediate F2 toggle and persistent prompt-cursor settings.
- **Safer development builds** — shared Cargo output now has bounded parallelism, stale incremental-cache maintenance, and a configurable free-space guard.

### Added

- Active-branch BTW history replay with question/answer de-duplication, timestamps, request identity, and the selected model metadata.
- External-only `[ui].pi_user_markdown` setting, default on, preserving classic collapsible plain text when disabled.
- Persistent prompt cursor presets or a validated single-column custom character.
- Cargo disk-guard and maintenance scripts used by build, verification, generated bindings, and stop-hook checks.

### Fixed

- Cancellation now suppresses residual queued continuation events and waits for stable idle confirmation before settling the Pager state.
- BTW and recap requests retain configured model chains and extension parameters across the ACP boundary.
- Session preview scrolling persists its clamped bottom offset instead of snapping back after wheel input.
- Pi skill discovery remains Pi-owned, so skill settings and `/reload` work without restarting grok-pi.

### Changed

- Integrated Grok Build `a422116` while preserving the documented Pi-Grok seams.
- Project documentation and verification commands now consistently use the guarded shared-Cargo wrapper.

### Fixed (2026-08-09)

- Pi RPC child is now owned by a dedicated exit-coordinator channel, so `kill()` never blocks behind `Child::wait()` (previously a mutex could stall shutdown).
- Every Pi bootstrap is bounded by a 60 s deadline, so a stuck extension startup no longer hangs grok-pi; startup recovery is documented in `--help`.
- CLI tool parsing accepts `--tools=/--exclude-tools=`, and the tools extension is only injected when no explicit tool override is present.
- Review modal key/mouse/paste routing is restored (an upstream merge dropped `handle_review_key`), so Esc/q dismiss and click-to-focus work again.

### Changed (2026-08-09)

- Incremental compilation is disabled by default and the shared Cargo target is hard-capped at 64 GiB (`CARGO_TARGET_MAX_GIB`); maintenance clears legacy incremental caches first, then falls back to `cargo clean`. The disk guard enforces the cap live so a single Cargo call cannot overflow the shared target.

## [0.0.14] - 2026-07-31

Range: `v0.0.13` → `v0.0.14` (2026-07-30 → 2026-07-31).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Highlights

- **Native Pi interaction polish** — runtime model mapping, hardened ask-user responses, session cost context, workflow catalog stability, configurable thinking borders, and prompt cursor controls.
- **Remote TUI parity** — `ctx.ui.custom()` remains inline by default and now supports native Pager overlays, sizing/position metadata, and Kitty repeat/release key sequences.
- **EditTool review** — an external-only F2 opt-in renders wide EditTool diffs side by side with unified fallback on narrow terminals; code review keeps its unified dual-gutter layout.

### Added

- Side-by-side EditTool rendering with old/new columns, `-`/`+` markers, fullscreen viewer support, patch-copy preservation, and a default-off **Side-by-side edit diffs** setting.
- Remote TUI overlay placement, width/height constraints, anchors, offsets, and repeat/release input forwarding.
- Configurable thinking border colors and prompt cursor appearance.
- Session cost in context information and runtime Pi model mappings.

### Fixed

- Pi `ask_user` response handling and stale/duplicate session handlers.
- Workflow visibility during catalog reload and accidental `pi-open-tui` renderer admission.
- Native verifier accounting for the declared EditTool renderer seams.

### Changed

- Synced Grok Build through `dd04f39` while retaining the Pi-Grok native Pager seams.

### Notes

- Side-by-side EditTool diffs are Pager-owned, external-only, process-local, and default off; narrow terminals and code-review surfaces use the native unified renderer.

## [0.0.13] - 2026-07-30

Range: `v0.0.12` → `v0.0.13` (2026-07-28 → 2026-07-30).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Added

- **Q&A desktop notifications** — when an enabled native `ask_user_question` request arrives while grok-pi is unfocused, Pager sends a best-effort native desktop notification. F2 → Agent → **Q&A desktop notifications** controls it immediately; it defaults on and does not affect the Q&A tool admission setting.

### Fixed

- **External ACP startup noise** — the Pager-owned auth manager no longer logs expected missing Grok auth-file diagnostics for grok-pi's product-isolated external profile.

---

## [0.0.12] - 2026-07-28

Range: `v0.0.11` → `v0.0.12` (2026-07-25 → 2026-07-28).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Highlights

- **Native Pi model manager** — manage `models.json` in a Pager modal, then hot-reload Pi without restarting the session.
- **Product onboarding and Herdr** — a grok-pi-specific 18-part tutorial plus an opt-in native Herdr lifecycle bridge.
- **Safer product boundaries** — recap capability is advertised only when its bridge is loaded; missing bridge commands now fail explicitly.

### Added

- **`/pi-models`** (also `/model-config`, `/models-config`): native Provider → Model → Detail management for Pi `models.json`, with search, create/clone/edit/delete, validation, external-change detection, backups and restore. Saving uses Pi's official reload path; activating a model continues to use typed ACP `session/set_model`.
- **grok-pi tutorial profile**: `/tutorial`, `/tour`, and `/onboarding` now present 18 product-specific chapters covering native Pager surfaces, Pi capabilities, optional bridges, and their boundaries rather than reusing stock Grok copy.
- **Herdr lifecycle integration**: an F2-controlled, restart-required **Pi Herdr integration** injects a host-owned extension to report root Pi session identity and working/blocked/idle state. It is inert outside Herdr and can be disabled with `[ui].pi_herdr = false`.
- **Subagent session isolation**: subagent session files are created below the parent session directory's `subagent/` tree.

### Fixed

- **Recap and bridge commands** — only advertise session recap when its injected extension is present; reject requests for unloaded bridge commands and prevent concurrent recap requests.
- **Thinking stream rendering** — strip complete ANSI control sequences while retaining incomplete sequences across chunks, so terminal escapes do not leak into thinking text or Rust code fences.
- **Startup noise** — no longer print a successful Pi host version check to stderr.

### Changed

- Integrated Grok Build `47348d1` through the documented changelog-first, isolated sync workflow; retained the narrow Pi-Grok seams and shared Cargo-target setup for linked worktrees.
- README, feature matrix, architecture notes, and bilingual Herdr usage guides now document the new product surfaces and opt-in policy.

### Notes

- The model manager deliberately does not introduce a synthetic enabled/disabled model state: model availability and authentication remain Pi-owned.
- Herdr and recap bridges require a full restart after their extension admission settings change.

---

## [0.0.11] - 2026-07-25

Range: `v0.0.10` → `v0.0.11` (2026-07-25).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Fixed

- **Release completeness** — include the local Pager appearance, settings, router, and renderer sources required by the bash run-display integration so all release targets compile from the tagged checkout.

## [0.0.10] - 2026-07-25

Range: `v0.0.9` → `v0.0.10` (2026-07-24 → 2026-07-25).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Fixed

- **Session replacement crash** — shortcut-manager no longer retains a stale Pi extension context in delayed callbacks after a session reload, fork, or switch.
- **Pi RPC diagnostics** — complete child stderr is appended to `$GROK_HOME/logs/pi-rpc-stderr.log`, preserving uncropped Node stack traces when terminal error surfaces are narrow.

## [0.0.9] - 2026-07-24

Range: `v0.0.8` → `v0.0.9` (2026-07-22 → 2026-07-24).

> **中文版本：** [CHANGELOG.zh-CN.md](docs/CHANGELOG.zh-CN.md)

### Highlights

- **Transparent-theme wave accent works again** — running tool / thinking `┃` breathing animation no longer freezes on `pi:transparent` / terminal-native canvas
- **Session surfaces** — cache graph in Context, `/review-session` / `/review-message`, session tree map
- **Native bridges (F2, mostly off by default)** — ask-user QuestionView, `/btw`, `/loop` scheduler
- **Adapter fidelity** — live `promptId` on every ACP notification; bash/Execute mid-command `output_delta` streaming
- **Upstream** — Grok Build `a5727c5` integrated with Pi-Grok seams preserved + post-merge loss restore
- **Windows / multi-arch install** — reliable Pi host shim resolution; installers and release matrix cover x86_64 + aarch64 on macOS / Linux / Windows

### Added

#### Context, review, and tree

- Context modal **cache graph** (`F2` `[ui].pi_cache_graph`, default **on**): adapter projects `cacheMetrics` from Pi `get_entries`; views `0/1/2/3`, `s` sort, `e` export, `r` refresh — no `ctx.ui.custom`
- **`/review-session`** and **`/review-message`**: native Pager review modal (file list + BlockViewer diff preview); F2 `review_file_tree` default **off**; `t` toggles tree in-modal
- Session **tree map** surface for branch orientation (alongside existing Session Tree navigation)

#### Extension bridges (opt-in F2 / inject)

- **Native Q&A** — F2 `[ui].pi_ask_user_question` (default **off**, restart): `ask_user_question` tool → multi-question `x.ai/ask_user_question` → native QuestionView; control-dir completes the tool. Host conflict list in `assets/native_feature_conflicts.toml` (+ `$GROK_HOME` / project overlays)
- **`/btw`** — F2 `pi_btw` (default **off**): side-question path via adapter `x.ai/btw` + `pi-grok-btw` (not juicesharp overlay remapping)
- **`/loop` scheduler** — F2 `[ui].pi_loop` (default **off**, restart): `scheduler_create` / `delete` / `list` → native `ScheduledTask*` / tasks pane; session-only (no durable loop subagent)
- Slash **`getArgumentCompletions`** bridge so extension commands (e.g. `/gapp`) populate the Grok argument dropdown; `/model` completion tokens aligned with Pi `provider/id`
- Experimental **rust-tui bridge** path (comment hygiene only in this tag); shortcut-manager / remote-tui snapshots archived under `extensions/_archived/`

#### Adapter / queue / tool streaming

- Stamp client **`promptId` on every live ACP `SessionNotification._meta`** so Pager prompt-id gate and turn chrome match stock Grok shell
- **Pin `runningPromptId`** via `QueueMirror::set_running` on primary `session/prompt` (idle primary never only in Pi steering/follow-up arrays); rebroadcast before first Pi event so queue adoption sees a server-side running id
- Convert Pi growing full-text **`partialResult` → `BashOutput.output_delta`** so Run/bash cards stream mid-command instead of jumping only at tool end

#### Resources, telemetry, website

- Project **resource policy** + self-heal crash report path
- **`tools/ext-crash-telemetry`**: CLI + Cloudflare Worker + dashboard for extension crash reports (optional ops tooling)
- Website: **static export** deploy path for GitHub Pages; basePath-aware `/docs` links; EN/ZH docs dictionaries expanded (features, migration, extensions, configuration)

#### Platform

- Windows: resolve bare `pi` / `pi.cmd` to absolute host shims (PATH + pi-node/npm); spawn `.cmd` via `cmd.exe`; rewrite `args.pi_bin` after version probe
- Release / install matrix: macOS, Linux, Windows × x86_64 + aarch64 assets documented and built

#### Upstream

- Integrate Grok Build **`a5727c5`** (`Synced from monorepo`); record in `docs/upstream/UPSTREAM_CHANGELOG.md`; AGENTS `base` updated after verified merge
- Post-merge **auto-merge loss restore** on narrow Pi-Grok seams (render/effects/shortcuts/shell ops)

### Fixed

#### Wave accent on transparent themes (user-visible regression)

- **Root cause:** transparent / terminal-native themes set `Theme.bg_base = Color::Reset`. Running accent waves call `blend_color(bg, accent, wave_brightness)`. `blend_color` previously returned `None` for `Reset`, and callers used `unwrap_or(accent)` → **static bar every frame** (looked like “no breathing”)
- **Fix:** `xai-grok-pager-render` `blend_color` maps `Color::Reset` to a synthetic dark canvas `(0x12, 0x12, 0x18)` for lerp only (page stays transparent; no forced opaque fill). Named ANSI colors remain non-blendable
- **Regression test:** `test_blend_color_reset_base_keeps_wave`
- **Also:** `EntryRenderer` always applies animated `accent_running` when `entry.is_running` even if block `accent()` returns `None` (Collapsed defaults for Read/Search/Edit/Thinking)

#### Other

- Resume picker: full-text search, fork tree, preview mode, key hints polish
- Sync seam restore after `a5727c5` integration (dashboard render, shortcuts help, shell mvp ops)
- Website docs links under GH Pages `basePath`
- rust-tui-bridge comment: drop accidental keyfile-substring noise

### Changed

- FEATURE_MATRIX / README (EN + ZH) aligned with session tree, review, queue, Q&A, btw, loop, cache graph, notify→scrollback-only for multi-line info
- Info notifications: prefer **scrollback `SystemMessage`** for multi-line / Pi `showStatus` style (avoid toast-only loss)
- Docs start path simplified to **`grok-pi` / `pi-grok`** only
- `.gitignore`: local fabric mesh runtime state
- Upstream workflow docs: changelog-first, then isolated merge + seam reapply

### Notes

- F2 features that inject extensions (**ask-user**, **btw**, **loop**, **workflows**, **goal**) need **full quit + restart** after toggle
- Transparent themes: wave uses synthetic blend canvas for motion only; UI chrome stays host-transparent
- Investigation notes (optional): `docs/investigation/breathing-animation-debug.md`
- Upgrading from **0.0.8**: no migration step; transparent-theme users should see breathing again without switching theme
- GitHub Release notes for 0.0.9 still default to cumulative sections since **0.0.6** via `scripts/extract-changelog-section.py`

---

## [0.0.8] - 2026-07-22

Range: `v0.0.7` → `v0.0.8` (2026-07-21 → 2026-07-22).

### Added

#### Product state isolation

- Default user home is now **`~/.grok-pi`** (`$GROK_HOME`) instead of sharing stock Grok’s `~/.grok`
- Default project config tree is **`<repo>/.grok-pi`** (`$GROK_PROJECT_DIR`); paths resolve via `xai_grok_config::project_config_dirname()` / `project_config_dir`
- Startup injects home + project dirname before any library pins `grok_home()` via `OnceLock`
- **`grok-pi migrate-home`**: copy allowlisted files from legacy `~/.grok` (or `$GROK_LEGACY_HOME`) with `--status` / `--dry-run` / `--force` / `--from` / `--into` / optional `--include-auth`
- Safe **auto-migrate once** when target home is empty and legacy has data; writes `.migrated-from-legacy` marker
- **Not** dual-scanned: stock `~/.grok` / `<repo>/.grok` are not auto-read for discovery (true isolation)
- Unit tests without env keep stock `.grok` default so upstream-style tests stay green

#### Pi workflows (Rhai, F2 off by default)

- Reuse upstream **`xai-workflow`** engine + shell orchestration; **SpawnBackend** pluggable (`Grok` default / `Pi` bridge for grok-pi)
- Adapter: `WorkflowHost`, `pi_workflow_backend`, ACP `x.ai/workflow/{launch,pause,stop}`, `x.ai/workflows/list`, `workflow_updated` notifications
- Injected extension + slash: `/workflow`, `/workflows`, `/create-workflow` (+ named scripts); bridge cmds `__pi_workflow_*` filtered from catalog
- F2 **`[ui].pi_workflows`** default **off**, restart required to inject extension
- Project scripts live under `<repo>/.grok-pi/workflows` (and user `~/.grok-pi/workflows`)
- `/create-workflow` is Pager PassThrough user prompt (not a Pi skill)

#### Goal mode (F2 off by default)

- F2 **`[ui].pi_goal`** default **off**, restart required
- Injected extension: `/goal` + `update_goal` tool + control file
- Adapter **`GoalHost`**: state machine → native `GoalUpdated` (`goal_detail` / status bar)
- Active goal continues via `agent_settled` follow-up (not full shell multi-agent classifier/planner/strategist stack)

#### Export / share

- Default-on **`pi-grok-export`**: `/export-html` (Pi HTML or `.jsonl` path) and `/pi-share` (private gh gist + pi.dev viewer)
- Grok `/export` remains Markdown transcript; no second TUI

#### Website & CI

- Marketing + docs site under **`website/`** (Next.js 15, EN/ZH, landing + docs)
- GitHub Actions **`Website`** workflow: `workflow_dispatch` only; `npm ci` + `npm run build`; uploads standalone / static / public artifacts
- Release workflow: **Swatinem/rust-cache** for faster multi-platform builds

#### Session picker & theming

- Session picker **full-text search** (`Ctrl+F`) for external Pi sessions; dedicated search page on the external catalog, state reset on each toggle
- Transparent themes (`pi:transparent` / `pi:transparent-light`): code blocks use terminal default background (`Color::Reset`); user messages get a **left accent bar** when canvas bg is Reset
- Transparent theme JSON assets + Pi theme map (`theme/pi/map.rs`) updates for code/user surfaces
- Pager app/view/dispatch wiring for session load, jump, modals, welcome, and overlay list polish that landed with the picker/search work

#### Adapter / queue (pre-isolation wave)

- `pi-grok-adapter` model / RPC / session / queue bridge updates: richer session catalog fields, queue mirror alignment with Pager optimistic dequeue, RPC client/session path hardening for workflow/goal hosts that follow

#### Upstream & tooling

- Sync Grok Build **`3af4d5d`** (`SOURCE_REV` `0f4d7c91`): monorepo pull including new **`xai-workflow`** crate, workspace permission/security overhaul (exec-risk, auto-mode, hardened shell), shell working-directory relocation, doctor/terminal-setup surfaces, prompt-queue batching knobs — full list in `docs/upstream/UPSTREAM_CHANGELOG.md`
- **`upstream-changelog`** skill (`.pi/skills/upstream-changelog/`) + first structured entry for 3af4d5d; AGENTS.md documents two-phase sync (changelog first, merge second)
- `pi-main` submodule advanced to latest **earendil-works/pi** `main` (`a5afc3f1`); **local dirty RPC edits in the submodule are rejected** — pin only
- Release CI: **Swatinem/rust-cache**; release notes body built from `CHANGELOG.MD` via `scripts/extract-changelog-section.py` (strict, optional cumulative `--since 0.0.6`, install footer) then GitHub auto commit list

#### Docs / legal

- Queue architecture redesign research: `docs/issues/queue-architecture-redesign.md`, `queue-redesign-feasibility.md` (Pager-owned queue / eliminate multi-layer reconcile — research only, not shipped)
- Isolation / workflow / goal Issues under `docs/issues/架构/` and `docs/issues/adapter/`
- LICENSE: retain SpaceXAI upstream Apache-2.0; note Dwsy fork copyright for modifications
- README / FEATURE_MATRIX / AGENTS bilingual updates for product isolation, workflows, goal, export

### Changed

- Adapter model / RPC / session / queue bridge updates for upstream + workflow/goal hosts
- Pager app logic, jump/session picker views, dispatch, effects, and `grok-pi` bin wiring for new F2 gates (`pi_workflows`, `pi_goal`) and extension injection order
- `install.sh` / path helpers respect product-isolated homes where applicable
- Slash command registration: external-profile gating consistency across Pager-local commands; `/doctor` treated as pager-native terminal diagnostics

### Fixed

- `pi-grok-bash` extension updates aligned with background-task kill control path (`x.ai/task/kill`)

### Notes

- Local dirty edits to **`pi-main`** source are **not** allowed; submodule is a clean pin only
- F2 **Pi workflows** / **Pi goal** require full quit + restart (extension inject at process start)
- Uncommitted WIP (e.g. `/review-session` polish) is **not** part of this tag unless landed before cut
- Upgrading from **0.0.6**: read **[0.0.7]** as well (plan mode, `/jump`, `/fork`/`/clone`/`/reload`, resource policy, Remote TUI, tree rollback, update proxy) — GitHub Release notes for 0.0.8 embed both sections by default

---

## [0.0.7] - 2026-07-21

Range: `v0.0.6` → `v0.0.7`. (Previously omitted from this file; reconstructed from git history.)

### Added

#### Plan mode (native Pager ↔ Pi)

- Pager-native Plan toggle bridged to adapter-owned state machine: `Inactive` / `Pending` / `Active` / `ExitPending`
- Session-private `.plan.md` sidecar + `.plan-mode.json` persistence
- Injected tool gate blocks `edit` / `write` / `bash` except the plan file while active
- Pi `exit_plan_mode` → native `x.ai/exit_plan_mode` approval surface
- Full/sparse system-reminder prefixes depending on mode phase
- Keyboard chord **Ctrl+Shift+T** toggles plan mode; plan file path published for `/view-plan`

#### Session navigation & tree

- **`/jump`**: native turn picker with timeline previews, compact wall-clock time (`HH:MM`), viewport restore
- External profile: empty **double-Esc** and **`/rewind`** open **SessionTree** (Pi tree navigation, not Grok destructive rewind)
- SessionTree selected-row highlight aligned with Pi selection semantics
- **Tree file rollback** (F2 `pi_tree_file_rollback`, external-only, restart required): SessionTree `r` = preview, `R` = execute; checkpoint preimages for write/edit via `pi-grok-rollback` + adapter `pi/session/rollback_preview|execute`

#### Pi session branch ops

- **`/fork`** (external): jump-style `ListOverlay` from RPC `get_fork_messages`; `fork` creates branched session file; same agent rebinds `sessionId` + `session/load` replay; selected text prefills prompt
- **`/clone`** (external): RPC `clone` duplicates current leaf; rebind + replay; prompt cleared (Pi parity)
- Grok peer-agent `/fork` unchanged for non-external profiles

#### Reload, hotkeys, session alias

- **`/reload`**: `__pi_reload` → `ctx.reload()`; blocks while **streaming or compacting**; adapter refreshes command/model catalogs; Pager rediscovers Pi themes and re-applies active `pi:*` theme; toast copy matches Pi interactive
- **`/hotkeys`** (aliases `shortcuts` / `keys`): opens native ShortcutsHelp modal (same as Ctrl+.)
- **`/session`** alias for `/session-info`; payload can include `session_file` + message/turn/tool-call counts

#### Resource policy & CLI

- **Pi resource admission policy** (`ResourcePolicy`): allow/block lists + heuristics (e.g. block `pi-tool-display`, custom header/footer); applied at `grok-pi` startup
- Startup disables Pi auto-discovery (`--no-extensions/skills/prompt-templates/themes` as needed) and injects only policy-approved resources via explicit flags
- Catalog discovery aligned with Pi package auto-entry rules; package dirs expand to declared entry only (not nested private modules)
- `/pi-config` modal: All/Enabled/Disabled filter, policy view (`a`), refresh (`r`), Tab scope, expanded shortcuts; F2 Enter on `pi_config` opens modal
- CLI tool flags: `--no-tools` / `-nt` / `--no-builtin-tools` / `-nbt`, `--exclude-tools` / `-xt`, env `PI_GROK_EXCLUDE_TOOLS`
- First-class Pi flags forwarded: model, session, prompts, resources, tools, trust/network; args after `--` still passthrough

#### Remote TUI, auth, recap

- Remote TUI host uses official Pi **`rpc-entry.js`** + extension mode facade (`ctx.mode` appears as `tui` to extensions while transport stays RPC) — **no Pi source fork**
- `pi-grok-remote-tui`: multi-select capability lab (header/footer widgets, status, title, editor text); overlay stacking with restore
- `pi-grok-auth`: fix nested `openCustom` tearing down LoginDialog; `showOverlay` / `showAuthPrompt` / `prompt.signal`
- `pi-grok-recap`: optional **Mermaid** via F2 `recap_mermaid`; markdown cleaner preserves inner mermaid fences; recap body rendered as Markdown

#### Settings (external-only gates)

- `SettingMeta.external_only`: rows hidden unless grok-pi / external profile active
- F2: `pi_tree_file_rollback` (restart-required), `recap_mermaid`, `remote_tui_footer`
- `EXTERNAL_AGENT_ACTIVE` atomic set once at startup from UI profile

#### Timeline / theme render

- Timeline rail glyphs (chevron, heavy/light horizontal, active/hover ticks) with ConHost fallbacks; horizontal strokes for Pi visual parity
- Pi theme registry **`rediscover()`** after `/reload`

#### Update channel

- GitHub release discovery via **JSP proxy** fallback (`jsp.dwsy.link`) to avoid unauthenticated API rate limits; fall back to `api.github.com`

### Fixed

- Expanded edit card: throttle redraw when in viewport; sticky-header cache + edit gutter height estimate
- Live-turn sticky suppress + Pi-aligned spinner
- Version compare: ignore dirty prerelease; use `+dirty` build metadata so local builds do not falsely report “update available”
- Package extension discovery: expand package dirs as Pi auto-entries (manifest entry only)
- Bash task kill: `x.ai/task/kill` reaches Pi Bash extension with `runningTaskIds` validation (`KillOutcome`)
- Widget rendering alignment + picker `label_color` polish

### Changed

- Adapter: Pi RPC stderr ring buffer diagnostics; rustfmt cleanup
- Bilingual README streamlining; Remote TUI docs / flow demo
- Upstream monorepo syncs absorbed during the window (see git `Synced from monorepo` commits)

### Docs

- FEATURE_MATRIX rows for `/fork` `/clone` `/reload` `/hotkeys` `/session`
- Resume preview & search alignment issue specs

---

## [0.0.6] - 2026-07-20

### Added

- PSM session catalog in the adapter with enriched metadata and tree editor text support
- Session picker sorting, expand-all, rich metadata display, and tree navigation

### Fixed

- Model picker height is now constrained to the available viewport
- Remote TUI paste events are forwarded correctly
- Subagents live traffic uses `appendEntry` with simplified recap emit

## [0.0.5] - 2026-07-18

### Added

- Native Pi resource manager for extensions, skills, prompts, and themes
- Context visualisation bridge with native Pager rendering
- Session recap, timeline, and Pi model-selection integration improvements

### Changed

- Welcome hero card now uses the `grok-pi` name, product version, and Pi description
- Release builds, update checks, and update installs use only `GROK_PI_VERSION`

### Fixed

- Upstream Grok `GROK_VERSION` and workspace version no longer affect `grok-pi` version display or update comparison

## [0.0.4] - 2026-07-17

### Added

- Context click / `/context`: adapter implements `x.ai/session/info` from Pi `get_session_stats` + message estimate → native `ContextInfoBlock`
- Queue pane bridge: Pi `queue_update` full arrays → `x.ai/queue/changed` so optimistic dequeue works
- Product version stamp via `GROK_PI_VERSION` / git describe (no more upstream `0.1.220-alpha.*` on `--version`)

### Fixed

- Context breakdown no longer dumps the whole window into Reasoning/overhead when message estimate is empty (fallback → Messages)
- Session-info payload omits null Option fields for safer pager deserial
- Welcome / dashboard / session load paths for Pi catalog continuity

## [0.0.3] - 2026-07-17

### Changed

- Update check/install is **GitHub-only** (`Dwsy/grok-pi` releases JSON + install.sh/ps1)
- Removed npm registry fallback (unscoped `grok-pi` is a foreign package; scoped package not published yet)

## [0.0.2] - 2026-07-17

### Added

- Native Welcome screen with Pi π block logo (default start; `-c/--continue` resumes)
- Agent Dashboard adaptation: `/dashboard` · Ctrl+\ · Pi session catalog → dormant roster
- Update check & install:
  - Sources: GitHub `Dwsy/grok-pi` releases JSON (npm fallback removed in 0.0.3)
  - CLI: `grok-pi update`, `grok-pi update --check`, `grok-pi update --to 0.0.2`
  - Welcome **Ctrl+U** runs the same installer after quit-for-update
- Welcome **Changelog** opens this file on GitHub
- Welcome **Resume session** via native SessionPicker (Pi catalog)

### Changed

- Welcome **New worktree** hidden for Pi (no Grok worktree product path)
- Canonical GitHub repo: `Dwsy/grok-pi`
- Release workflow stamps `GROK_VERSION` from the git tag

## [0.0.1] - 2026-07-17

### Added

- Initial `grok-pi` composition: Pi JSONL RPC ↔ ACP adapter + Grok Pager TUI
- Install scripts and multi-platform GitHub release packaging
