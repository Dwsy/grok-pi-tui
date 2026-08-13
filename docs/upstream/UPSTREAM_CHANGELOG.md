# Upstream Changelog

Changelog of upstream **Grok Build** (`xai-org/grok-build`) changes absorbed by
this fork (`Dwsy/grok-pi`). This is the **upstream update record**: it lists what
upstream changed and which features were affected, so each sync can be reviewed
before and after the merge.

> [!NOTE]
> Upstream commits are titled `Synced from monorepo` but each carries a full
> **`Changes:`** bullet list and a **`Source-Revision:`** trailer in its message
> body. Feature descriptions below are **transcribed from those commit messages**
> (the authoritative source). Diff analysis is used only to fill the Areas-touched
> statistics and to derive descriptions for the rare commit that lacks a
> `Changes:` list.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Entries are **newest first**. This file is maintained by the
[`upstream-changelog`](../../.pi/skills/upstream-changelog/SKILL.md) skill.

## Entry schema

Each entry records:

| Field | Meaning |
|---|---|
| Upstream tip | Full upstream commit SHA being synced to |
| Range | `FROM..TO` git range (`merge-base..upstream-tip`) |
| SOURCE_REV | Monorepo revision from the `Source-Revision:` trailer / `SOURCE_REV` file at the upstream tip |
| Date | Date the record was generated (YYYY-MM-DD) |
| Stats | Files changed, insertions(+), deletions(−) |
| Added / Changed / Fixed | Feature bullets transcribed from upstream commit `Changes:` lists |
| Areas touched | Per-crate/area change statistics table (from `git diff --numstat`) |

<!-- entries below this line -->

## [e5fd481] — 2026-08-13

> **Status:** Pending — not yet merged into grok-pi.

- **Sync range:** `75e73f3..e5fd481` (`75e73f3d6ac0350d211f12ae7d57c2c0aad72576` → `e5fd4816d43260c15ba785f103990c1ed6cea230`)
- **Upstream commits:** 3 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `ea094a8c369475f97c85540d01730baec0dce5d6` (was `a61c32b12a2b400f212221cd8762e05f9b36828d`)
- **Diff size:** 643 files changed, +148800 / −110648

### Summary

Three monorepo syncs land a live presence protocol, a `/rename` overhaul, and a
large round of subagent, permission, image, and cancellation hardening. Shell
and Pager together account for the bulk of the diff, with Workspace/Permission
third; the biggest new surfaces are workspace bindings (`EnsureBinding`,
`MergeToMain`, `Push`), `GROK_SESSION_ID` propagation into tool commands and MCP
servers, and MCP protocol version `2025-11-25`. For grok-pi the sensitive part
is not the feature list but the location: subagent stop/cancel, queue promote,
composer paste/type-ahead, and dashboard shortcuts all sit on Pi-owned seams.

This entry starts at `75e73f3` rather than `a5589e9` because the earlier
`a5589e9..75e73f3` range was already recorded and integrated on
`sync/upstream-75e73f3`, which is being delivered as the base of this sync.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 205 | +48473/−35942 | 53/0 | presence protocol, subagent lifecycle, auth straddle, image budgets, doom-loop defaults |
| Pager (TUI) | 178 | +43178/−35189 | 15/0 | `/rename`, `/session-info` copy, composer paste, subagent overlay stop, selection fixes |
| Workspace / Permission | 58 | +19486/−13516 | 11/0 | `EnsureBinding`/`MergeToMain`/`Push`, provisioned-mount walk, auto-mode grant handling |
| Other crates | 72 | +10359/−6958 | 22/0 | foreign sessions, session search, tty utils, fsnotify and supporting infrastructure |
| Textarea / Inline | 4 | +6360/−6244 | 1/0 | soft-wrap word selection and composer editing |
| Chat state | 14 | +3805/−3356 | 2/0 | chat-kind plumbing behind `/rename` |
| MCP | 3 | +3319/−3282 | 1/0 | protocol version `2025-11-25` |
| Computer Hub | 5 | +2981/−2962 | 1/0 | hub-side supporting changes |
| Update / Version | 8 | +2994/−2498 | 1/0 | native arm64 install, alpha-channel failure guidance, install telemetry |
| Tools | 24 | +2022/−185 | 0/0 | stale live children, ripgrep reaping, `GROK_SESSION_ID` |
| Worktree / GC | 6 | +1715/−14 | 2/0 | narrowed standalone origin fetch, no inconsistent shallow clones |
| Models / Sampling | 17 | +1477/−89 | 0/0 | catalog refresh keeps the displayed session model |
| Hooks / Plugins | 6 | +708/−91 | 0/0 | first stderr line on hook failure |
| ACP / Protocol | 7 | +403/−49 | 0/0 | protocol plumbing for the above |
| Agent lifecycle | 12 | +302/−120 | 1/0 | child-process wait replacement |
| Telemetry / Mixpanel | 10 | +359/−57 | 0/0 | CLI update install telemetry |
| Sandbox | 6 | +382/−13 | 2/0 | allow-path `/**` normalization |
| Root / meta | 3 | +228/−64 | 0/0 | lockfile and upstream revision metadata |
| Config | 3 | +164/−9 | 0/0 | supporting configuration changes |
| Prod / release assets | 2 | +85/−10 | 0/0 | release asset naming |
| **Total** | **643** | **+148800/−110648** | **112/0** | |

### Added

- Presence protocol end to end: live presence updates flow through the gateway into client presence tiers.
- Workspace: `EnsureBinding`, `MergeToMain`, and `Push` operations, plus start-from-bindings.
- Workspace: report the workspace server version in `workspace.info`.
- Tools: set `GROK_SESSION_ID` for tool commands and MCP servers.
- `/rename`: title cap, ghost prefill, and cross-host manual titles.
- Pager: copyable `/session-info` with per-row click-to-copy and copy-all.
- Pager: paste and drag images into the composer while the prompt is unfocused.
- Pager: accept the legacy Ctrl+4 shortcut for opening the dashboard.
- Pager: show an interjection note on send-now turns.
- Pager: typed Automations tool-usage card variant with client render support.
- Prompt: add a communication section to the system prompt.
- Prompt: render the browser verification policy in the prompt template.
- Plan: add UI verification and project rules to `grok-build-plan`.
- Telemetry: emit CLI update telemetry for install attempts.
- Voice: preset voices for `reference_to_video` via grok-imagine-video-1.5.

### Changed

- Prompt: remind the model to finish previous work on a mid-turn send.
- Prompt: rename the system prompt `<output_efficiency>` section to `<response_guidelines>`.
- Prompt: strengthen agent work discipline.
- `/rename --auto` unpins a manual title.
- Runtime: cap and pre-warm the tokio blocking pools to stop EAGAIN aborts.
- Shell: replace signal-based child process waits.
- Shell: default the doom-loop `max_threshold` to 32.
- Shell: default `prompt_cache_key` to the conversation id on the Responses backend.
- Shell: skip the sessions-root scan on live subagent spawn.
- Shell: share image byte budgets with compaction.
- Shell: capture and persist bound image capabilities.
- Shell: preserve Auto-mode user intent context.
- Shell: hide zero-score first-turn memory.
- Shell: surface the first stderr line on hook failures.
- Shell: disable inline citations for backend search.
- Worktree: narrow the standalone origin fetch and drop inconsistent shallow clones.
- Pager: group hooked read-only tool calls.
- Pager: show tool-call writing activity instead of waiting for the response.
- Pager: name the subagent in the wait spinner and count parallel prompt writes.
- Pager: `[stop]` in a subagent overlay cancels the child turn, and subagent-view `[stop]`/Ctrl+C kills the focused subagent.
- Pager: frame a post-cancel follow-up like an interjection.
- Pager: enable display-refresh auto-cadence by default.
- Pager: spawn the history-search matcher thread lazily instead of per prompt.
- Pager: explain the startup timeout.
- MCP: advertise protocol version `2025-11-25`.
- Video tools: explain the ZDR restriction instead of dropping the tools.
- Update: point alpha update failures at a `GROK_CHANNEL` reinstall.

### Fixed

- Security: restrict session directories to owner-only permissions.
- Security: auth straddle hardening with a consumed-token sentinel and failure poisoning.
- Permission: honor explicit user grants and narrow allow rules in auto permission mode.
- Sandbox: strip a trailing `/**` on allow paths.
- Update: install native arm64 on Apple Silicon, including from Rosetta shells and x86_64 updaters.
- Models: retain the displayed session model when the model catalog refreshes.
- Images: recognize invalid-image rejections by the server's error code, and heal sessions poisoned by invalid images by persisting the strip.
- `/rename`: remote revert, chat-kind plumbing, and doc fixes.
- Tools: finalize stale live children and return wait when already terminal.
- Tools: wake the model when a UI kill never delivered a tool result.
- Tools: kill and reap ripgrep children when tool calls are cancelled.
- Workspace: walk every provisioned mount for prompt, graph, and fs-notify.
- Workspace: make boundary commits dirty trees for real.
- Pager: paste Flameshot image-only screenshots.
- Pager: Apple Terminal Cmd+click opens the first autolink across messages.
- Pager: finish drag selection when the mouse release is lost, and select wrapped words across soft-wrap rows.
- Pager: Esc on the cancel-subagents panel keeps the turn running.
- Pager: fire the permission-prompt notification only on a real UI wait.
- Pager: preserve startup type-ahead into the composer.
- Pager: block queue promote while the front row is under edit.
- Pager: resolve relative markdown links to existing cwd files.
- Fix `PromptMetadata` construction under feature unification.

### Removed / Deprecated

- Retire the grok-code Direct Mode documentation.

### Merge risk for grok-pi

Static three-way preview (`git merge-tree --write-tree`) against the delivery
base predicts **22 conflicted files** — 21 content conflicts plus one
modify/delete (`tests/pty_e2e_shell_tools.rs`, deleted locally, modified
upstream). Hotspots, in seam order:

- **Pager app root and dispatch:** `app/actions.rs`, `app/app_view.rs`,
  `app/event_loop.rs`, `app/effects/mod.rs`, `app/dispatch/router.rs`,
  `app/dispatch/session/modal.rs`, `app/agent_view/input.rs`. These carry the
  freshly added `Action::OpenPiSettings` seam, so resolution must keep both the
  upstream action surface and the grok-pi settings entry point.
- **ACP mirror:** `acp/model_state.rs`, `acp/tracker.rs` — upstream's
  catalog-refresh model retention meets the adapter-driven model state.
- **Views:** `views/dashboard/{render,state}.rs`, `views/block_viewer.rs`,
  `views/shortcuts_help.rs` — Ctrl+4, subagent stop, and shortcut listings.
- **Selection/scrollback:** `scrollback/state/selection.rs`,
  `scrollback/blocks/tool/edit.rs` — soft-wrap word selection.
- **Shell/workspace:** `shell-base/util/grok_home.rs` (product state isolation
  — must keep `$GROK_HOME` → `~/.grok-pi`), `session/acp_session_impl/laziness_classifier.rs`,
  `workspace-types/src/rpc/workspace.rs` (new binding operations).

Upstream's `views/settings_modal` is untouched by grok-pi, so the new
`views/pi_settings` panel adds no conflict there; the risk is concentrated in
the action/router/modal wiring instead.

## [75e73f3] — 2026-08-10

> **Status:** Merged — integrated on `sync/upstream-75e73f3` as merge commit `dc690ac2139cd62bf7c44400da704e3dc5ff52b9`; validation was static-only (no Cargo).

- **Sync range:** `a5589e9..75e73f3` (`a5589e958437d79e13db026eedcb1720bffd4063` → `75e73f3d6ac0350d211f12ae7d57c2c0aad72576`)
- **Upstream commits:** 4 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `a61c32b12a2b400f212221cd8762e05f9b36828d` (was `4d6d11372ab8f73026a78c45a7b7e7b1310eb39f`)
- **Diff size:** 406 files changed, +36871 / -12418

### Summary

Pager/Shell lifecycle, rewind, usage UI, memory trace, worktree, MCP and task/subagent safety dominate this range. It overlaps 89 locally changed files and predicts 18 content conflicts, so integration stays isolated and preserves Pi-owned agent/session/queue semantics.

### Areas touched

| Area | Files | +/- | Added / Deleted |
|---|---:|---:|---:|
| Pager (TUI) | 167 | +14563/-3497 | 19/0 |
| Shell (agent runtime) | 117 | +11275/-7652 | 13/3 |
| Workspace / Permission | 29 | +4149/-564 | 5/0 |
| Tools | 45 | +2950/-215 | 7/0 |
| Telemetry / Mixpanel | 10 | +1690/-41 | 2/0 |
| Root / meta | 9 | +1318/-187 | 0/0 |
| Worktree / GC | 4 | +347/-134 | 0/0 |
| Other crates | 7 | +287/-24 | 0/0 |
| Update / Version | 4 | +167/-26 | 0/0 |
| Sandbox | 4 | +29/-29 | 0/0 |
| Textarea / Inline | 2 | +30/-18 | 0/0 |
| Agent lifecycle | 3 | +37/-3 | 0/0 |
| Models / Sampling | 1 | +16/-18 | 0/0 |
| Token estimation | 1 | +3/-5 | 0/0 |
| Config | 1 | +7/-0 | 0/0 |
| Chat state | 1 | +2/-4 | 0/0 |
| Markdown / Mermaid | 1 | +1/-1 | 0/0 |
| **Total** | **406** | **+36871/-12418** | **46/3** |

### Added

- Show what ~/.grok uses on disk with grok du
- Name the startup phase a slow launch is stuck in
- Export whether a tool only reads
- Conversation-only /rewind with confirm before rewind
- Show leader version mismatch in scrollback
- Expose Auto decision telemetry
- Typed Voice/Finance ToolUsageCard variants with client render support
- Report box in the /feedback card
- Bundle memory traces in the session trace export
- Tabbed usage/session-info modal for /usage, /session-info, and /context
- Detect standalone grok worktrees for branch display
- Suggest registered skill paths on failed reads
- Diagnose scroll anchor jolts
- Protect the tools server from the OOM killer and attribute OOM kills

### Changed

- Bound and measure how many subagents a session runs at once
- Prefill dashboard Ctrl+R rename with existing session title
- Warn and skip NotebookEdit/NotebookRead (like EnterWorktree)
- Full-jitter the reconnect backoff and gate the attempt reset on stability
- Tell WebLogin users to run grok update before re-authenticating
- Drop the Beta label from the product
- Speed up empty TUI exit
- Bump plugin CTA debounce to 500ms
- Name latest Windows download Grok Setup.exe
- Allow Send Now throughout goal mode
- Make non-blocking startup structural
- Speed up local /resume on large session transcripts
- include untracked files in HEAD→working git diff stats

### Fixed

- Launching many shell instances no longer locks the shared session search index
- Keep colliding skills invocable beside builtins
- Bound post-kill reaps so D-state children cannot wedge the turn
- Goal checker uses session model without eval timeout
- Keep the composer’s mode when the /feedback pane opens
- Wrap /btw overlay errors instead of truncating to one line
- Fix PROMPT_COMPLETE_DEPRECATION.md path in deprecation TODOs
- Esc and the [stop] button suppress task wakes like Ctrl+C
- Forward queue hold_edit/release_edit in leader mode
- Guard in-process git status/diff from client spam
- Make session recaps follow the session language instead of always English
- Honor startupHints on session request metadata; fix headless MCP connecting reminder
- Make memory trace wait signal-safe
- Keep standalone worktree flag across cwd switch and resume
- A requested quit always exits the process
- Answer HITL ExtMethods on -p
- Bound .envrc evaluation so a blocked read can't freeze session load
- Drain subagents before session delete
- Honest system.notify acks for synchronous pre-forward drops
- fix(textarea): Home/End jump to logical line when prompt is wrapped
- fix(sandbox): gate Linux-only hook write-deny code off macOS
- fix(pager): do not clobber worktree badge when opening the dashboard

### Removed / Deprecated

- Remove stale extracted platform skills that shadow bundled skills
- Remove legacy managed MCP configs client from shell and pager

### Merge risk for grok-pi

- Static preflight reports 18 content conflicts in Pager ACP/dispatch/event-loop/app/modals/context/scrollback/slash/views and Shell workflow host/manager.
- 89 upstream-touched files also changed locally since `a5589e9`; auto-merged files still require seam review.
- Preserve Pager-only TUI, Pi-owned agent/session/queue, headless adapter, and no Pi-source RPC extensions.
- Verification is static-only by user instruction; no Cargo build/test/check commands may run.


## [a5589e9] — 2026-08-07

> **Status:** Integrated and verified; delivered to local `main` via `sync/upstream-a5589e9` after explicit user authorization; not pushed.

- **Sync range:** `a422116..a5589e9` (`a4221165824e5b1f5c4c10b7459f65e78dd6448d` → `a5589e958437d79e13db026eedcb1720bffd4063`)
- **Upstream commits:** 4 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `4d6d11372ab8f73026a78c45a7b7e7b1310eb39f` (was `8d69c91f02bcacf01e98d5aebbf2f92547c45738`)
- **Diff size:** 577 files changed, +44078 / −17588

### Summary

This four-commit sync is dominated by Shell and Pager lifecycle, session, queue, dashboard, plan, permission, and terminal behavior. It adds ACP session resume/close operations, bounded-memory session forking, richer queue controls, permission-pattern editing, sandbox metadata, sampling retries, and multiple dashboard/Pager affordances while hardening auth, `/resume`, background-task, tmux, MCP, and teardown paths. The range heavily overlaps grok-pi's Pager/session seams, so integration must remain isolated and preserve Pi as the sole agent, session, and queue owner.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 288 | +17581/−11627 | 30/2 | auth flow, session eviction, bounded-memory forks, recap and background-parent continuity |
| Pager (TUI) | 175 | +16428/−4520 | 15/1 | `/resume`, queue, dashboard, plan approval, modal, copy and terminal behavior |
| Workspace / Permission | 26 | +4265/−476 | 5/0 | normalized path patterns, read-only Git approval and large-workspace deny globs |
| Tools | 24 | +1446/−225 | 4/0 | task-log sizing, output truncation and attachment handling |
| Other crates | 19 | +1134/−92 | 6/0 | shared runtime, test and supporting infrastructure |
| Models / Sampling | 15 | +932/−172 | 1/0 | retry propagation, 5xx recovery and optimistic model selection |
| Telemetry / Mixpanel | 11 | +920/−136 | 1/0 | shortcut and model-side skill-read telemetry |
| Sandbox | 3 | +577/−237 | 0/0 | plan, durable metadata and repository manifest types |
| Markdown / Mermaid | 3 | +425/−41 | 0/0 | plan preview, ANSI16 palette, wrapped diffs and narrow tables |
| ACP / Protocol | 4 | +255/−15 | 0/0 | session resume and close operations |
| Root / meta | 4 | +63/−15 | 0/0 | Rust toolchain, workspace lockfile and upstream revision metadata |
| Auth / Secrets | 3 | +45/−30 | 1/0 | sign-in, token suffix and first-party API-key probing |
| Config | 1 | +6/−1 | 0/0 | supporting configuration changes |
| Update / Version | 1 | +1/−1 | 0/0 | version metadata |
| **Total** | **577** | **+44078/−17588** | **63/3** | |

### Added

- Pager: make the response ▲ affordance clickable to jump to the top of the response being read.
- Pager: show Mermaid affordances in plan-mode preview.
- Permission prompt: add a free-form pattern editor to the "Always allow" command flow.
- Pager: let Tab walk answers in the `ask_user_question` card.
- Doctor: report tmux truecolor clamping.
- Telemetry: record `shortcut_used` for Ctrl+L actions.
- ACP: add `session/resume` and `session/close` operations.
- Pager: allow model switching during plan approval.
- Sandbox: provision plan, durable metadata, and repository-manifest types.
- Pager: let any queued item move up or down.
- Pager: toast when a session-only modal is opened from the dashboard.
- Permission UI: add a full-script permission showcase.
- Pager: surface disk-full failures during live sessions.
- Dashboard: show a per-turn summary on agent rows.
- Pager: detect automatic themes over SSH and tmux.
- Pager: make text in pinned sticky headers selectable and copyable.

### Changed

- Build: bump the Rust toolchain to 1.93.0.
- Pager: remove the manage-account link from `/session-info`.
- Workspace: auto-approve read-only Git queries and defer the write floor to the auto classifier.
- External-provider auth refresh: use one seven-second attempt instead of three five-second attempts.
- External-binary auth: treat sign-in as a fresh login rather than a refresh.
- Telemetry: count model-side skill reads and restore the skill-dispatched trigger.
- Models: apply pre-session model selection optimistically.
- Dashboard: clarify the overlay previous/next shortcut hint.
- Workflow: cap live subagents at 16 per run.
- Settings: render model names consistently.
- Session fork: stream the copy so large sessions fork with bounded memory.
- Input replay: represent file-mention chunks as user attachment links.
- Pager: show non-200 API errors as clean TUI banners.
- Shell: refresh leader documentation to match current behavior.
- Markdown: pin the palette to ANSI16 hues.
- Permission UI: collapse long Bash permission bodies behind Ctrl+F.
- Security UI: contextualize Auto-mode findings.
- MCP: show disabled server stubs only when they can be re-enabled.
- Dashboard: improve the per-turn summary prompt to emphasize reply substance.
- Markdown: reflow narrow tables within cells.
- Pager: standardize one Tab contract across blocking cards.
- Extensions modal: group entries, sort them A–Z, and make skills collapsible.

### Fixed

- Shell auth: route expired external-provider credentials to sign-in instead of a 401 loop.
- Shell: prevent large task logs from making completion messages excessively long.
- Plan viewer: widen the scrollbar grab zone and remove the striped thumb in Terminal.app.
- Pager: poll the tmux probe teardown grace instead of sleeping through it.
- Security: enforce the vendor-compat MCP kill switch when it is reported as enabled.
- Shell: restore session eviction when a leader client disconnects.
- Workspace security: lexical-normalize permission path patterns before glob matching.
- Pager: reject invalid Enter input in the `/resume` picker.
- Shell: correct `/btw` caching.
- Pager: do not resurrect completed background tasks as Running when completion arrives first.
- Plan viewer: prevent scrollbar click-drag from being hijacked by the comment gutter.
- Pager/Shell: stop duplicate Recap after the same final turn.
- Sampler: preserve `x-should-retry` through stream collection.
- Pager: clear the plan-mode indicator immediately after plan approval.
- Pager: avoid making tmux re-read its configuration on reattach.
- File watching: skip nested checkouts so in-repository worktrees cannot stall startup.
- Tools: report the real log size when short output is only a partial view.
- Workspace: emit `git-head-changed` for same-branch commits.
- Auth: correct authentication-token suffix handling.
- Runtime: preserve `errno` across signal callbacks.
- MCP: extract images before output truncation.
- Sandbox: avoid startup refusal when deny globs traverse a large workspace.
- Sampling: retry Cloudflare 52x and other 5xx errors.
- Pager: include sessions in `/resume` search that are loadable through `--resume`.
- Pager: paint automatic Recap only while the CLI is idle.
- Terminal teardown: always emit mouse and paste resets.
- Queue: keep plain queued prompts visible.
- Mouse copy: preserve wide CJK graphemes at selection boundaries.
- Diff rendering: retain syntax styles on wrapped lines.
- Session UI: report the mode the session is actually using.
- Dashboard: make exit/quit terminate the CLI.
- Queue: ensure send-now never silently destroys an earlier queued message.
- Slash UI: execute the highlighted command on Enter.
- Dashboard: return to the dashboard after `/delete` from an attached session.
- Auth: probe the first-party API key before skipping login.
- Background work: continue in-flight parent work after spawning a child task.
- Session restore: register restored child sessions so `--resume` does not return 404.
- Dashboard: re-point the attached session after `/new`.

### Removed / Deprecated

- Remove the project-directory picker.

### Merge risk for grok-pi

- Shell and Pager account for 463 of 577 changed files; 99 upstream paths overlap local post-`a422116` work, including 61 Pager/session/model/settings seams.
- ACP session resume/close and restored-child registration must not transfer session ownership away from Pi or make `pi-grok-adapter` stateful; upstream does not touch the fork-only adapter crate.
- Queue reordering, send-now preservation, and background-parent continuation overlap grok-pi's Pi-owned queue mirror and require focused state-machine validation.
- Pager `/resume`, dashboard, plan approval, model selection, terminal teardown, copy, and modal changes overlap native external-profile seams and should be reapplied surgically in an isolated worktree.
- Permission-path normalization, read-only Git auto-approval, sandbox metadata, MCP kill-switch enforcement, and auth changes received focused security/error-path checks. This verified isolated integration updates `SOURCE_REV` and the `AGENTS.md` base without broadening verifier allowlists; delivery to local `main` completed after explicit user authorization, with no remote push.

## [a422116] — 2026-08-01

> **Status:** Merged into grok-pi via isolated commit `91394c1`; fast-forwarded to local `main` without pushing.

- **Sync range:** `dd04f39..a422116` (`dd04f397b1d02f2272b092555669dfba1f01bc85` → `a4221165824e5b1f5c4c10b7459f65e78dd6448d`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `8d69c91f02bcacf01e98d5aebbf2f92547c45738` (was `2a28b4a86cfc4a4c133c35b7fc2a6a9964387c39`)
- **Diff size:** 165 files changed, +15161 / −1969

### Summary

This sync is a broad runtime reliability and lifecycle update across Pager, Shell, tools, workspace, and Computer Hub. It adds session/task cleanup, configurable wait caps, context-overflow recognition, overload retries, compaction continuity, background-task reminders, and safer PTY/auth/permission behavior. Pager app/event-loop/session/dashboard changes and Shell agent/session/compaction changes overlap grok-pi integration seams, so the merge must be performed in an isolated worktree and validated without transferring ownership from Pi.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Pager (TUI) | 55 | +6829/−652 | 1/0 | dashboard/session deletion, workspace mode, history shortcuts, lifecycle and event-loop behavior |
| Shell (agent runtime) | 58 | +4478/−776 | 9/0 | session resource reclamation, compaction continuity, auth retry, PTY registry and config watching |
| Computer Hub | 4 | +1159/−174 | 0/0 | liveness, attached-client signals, and task lifecycle handling |
| Workspace / Permission | 12 | +752/−92 | 1/0 | protected sandbox configuration edits and workspace RPC exports |
| Tools | 8 | +526/−29 | 0/0 | wait caps, task completion reminders, and task/context schemas |
| Models / Sampling | 8 | +476/−87 | 0/0 | budget-overflow classification and overload/retry handling |
| Other crates | 4 | +289/−113 | 0/0 | HTTP/TLS, PTY control, test support, and managed-config types |
| ACP / Protocol | 4 | +231/−31 | 0/0 | task frames and tool/runtime protocol support |
| Telemetry / Mixpanel | 4 | +197/−5 | 0/0 | session activity and liveness telemetry |
| Root / meta | 4 | +191/−9 | 0/0 | dependency, lint, and upstream revision metadata |
| Compaction | 3 | +32/−0 | 0/0 | compaction reminders and sampling support |
| Update / Version | 1 | +1/−1 | 0/0 | version dependency metadata |
| **Total** | **165** | **+15161/−1969** | **11/0** | |

### Added

- Add background-subagent completion reminders with a selectable delivery surface.

### Changed

- Release a shell session's resources in one drop.
- Make the tools blocking-wait cap client-configurable and self-describing.
- Carry running background tasks and subagents across compaction.
- Require round-trip time for SDK liveness checks.
- Consume the attached-client signal and report why idle is withheld.
- Release a session's activity record when the session ends.
- Scope skills watches on project vendor roots.
- Make the leader soak measure the leader, not its harness.
- Delete sessions from the dashboard and welcome list.

### Fixed

- Recognize API "exceeds budget" errors as context overflow.
- Retry /btw on model overload.
- Make a PTY shell reap itself until it reaches the registry.
- Recover the OS error code from a TLS-phase connection reset.
- Treat `.grok/sandbox.toml` edits as protected so auto mode prompts before writing.
- Surface history/search in the Ctrl+. cheatsheet and keep it working in history view.
- Stop charging auth-retry budget for fail-closed 401s; reset it across suspends.
- Make [stop] cancel in-flight compaction.

### Merge risk for grok-pi

- The 55 Pager and 58 Shell files include the highest-risk `app/`, event-loop, session, dashboard, auth, compaction, PTY, and task-lifecycle paths; `pi-grok-adapter` remains untouched and must stay headless.
- Ten upstream-touched files overlap local post-`dd04f39` work: `app/actions.rs`, `app/agent_view/{mod,render,session}.rs`, `app/app_view.rs`, `app/dispatch/router.rs`, `app/dispatch/session/lifecycle.rs`, `app/effects/{helpers,mod}.rs`, and `Cargo.lock`.
- Session cleanup, background-task/subagent continuity, and compaction changes must preserve Pi as the sole agent/session owner and keep Grok Pager as the only visible TUI.
- Permission, sandbox, auth, and TLS changes require focused security/error-path checks; `SOURCE_REV`, the prior changelog status, and verifier metadata should only be closed after a verified integration.

## [dd04f39] — 2026-07-31

> **Status:** Merged before this sync in `360f801`; superseded by `[a422116]`.

- **Sync range:** `47348d1..dd04f39` (`47348d13ec4508dcfe440e34c6d511bb02998fb2` → `dd04f397b1d02f2272b092555669dfba1f01bc85`)
- **Upstream commits:** 5 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `2a28b4a86cfc4a4c133c35b7fc2a6a9964387c39` (was `d02693a856a54f1030695b36b91d276e96b30b23`)
- **Diff size:** 618 files changed, +55053 / −18012

### Summary

This five-commit sync is dominated by Shell and Pager lifecycle, headless ACP, task/process cleanup, sampling/compaction, and tool-runtime work. It bounds large-session and fork memory, reclaims session-owned child resources, adds ACP session listing and background-task/tool streaming, improves startup and terminal-resize resilience, and expands security, certificate, dashboard, and telemetry behavior. The range overlaps 102 fork-modified files, including Grok-Pi's highest-risk Pager `app/`, external ACP/session, plan/settings, queue/subagent, model/sampling, headless, and event-loop seams.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 163 | +17195/−5969 | 34/0 | session memory/process reclamation, startup recovery, persistence and ACP lifecycle |
| Pager (TUI) | 234 | +16466/−5325 | 34/0 | plan/settings/session behavior, headless split, terminal resize and ACP projection |
| Tools | 92 | +8861/−1283 | 9/0 | task/process cleanup, LSP diagnostics, monitoring and schema behavior |
| Models / Sampling | 19 | +5131/−4765 | 7/0 | per-backend conversion modules and sampling infrastructure |
| Other crates | 39 | +2006/−129 | 11/0 | HTTP, crash, proxy/admin, test and shared runtime infrastructure |
| Workspace / Permission | 16 | +1930/−223 | 0/0 | task snapshots, git/workspace operations and preview metrics |
| Compaction | 4 | +804/−17 | 1/0 | tokenizer-aligned counts and context-length recovery |
| Telemetry / Mixpanel | 9 | +677/−49 | 0/0 | consent, insert IDs, terminal/source and subscription telemetry |
| Worktree / GC | 7 | +485/−57 | 0/0 | resume-safe worktree pruning and worktree lifecycle |
| MCP | 4 | +427/−90 | 0/0 | CLI controls, credentials and child-process cleanup |
| Computer Hub | 4 | +292/−1 | 0/0 | hub metrics and task lifecycle integration |
| Agent lifecycle | 3 | +199/−29 | 0/0 | parent-death cleanup and agent/session lifecycle support |
| ACP / Protocol | 2 | +181/−31 | 0/0 | session listing and task/tool streaming protocol support |
| Hooks / Plugins | 6 | +139/−7 | 0/0 | session-owned hook child cleanup |
| Auth / Secrets | 2 | +113/−4 | 0/0 | provider command execution and token-refresh hardening |
| Root / meta | 3 | +50/−5 | 0/0 | lockfile, workspace metadata and SOURCE_REV |
| Update / Version | 2 | +36/−10 | 0/0 | source-tagged version reporting |
| Sandbox | 1 | +22/−0 | 0/0 | leader-process sandbox enforcement |
| Other | 1 | +12/−13 | 0/0 | generated/protocol support outside mapped crate areas |
| Config | 2 | +11/−5 | 0/0 | subagent depth and forking settings |
| Chat state | 2 | +10/−0 | 0/0 | history trailer and usage data |
| Voice | 2 | +4/−0 | 0/0 | supporting capture/runtime changes |
| Markdown / Mermaid | 1 | +2/−0 | 0/0 | supporting rendering changes |
| **Total** | **618** | **+55053/−18012** | **96/0** | |

### Added

- Detect the Herdr multiplexer.
- Add a subagent lifecycle soak that bounds threads, file descriptors, and heap, and fail closed when soak metrics are absent.
- Add source-tagged terminal version telemetry, DA2 terminal probing, and terminal-version feedback metadata.
- Add reusable session test helpers and synthetic replay/round-trip coverage.
- Add `computer_reason` to the `ConversationHistoryDone` trailer and forward it from history-load trailers.
- Make maximum subagent nesting depth configurable.
- Allow `/loop` to store prompts that can terminate the loop.
- Add SuperGrok Plus identity, CLI, and analytics tier surfaces.
- Add team-scoped Grok Code managed-config admin routes to the CLI chat proxy.
- Add CLI enable/disable controls for MCP servers.
- Harden workspace git operations and add `git_sync_base`.
- Add a feature-gated gRPC retry policy to the circuit breaker.
- Add a project-level forking-settings toggle with backend and deploy-time controls.
- Track coding-data consent decisions.
- Ship the Agent Dashboard user guide.
- Expose chat-product Skills through ACP `available_commands_update`.
- Add opt-in extra root CAs through `GROK_EXTRA_CA_BUNDLE`.
- Stream headless tool calls over ACP and bridge gateway task lifecycle for chat-session background tasks.
- Add an ACP `session/list` method.
- Add `/undo` as an alias for `/rewind`.
- Read C# diagnostics while keeping Roslyn alive across edits.

### Changed

- Bound peak memory while loading large sessions and stream inherited replay to bound fork memory.
- Show the UI immediately while fetching models and settings in the background.
- Copy the complete approved plan with `y`, keep the whole plan in scrollback, and separate reasoning from output in minimal mode.
- Make workspace task snapshots list only incomplete background tasks.
- Run plan-mode exit last in mixed tool batches.
- Reclaim retained/resident session state and session-owned processes, LSP servers, MCP children, subagents, Bash/background commands, and hook children when a session closes.
- Inherit the parent session process scope into subagents, cancel all session subagents when the user stops, and let session persistence exit when its session ends.
- Build the `@` file-search matcher lazily and cap Shell/Workspace Tokio workers on many-core hosts.
- Reuse spawn-time skill discovery for session telemetry.
- Mark `/gboom` as non-production code and quiet routine auth, LSP, and config warnings.
- Cache growing transcripts on the messages backend.
- Tell the model when a wait was clamped instead of re-inviting it.
- Deliver the stationarity nudge after the tool result and stop claiming results are identical.
- Run auth-provider commands through the platform shell.
- Keep monitor-tool stdout short and prescriptive.
- Use UUIDs for analytics event insert IDs.
- Allow deleting the current session from within that session.
- Enable doom-loop recovery by default.
- Kill agent children and the idle inhibitor when the parent process dies.
- Temporarily disable TUI session-share link creation.
- Split the headless Pager module into clearer submodules.
- Use the compaction sampler tokenizer for item token counts.
- Make fullscreen terminal resize substantially cheaper for long sessions.
- Hide `/usage` for external-auth deployments.
- Declare slash-command screen-mode support in one place.
- Keep settings enum pickers on the committed value until Enter.
- Give each sampling backend its own conversion module.
- Suppress the cancelled marker on send-now wake turns.

### Fixed

- Prevent armed signature verification from deleting the managed-deny smoke policy.
- Correct observability attributes for warm-store errors, restore setup, remote tools, and preview denials.
- Security: apply the sandbox profile to the leader process that executes tools.
- Withhold key-event types from affected Alacritty builds that otherwise double keys.
- Degrade `@` file search instead of aborting when thread creation is exhausted.
- Correct contradictions and defects in tool descriptions, schemas, outputs, and harness pools.
- Stop leaking shell-wrapper positional parameters into sourced scripts, including persistent/static-shell Conda activation.
- Self-heal a corrupt session-search SQLite cache.
- Capture `SIGABRT` so panic-aborts leave crash reports.
- Stop crashing at startup when the host cannot create more threads.
- Fail open the access gate to avoid false CLI paywalls.
- Fix multi-process credential wipes and orphaned session log writers.
- Do not approve a plan when the revise prompt receives an empty Enter.
- Return immediately from a blocking wait when the ACP task is already complete.
- Stop worktree pruning from removing user registrations during resume.
- Report honestly from `kill_task` when an ACP task does not exist.
- Reap a PTY's full process tree.
- Avoid truncated-history warnings for suppressed replay.
- Fit full-replace summarizer input and recover from context-length errors.
- Stop dropping agents for unrecognized frontmatter colors.
- Harden sleep/wake token-refresh paths against forced re-login.
- Treat an unenrolled child process as a lint error.

### Removed / Deprecated

- Remove the ineffective no-op tool reminder.

### Merge risk for grok-pi

- Pager and Shell account for 397 changed files; 102 paths overlap fork modifications across `app/`, settings/model startup, plan/review scrollback, queue/subagent/session lifecycle, external ACP, headless, and event-loop seams.
- The headless module split plus ACP `session/list`, tool-call streaming, and gateway-task lifecycle must preserve Pi as the only agent/session owner and keep `pi-grok-adapter` headless.
- Upstream Herdr detection must be reconciled with the fork's existing Herdr integration rather than replacing product-specific behavior.
- Session-close and parent-death reclamation spans Tools, MCP, PTYs, hooks, subprocess scope, LSP, persistence, and subagents; Pi child-session ownership and adapter lifecycle must remain intact.
- Sampling and compaction refactors overlap Grok-Pi model/thinking/context projection and require focused adapter plus Pager validation.
- Conflict resolution must happen in an isolated integration worktree before any verified result is delivered to local `main`; no remote push is part of this recording step.

## [47348d1] — 2026-07-26

> **Status:** Merged into local `main` by ff-only delivery (two-parent upstream merge `be91fe7`); 64-path concurrent WIP restored unstaged from verified safety tip `3d4278d`.

- **Sync range:** `6e38642..47348d1` (`6e386420825bd44ae648c63e7c8cba12fcec9401` → `47348d13ec4508dcfe440e34c6d511bb02998fb2`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `d02693a856a54f1030695b36b91d276e96b30b23` (was `9b8d35b46d959c042ea9aa31cbbebbd1f0c5c527`)
- **Diff size:** 138 files changed, +7283 / −5796

### Summary

This sync is dominated by Pager and Shell reliability changes plus workspace-security, managed-config signing, and hook configuration updates. It makes startup/runtime failures recoverable, preserves completed terminal output across gateway loss, tightens workspace file confinement and hook-root approval boundaries, and changes lifecycle behavior around session termination. Pager `app/`, session visibility, inline/freeform input, task output, hooks, config paths, and external-agent integration are high-risk Pi-Grok seam areas and must be merged in isolation.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 30 | +1891/−2608 | 5/1 | recoverable HTTP/runtime/session failures and terminal transport behavior |
| Pager (TUI) | 69 | +2181/−2148 | 1/0 | task details, paste parity, session visibility, status-marker rendering |
| Workspace / Permission | 16 | +1334/−325 | 2/0 | workspace file confinement and `acceptEdits` hook-root security |
| Config | 9 | +1039/−138 | 0/0 | signing key and managed-config verification controls |
| Hooks / Plugins | 7 | +722/−414 | 0/0 | config-file hook parsing and SessionEnd behavior |
| Agent lifecycle | 2 | +40/−148 | 0/0 | recoverable session thread/runtime spawning |
| Tools | 1 | +53/−6 | 0/0 | supporting tool behavior |
| Other crates | 1 | +16/−3 | 0/0 | supporting shared crate changes |
| Root / meta | 2 | +6/−5 | 0/0 | lockfile and SOURCE_REV |
| Update / Version | 1 | +1/−1 | 0/0 | version metadata |
| **Total** | **138** | **+7283/−5796** | **8/1** | |

### Added

- Raise the Linux file-descriptor soft limit and log effective limits at startup.
- Embed the deployment-config signing public key.
- Parse hooks from configuration files.
- Add a remote kill-switch for managed-config signature verification.

### Changed

- Keep completed terminal output when the gateway connection is lost.
- Show duration-only detail for single-task task output.
- Prevent a stale registry turn counter from hiding local sessions.
- Make HTTP client construction failures non-fatal.
- Make session-thread and runtime-spawn failures recoverable.
- Fire `SessionEnd` hooks on `/exit` and headless quit.

### Fixed

- Report invalid MCP server configuration instead of failing startup.
- Restore main-prompt paste parity in the question freeform input.
- Repaint paste-chip backgrounds on inline panel inputs.
- Security: prevent `acceptEdits` from auto-approving writes into the always-trusted global hook root.
- Render stacked “Worked for” markers correctly so parks appear as status and turns close with exactly one marker.
- Security: keep workspace file-reference resolution inside workspace filesystem confinement.

### Integration result

- Preserved upstream ancestry in two-parent merge commit `be91fe7` after resolving five conflicts by combining upstream lock/security/lifecycle semantics with Pi-Grok settings, model-picker and external-agent seams.
- Preserved Pi-owned queue/session/tree/trust/tools/extensions behavior, `OpenPiConfig`, `DirectPi`, F2 settings (including Pi built-in tools and grouped recap/BtW model slots), `.grok-pi` product isolation, and the locked `pi-main` gitlink `a5afc3f`.
- Restored omitted upstream lifecycle/test infrastructure during semantic audit: external ACP `agent_thread`, PTY timeout/EOF pump semantics, and `workspace.tasks_snapshot` dispatch.
- Passed adapter tests (128), serial `grok-pi` binary tests (56), config tests (184), isolated-home Workspace tests (1560 library + 21 server), settings-modal tests (173; 1 ignored), two focused Pager contract tests, `cargo check`, and `./build.sh`.
- Added a repository-managed Cargo cache at `<git-common-dir>/pi-grok-cargo-target`; all linked worktrees share the same generated artifacts via ignored `target` symlinks. The migration and wrapper were tested in a temporary multi-worktree repository and on all current worktrees.
- Remaining failures are separated from merge regressions: one Hooks DNS test reproduces before the merge because this machine resolves `.invalid` to an internal address; Python tree-sitter packages are absent; source/renderer hash manifests, slash `fork`/`voice` rules, and one mock completion-barrier expectation were already stale and were not weakened or regenerated blindly.
- Delivery was ff-only from local `main` `906470c` to integration base `1a52f81`; the final closeout documentation commit is layered on that history while the restored WIP remains uncommitted. Post-restore Herdr Node/Rust, Pi-model, cargo-check, and help-smoke checks passed with zero manifest mismatches.

### Merge risk for grok-pi

- Pager changes span 69 files and overlap Pi-Grok `app/`, modal/input, task-output, session and external-profile seams.
- Hook/config changes must preserve `.grok-pi` product isolation and `project_config_dir()` routing while absorbing upstream security fixes.
- Runtime recovery and gateway-loss behavior must not transfer agent/session ownership away from Pi or make the adapter stateful/UI-owning.
- Managed-config signing changes may alter verifier baselines and root metadata; `SOURCE_REV`, `AGENTS.md` base and baselines change only after a verified merge.

## [6e38642] — 2026-07-25

> **Status:** Merged into grok-pi `main` by ff-only through verified integration tip `92b7c3d` (two-parent upstream merge `963ccf5`).

- **Sync range:** `a5727c5..6e38642` (`a5727c5960452e7527a154b25cb5bf00cda0545e` → `6e386420825bd44ae648c63e7c8cba12fcec9401`)
- **Upstream commits:** 2 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `9b8d35b46d959c042ea9aa31cbbebbd1f0c5c527` (was `30192d2eef5d91a8fff0e53957de5bd05b43398c`)
- **Diff size:** 349 files changed, +27899 / −10881

### Summary

Large sync dominated by Pager and Shell changes: title-based resume, queue editing, tutorial and privacy surfaces, auth/provider hardening, true-noop turn handling, workflow recovery, and expanded tool/workspace behavior. Pager `app/`, session, queue, voice, settings, workflow overlay, and Shell ACP/auth paths overlap heavily with Pi-Grok integration seams, so the merge must remain isolated and preserve the fork's external-agent and Pi-owned runtime boundaries.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 111 | +6726/−7519 | 4/2 | auth refresh, provider gateways, turn stop/origin, resumable workflows |
| Pager (TUI) | 130 | +9666/−1587 | 16/0 | resume, queue edit, tutorial/privacy, voice and workflow overlay |
| Tools | 40 | +5623/−887 | 7/0 | managed catalog refresh and tools-server callback surface |
| Sandbox | 8 | +2103/−262 | 2/0 | persistent hook-source protection and deny-path hardening |
| Config | 8 | +1020/−2 | 2/0 | global hook sources and managed configuration |
| Workspace / Permission | 10 | +915/−63 | 0/0 | readiness failure reporting and fail-closed policy behavior |
| Update / Version | 5 | +349/−436 | 1/1 | soft and required CLI version checks |
| Agent lifecycle | 6 | +464/−28 | 1/0 | agent/session metadata and lifecycle changes |
| Models / Sampling | 7 | +338/−29 | 1/0 | image/session metadata and default web-search model |
| Other | 6 | +307/−5 | 0/0 | documentation and supporting project assets |
| Workflow | 2 | +183/−2 | 0/0 | scratch quotas and failed-run resume support |
| Other crates | 5 | +85/−16 | 0/0 | shared support changes outside mapped areas |
| Root / meta | 3 | +27/−31 | 0/0 | workspace metadata, lockfile, and SOURCE_REV |
| Hooks / Plugins | 2 | +44/−14 | 0/0 | marketplace URL validation and hook discovery |
| Chat state | 4 | +20/−0 | 0/0 | deploy-state and turn metadata plumbing |
| Telemetry / Mixpanel | 1 | +16/−0 | 0/0 | gateway lifecycle telemetry |
| Voice | 1 | +13/−0 | 0/0 | interim text submission and editing behavior |
| **Total** | **349** | **+27899/−10881** | **34/3** | |

### Added

- ACP terminal output recorder
- Cross-platform provider-auth commands in the Shell
- Custom provider gateways and subprocess-environment policy in the Shell
- `/tutorial`, an opt-in Grok Build onboarding tour
- Soft and required CLI version checks in the Shell
- Remote flag to override the image-edit model
- Edit control on queued prompt rows
- Setting to disable the Ctrl+Space/F8 voice shortcut
- Privacy upsell banner in agent view until acted on
- Tools-server client callback surface
- Documentation for marketplaces, plugins, and organization controls
- Chat API fields for deploy archive, taken-down, limit, and in-progress reasons
- Chat-supplied per-session turn index in turn hooks
- Metrics for true-noop and stationarity stops
- Gateway bridge lifecycle telemetry

### Changed

- Default `/resume` to Grok sessions and show a hint for hidden external sessions
- Resume sessions by title with `--resume`
- Limit app-builder archive size
- Drive slash-command tag labels from data
- Surface Grok Computer media-generation results as file-path chunks
- Stamp session ID on image-generation direct-to-API requests
- Make auto mode consider recent user intent
- Show Bash mode chrome in minimal mode
- Include voice interim text on prompt submission
- Silently end a turn on true-noop thrash
- Quiet the copy toast when clipboard delivery is confirmed
- Make the idle “still running” watcher cue open the tasks pane
- Default the web-search model to Grok 4.5
- Let plugin subagents inherit parent MCP servers
- Gate the no-op end-turn reminder on system reminders
- Allow editing finalized text while voice is open
- Relocate the token carrier to turn-commit events and plumb per-turn origin context
- Raise workflow scratch quotas and make failed runs resumable
- Auto-progress workflow-overlay phases, show live agent status, and remove the budget meter

### Fixed

- Report workspace-server `/ready` failure with dwell when hub connection fails
- Refresh the Grok agent OIDC token in the Shell
- Fix tmux issues through Doctor remediation
- Preserve privacy-banner environment overrides across live settings updates
- Return auth-info profile fields even when the access token is expired
- Keep fail-closed behavior when clearing orphans with no team
- Pass `--raw` to `pw-record` for Linux dictation on older PipeWire
- Validate Git URLs when adding marketplace entries
- Stop shipping stale tool-doc parameter and tool names
- Re-point dashboard attach after `/fork` only when the parent was attached
- Clear the web background-task tray on kill while retaining the task description
- Protect persistent global hook sources
- Refresh tool search when the managed MCP catalog is re-fetched
- Prevent duplicate leader-process spawn and startup hangs from stale leaders
- Correct auto-mode blocked documentation
- Enforce a fail-closed auth-refresh contract for Shell clients
- Fix session forks truncating at the wrong prompt in rewound sessions

### Integration result

- Resolved 16 conflicts in an isolated worktree and preserved upstream ancestry in two-parent merge commit `963ccf5`.
- Preserved Pi-owned workflow spawning, external-agent routing, product-isolated paths, Pi session/tree/queue/settings, DirectPi effects and `pi_update`; adapted upstream mailbox, voice, tutorial, privacy, slash-tag and send-now contracts.
- Passed adapter tests (128), serial `grok-pi` binary tests (56), `cargo check`, and `./build.sh`.
- Remaining source/renderer/slash/mock verifier failures reproduce unchanged on pre-merge `main`; allowlists were not broadened. Workflow focused tests are 73/74 with the known macOS `/var` canonical-path assertion failure.

### Merge risk for grok-pi

- Upstream changes heavily overlap `xai-grok-pager/src/app/`, including session lifecycle, queue editing, settings, voice, workflow overlay, event loop, actions, effects, mouse handling, and task results.
- Preserve Pi-Grok seams: `external_agent` routing, Pi-owned queue/session/trust behavior, OpenPiConfig and product-isolated paths, DirectPi effects/results, model-picker guards, and native Pager-only rendering.
- Shell auth/workflow changes are upstream-owned and should normally take upstream behavior; do not let them pull Grok runtime ownership into `pi-grok-adapter`.
- Update `SOURCE_REV`, `AGENTS.md` base, and source-identity/renderer baselines only after the isolated merge is resolved and verified.

## [a5727c5] — 2026-07-23

> **Status:** Merged into grok-pi `main` via `sync/upstream-a5727c5` @ `4d19f00` (ff-only).

- **Sync range:** `3af4d5d..a5727c5` (`3af4d5d39897855bdcc74f23e690024a5dc05573` → `a5727c5960452e7527a154b25cb5bf00cda0545e`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `30192d2eef5d91a8fff0e53957de5bd05b43398c` (was `0f4d7c91b8b2b408333f6de1e8a76cb8eaa71899`)
- **Diff size:** 482 files changed, +37627 / −13402

### Summary

Medium-large monorepo sync focused on **Doctor remediation consolidation**, **auto-mode classifier / permission gate behavior**, **marketplace reliability**, **working-directory relocation recovery**, and broad **Pager UX** (Esc cancel, queue edit newlines, permission auto-focus, dashboard hit targets). Shell/runtime and workspace permission crates dominate the +/−; Pager `app/` and `dispatch/` remain high-risk seam surfaces for grok-pi.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 136 | +13903/−6593 | 3/0 | relocation recovery, doctor, toolOverrides, workflows default-on |
| Pager (TUI) | 234 | +11313/−3749 | 1/0 | Esc cancel, queue edit, dashboard, session-info, doctor UI |
| Workspace / Permission | 28 | +4083/−1405 | 1/0 | auto-mode classifier, Bash(git:*) chain match, folder trust |
| Test support | 9 | +3227/−341 | 1/0 | shared process lifecycle + sandbox |
| Tools | 25 | +2075/−362 | 2/0 | bang timeout, scheduler expiry, toolOverrides wire |
| Voice | 13 | +1022/−350 | 3/1 | out-of-process macOS mic capture |
| Common / models / agent | 16 | +1623/−376 | 1/0 | sampling types, agent lifecycle, file-utils |
| Config / hooks / chat / meta | 21 | +240/−133 | 0/0 | feedback.user docs, marketplace, SOURCE_REV |
| **Total** | **482** | **+37627/−13402** | **12/1** | |

### Added

- Non-blocking coding-data sharing upsell banner
- `toolOverrides` wire types and session/agent wiring
- Out-of-process macOS mic capture
- Shared test process lifecycle and shared test sandbox
- Relocation transaction state machine
- Privacy notice rollout flag
- One-shot occurrence journal persistence
- Durable scheduler expiry persistence
- Document `[feedback.user]` author identity config

### Changed

- Consolidate remediation in Doctor; apply doctor fixes in the TUI; route startup warnings to doctor
- Auto mode defers fail-closed gate asks to the classifier; classifier honors recorded approvals; timeouts prompt instead of silent deny
- Marketplace: coalesce list fetches; remove source by name; contain hung git sources (timeouts, non-blocking refresh, unbrick modal)
- Report real exit codes for completed background shells
- Narrow the date-rollover reminder to date-bearing templates
- Split prompt-trigger telemetry and record classifier provenance
- Raise connectors-manager timeout to 60s
- Scope subagent completion drains to the owning session
- Set `client_identifier=grok-agent-sdk`
- Accept both spellings of the workspace-teleport kill switch
- Stop turns that poll the exact same tool call 16× in a row
- Copy compaction checkpoint files when forking sessions
- Auto-focus permission prompt from scrollback
- Esc cancels the running turn in non-vim and minimal modes
- List Ctrl+Z undo and redo in keyboard shortcuts
- Show active auth mode on session-info
- Install the npm binary under `$GROK_HOME`
- Shift/Alt+Enter inserts newline when editing a queued prompt
- Gate project Claude permissions on folder trust
- Echo `response.create.event_id` on `response.created`
- Toast when session creation fails from disk full
- Enable dynamic workflows by default
- Integrate relocation recovery
- Confirm before removing extensions-modal items
- Re-run compact and prompt after login when compact hit expired auth
- Recap sends hosted tools under backend search
- Extend bang command timeout
- Label failed workspace RPCs with `error_kind`
- Drop redundant explicit tonic/prost deps from `xai-grok-shell`

### Fixed

- Security: Bash(`git:*`) allowlist matches whole command chain by prefix
- Close combine-queued edit-hold race
- Break harness discovery ref cycle so connections can idle-evict
- Remove hover/click dead zones between dashboard items
- Surface auth failures on model-switch compact

### Merge risk for grok-pi

- **Do not merge on `main`.** A trial `git merge upstream/main` produced **48 unmerged paths** and was aborted; use an isolated worktree/branch.
- High seam overlap: `xai-grok-pager/src/app/` (69 files, +5675/−1761), `dispatch/` (+2208/−481), `acp/tracker`, `event_loop`, `slash/`, `pager-bin/src/main.rs`.
- Permission/auto-mode and queue-edit changes may interact with Pi queue mirror + External profile intercepts — reapply narrow Pi-Grok seams after taking upstream core logic.
- `SOURCE_REV` / `AGENTS.md` base updated on merge-back (`30192d2e…` / `a5727c5`). Source-identity baselines may still need a deliberate regen if verifiers fail.
- Pi-Grok-only crates (`pi-grok-adapter`, `extensions/`) are not in this upstream range.


## [3af4d5d] — 2026-07-22

> **Status:** Merged into grok-pi (branch `sync/upstream-3af4d5d` @ `a5ffbcb`, pending merge back to `main`).

- **Sync range:** `a881e67..3af4d5d` (`a881e6703f46b01d8c7d4a5437683546df30449d` → `3af4d5d39897855bdcc74f23e690024a5dc05573`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `0f4d7c91b8b2b408333f6de1e8a76cb8eaa71899` (was `c5c4ce03436b4bb2cec43d3feaa27dee0109bf37`)
- **Diff size:** 556 files changed, +56609 / −21892

### Summary

Large monorepo sync dominated by a brand-new **workflow engine** crate
(`xai-workflow`), a major **permission/security overhaul** in
`xai-grok-workspace` (exec-risk scoring, auto-mode, hardened shell access), and
extensive **Shell** and **Pager** changes (working-directory relocation, model
providers, doctor diagnostics, prompt-queue batching). Multiple security fixes
close RCE and credential-plugin attack vectors.

### Added

- Workflow: new `xai-workflow` crate — durable workflow execution engine with journaling, metadata, validation, and host interface
- Workflow authoring skills: `create-workflow` and `import-claude-workflow` docs
- Worktree: kind-aware auto-GC TTLs and config knobs
- Worktree: macOS process CWD scan and Unix PID liveness for GC guards
- Worktree: automatic throttled GC on startup (Linux age-based; non-Linux dead-only)
- Pager: `[ui].combine_queued_prompts` config to batch queued follow-ups
- Pager: expose `doctor` in the TUI
- Pager: edit minimal prompts in an external editor
- Shell: working-directory relocation state primitives and storage primitives
- Shell: resume sessions when the working directory moves
- Shell: `max` as a distinct reasoning effort tier
- Shell: model providers
- Shell: attach author identity to feedback when the deployment opts in
- Tools: scheduler lifecycle version clock
- Proto: `ClientToolResult` and `ChatConfig` client-side tools
- `/usage` shows per-session token and dollar usage in the TUI
- Voice: diagnose silent-mic failures (macOS permission) and add doctor/terminal-setup Voice section
- App builder deployer: `allow_forking` and `show_built_with_grok`
- Doctor: read-only `grok doctor` command

### Changed

- Shell: accept target response id on rewind execute
- Shell: stamp response id on chat user message chunks
- Shell: give side model calls their own conversation ids
- Shell: recap rides the parent turn's prompt cache
- Worktree: optional rebuild and stale git registration cleanup in auto-GC
- Tools: read markdown in `skills/` directories untruncated
- Tools: serialize background `/loop` fires on the whole work unit
- Pager: idle watcher cue — "1 subagent still running" instead of "watching · 1 subagent"
- Pager: make actions screen-mode aware
- Pager: centralize terminal diagnostics and probes
- Pager: standardize backgrounding on Ctrl+B
- Chat: select App Builder product on the Build path
- Sandbox: apply Landlock without a controlling TTY
- Workspace: gate inline shell file access

### Fixed

- Shell: stop overwriting user skills
- Security: prompt on environment-dumping `ps` variants
- Security: `kubectl` no longer runs arbitrary kubeconfig credential plugins without permission
- Security: peel `env -S` / `--split-string` operands in the Bash permission gate (managed deny/ask)
- Security: block unauthorized RCE via abused safe commands
- Security: block `rg --pre` arbitrary code execution in auto-mode
- Tools: make scheduler deletion durable
- Workflow: fix five workflow-runtime bugs (budget, pause, cancel, reconnect)
- Pager: stop stacking duplicate "Worked for" markers on parked turns
- Pager: recover image paste over grok wrap on headless remotes
- Doctor: fix for SSH wrap setup

### Areas touched

| Area | Files | +/− | Notes |
|------|------:|----:|-------|
| Shell (agent runtime) | 167 | +19642/−16719 | relocation, model providers, reasoning tiers, recap caching |
| Pager (TUI) | 266 | +19117/−4076 | doctor, prompt combine, external editor, diagnostics, Ctrl+B |
| Workspace / Permission | 14 | +3693/−225 | exec-risk scoring, auto-mode, shell access hardening |
| Worktree / GC | 7 | +3774/−127 | auto-GC TTLs, PID liveness, startup GC |
| Workflow (new crate) | 9 | +3174/−0 | durable workflow engine + journaling + validation |
| Config | 9 | +2847/−3 | new config types for workflow/GC knobs |
| Tools | 27 | +1989/−309 | scheduler durability, `/loop` serialization, skills reading |
| Chat state | 9 | +619/−29 | App Builder product selection |
| Pager render | 9 | +553/−85 | rendering updates |
| Pager PTY harness | 9 | +431/−94 | test harness updates |
| Voice | 8 | +315/−55 | silent-mic diagnostics, PCM processing |
| Sampler / Sampling types | 7 | +444/−74 | model provider plumbing |
| Prompt queue | 4 | +301/−4 | `combine_queued_prompts` batching |
| Sandbox | 2 | +121/−4 | Landlock without controlling TTY |
| Test support | 5 | +167/−113 | test infrastructure |
| Shared | 2 | +165/−65 | shared utilities |
| Subagent resolution | 2 | +41/−16 | subagent updates |
| Agent lifecycle | 2 | +31/−4 | agent identity |
| Shell base | 1 | +15/−15 | shell base updates |
| Hunk tracker | 1 | +13/−10 | file utils |
| Plugin marketplace | 1 | +12/−8 | marketplace updates |
| Tools API | 2 | +10/−8 | tool API updates |
| Tool runtime / protocol | 3 | +11/−18 | identifier validation, error conversion |
| Computer Hub | 2 | +9/−10 | notification, bridge |
| Textarea | 2 | +4/−2 | minor textarea adjustments |
| Markdown | 1 | +3/−6 | markdown updates |
| MCP | 1 | +3/−3 | MCP updates |
| Hooks | 1 | +1/−2 | hook updates |
| Memory | 1 | +1/−2 | memory updates |
| Version | 1 | +1/−1 | version bump |
| Root / meta | 3 | +116/−10 | Cargo.toml, Cargo.lock, SOURCE_REV |
| **Total** | **556** | **+56609/−21892** | |

### Merge risk for grok-pi

- **High:** `xai-grok-workspace/permission/` — exec-risk scoring, auto-mode, and shell-access hardening overlap with Pi-Grok's bash tool bridging and trust model. Review carefully during merge.
- **High:** `xai-grok-shell` (167 files, +19642/−16719) — massive churn in the agent runtime; relocation primitives, model providers, and reasoning tiers may shift APIs the adapter depends on.
- **Medium:** `xai-grok-pager` (266 files) — doctor, prompt combine, external editor, and diagnostics touch Pager surfaces that Pi-Grok maps to native components.
- **Low:** `xai-workflow` is a new isolated crate; `xai-prompt-queue/combine` is additive; voice/config changes are self-contained.
