# AGENTS.md — Pi-Grok Native TUI

## Project root and Git

`grok-build-main/` is the primary project checkout. Isolated integration work may use linked Git worktrees that share this checkout's Git common directory.

```text
origin   https://github.com/Dwsy/grok-pi.git
upstream https://github.com/xai-org/grok-build.git
base     07b2f7144fd5c5c9d3dd1966937a87852d2dbdb8
```

- Work from this directory; do not use its parent wrapper as a repository.
- `origin/main` is the Pi-Grok integration branch; `upstream` is read-only Grok Build.
- Never directly merge an upstream root commit into this repository without a migration plan. Reapply the narrow integration seams and validate them instead.
- Keep commits focused. Do not stage generated `pi-main` model catalog changes unless they are intentional.
- Pi host default is **system `pi` >= 0.84.3** (`npm i -g @earendil-works/pi-coding-agent`). Override with `--pi-bin` / `PI_BIN`.
- Optional Pi source checkout is the git submodule [`pi-main`](https://github.com/earendil-works/pi) (not a vendored copy). Follow [`pi-main/AGENTS.md`](pi-main/AGENTS.md) when working inside the submodule.

## Upstream sync workflow

Syncing upstream Grok Build is a **two-phase** process. Always record what
upstream changed *before* merging it.

1. **Fill the upstream update record first.** Run the
   [`upstream-changelog`](.pi/skills/upstream-changelog/SKILL.md) skill (trigger:
   `/skill:upstream-changelog`, or "上游更新记录" / "fill the upstream update
   list"). It fetches `upstream`, computes the pending range
   (`merge-base HEAD upstream/main .. upstream/main`), transcribes the
   `Changes:` list from each upstream commit message, and writes a structured
   English entry to [`docs/upstream/UPSTREAM_CHANGELOG.md`](docs/upstream/UPSTREAM_CHANGELOG.md).
   This step is **read-only** — it records changes but merges nothing.
2. **Then merge.** Only after the changelog entry exists, proceed with the
   isolated-worktree merge, seam reapplication, and validation (see the
   `docs/issues/架构/` sync issues). Never merge an upstream root commit
   directly into `main` without this plan.

Upstream commits are titled `Synced from monorepo` but each carries a full
`Changes:` bullet list and a `Source-Revision:` trailer — the changelog skill
transcribes those as the authoritative feature list (diff analysis is only a
fallback for commits lacking a `Changes:` list). `SOURCE_REV`, `AGENTS.md`
`base`, and verifier baselines are updated only after a completed, verified
merge — never by the changelog skill.

## Architecture invariants

1. **Grok Pager is the only terminal UI.** All visible terminal surfaces must come from `xai-grok-pager` or native component crates.
2. **Pi is the only agent core.** Pi owns models, providers, agent loop, tools, extensions, compaction, retries, and sessions.
3. **`pi-grok-adapter` is headless and library-only.** It may translate JSONL RPC ↔ ACP, but must not render widgets, own a terminal, read keyboard events, or depend on Ratatui/Crossterm.
4. **Reuse native Grok surfaces.** Map Pi capabilities to existing Pager prompt, slash, QuestionView, toast, banner, tool card, diff, and scrollback surfaces. Do not create a second TUI or ASCII fallback UI.
5. **Do not modify Pi source to extend RPC.** When a Pi core capability is not exposed over RPC, prefer the official extension API. Preserve Pi semantics rather than emulating them with JSONL edits or unrelated RPCs.
6. **Product-isolated state trees.** grok-pi must not share stock Grok’s user or project config roots (see [Product state isolation](#product-state-isolation)).

Read [`NATIVE_GROK_TUI_ALIGNMENT.md`](docs/NATIVE_GROK_TUI_ALIGNMENT.md) and [`FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md) before changing protocol or UI behavior.

## Product state isolation

Stock Grok uses `~/.grok` (user) and `<repo>/.grok` (project). **grok-pi defaults are product-isolated** so UI settings, trust, skills, hooks, and workflows do not collide with stock Grok:

| Layer | stock Grok | grok-pi default | Override |
|---|---|---|---|
| User home | `~/.grok` | `~/.grok-pi` | `$GROK_HOME` |
| Project tree | `<repo>/.grok` | `<repo>/.grok-pi` | `$GROK_PROJECT_DIR` |

Rules:

- `ensure_default_grok_home()` (grok-pi startup) sets `$GROK_HOME` → `~/.grok-pi` and `$GROK_PROJECT_DIR` → `.grok-pi` when unset.
- Resolve project paths only via `xai_grok_config::project_config_dirname()` / `project_config_dir(root)` — never hardcode `.join(".grok")` for project assets in grok-pi production code.
- User paths go through `grok_home()` / `$GROK_HOME` (workflows → `$GROK_HOME/workflows`, config → `$GROK_HOME/config.toml`, etc.).
- **No dual-scan of stock trees by default.** grok-pi does not auto-read `~/.grok` or `<repo>/.grok` for project discovery; migrate with `grok-pi migrate-home` (allowlisted user files only — **not** `workflows/`) or copy project trees manually into `.grok-pi`.
- Unit tests without env keep the stock default `.grok` so upstream-style tests stay green.
- Design note: [`docs/issues/架构/20260722-项目级.grok-pi隔离.md`](docs/issues/架构/20260722-项目级.grok-pi隔离.md).

Examples under a git repo:

```text
~/.grok-pi/config.toml              # F2 / UI (e.g. [ui].pi_workflows)
~/.grok-pi/workflows/*.rhai         # user workflows
<repo>/.grok-pi/workflows/*.rhai    # project workflows (folder trust)
<repo>/.grok-pi/hooks/              # project hooks
<repo>/.grok-pi/config.toml         # project config overlay
```

## Important paths

| Concern | Path |
|---|---|
| Composition binary | `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` |
| Default home / project dirname | `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/home.rs` |
| Path helpers (`grok_home`, `project_config_dir`) | `crates/codegen/xai-grok-config/src/paths.rs` |
| Pi JSONL RPC transport | `crates/codegen/pi-grok-adapter/src/pi_rpc.rs` |
| Pi data parsers | `crates/codegen/pi-grok-adapter/src/model.rs` |
| ACP adapter and UI mapping | `crates/codegen/pi-grok-adapter/src/pi_adapter.rs` |
| Native Pager external-profile seams | `crates/codegen/xai-grok-pager/src/app/` |
| Pi RPC facts (submodule / installed package) | `pi-main/packages/coding-agent/src/modes/rpc/` or npm package dist |
| Pi session lifecycle facts | `pi-main/packages/coding-agent/src/core/agent-session.ts` |
| Architecture and task records | `docs/` |
| Upstream update record (changelog) | `docs/upstream/UPSTREAM_CHANGELOG.md` |
| Upstream changelog skill | `.pi/skills/upstream-changelog/SKILL.md` |
| Project `.grok-pi` isolation issue | `docs/issues/架构/20260722-项目级.grok-pi隔离.md` |

## Session and tree rules

- Pi owns session files, trees, and the active leaf.
- `/resume` must use the native Grok `SessionPicker`; catalog scanning is on-demand, never startup work.
- Respect Pi's default session root, `--session-dir`, `PI_CODING_AGENT_SESSION_DIR`, and `sessionFile`-derived custom directories.
- `navigateTree()` changes Pi's in-memory leaf and context. Do not fake it with `fork`, `switch_session`, or direct JSONL mutation.
- If tree navigation is added without changing Pi source, bridge the official Pi extension API (`ctx.navigateTree`) and render only with native Grok components.

## Build and verification

Run from the project root:

```bash
./build.sh
./scripts/cargo-shared.sh test -p pi-grok-adapter
./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi
./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
```

`./build.sh` builds `grok-pi`. When the optional `pi-main` submodule has workspace dependencies installed (`pi-main/node_modules/.bin/tsgo`), it rebuilds the coding-agent checkout first; a freshly initialized, unprovisioned submodule is skipped. Requires system Pi >= **0.84.3**, Node.js >= 22.19.0, and the repository Rust toolchain.

All linked worktrees share one Cargo output tree at `<git-common-dir>/pi-grok-cargo-target`. `./build.sh` and `./verify.sh` initialize the root `target` symlink automatically. Use `./scripts/cargo-shared.sh <cargo-args>` for project Cargo commands: it sets up the shared target, enables incremental compilation by default, caps generated target output at 128 GiB, and stops Cargo at the default 20 GiB free-space floor. Override the target cap with `CARGO_TARGET_MAX_GIB`; when a target is already over the cap, periodic maintenance clears incremental roots first and falls back to `cargo clean` if needed. Raise the free-space floor with `CARGO_MIN_FREE_GIB`; use `CARGO_DISK_GUARD_PATH` when output is on a custom filesystem; set `CARGO_MAINTENANCE=0` to skip one pre-command maintenance pass. The running disk guard continuously enforces the free-space floor; target-size maintenance runs on its configured cadence. An explicit `CARGO_TARGET_DIR` remains authoritative for CI or one-off isolation. Never copy `target/` between worktrees. Direct raw `cargo` remains available for deliberate recovery/maintenance, but is not protected by the project guard.

`./verify.sh` additionally runs architecture, mock, syntax, and Pager checks. Current known blockers are documented in [`VERIFICATION.md`](docs/VERIFICATION.md): Python tree-sitter dependencies are not provisioned, and several source-identity/mock expectations require deliberate baseline maintenance. Focused Pager lib tests compile and pass after the `47348d1` integration. Do not claim full verification is green unless the remaining blockers are resolved.

For a standalone change under `extensions/`, validate the extension source and diff only; do **not** run Cargo unless Rust code, the embedded-extension loader, or its Rust contract changed, or the user asks.

Before reporting completion:

1. Run the narrowest relevant tests and build/check.
2. Read the exit status and complete output.
3. Review the diff for scope and whitespace errors.
4. State known blockers separately from passing checks.

## Diagnosing Pi RPC bootstrap / extension failures

When Pi RPC bootstrap fails and grok-pi self-heals by dropping an extension or
relaunching with `-ne`, do not stop at the reported culprit name — the bisect
only names a file (e.g. `index.ts`), never the real error. Follow this order:

1. **Read the real Pi error first.** Pi's complete stderr is appended to
   `{GROK_HOME}/logs/pi-rpc-stderr.log` (default `~/.grok-pi/logs/pi-rpc-stderr.log`)
   by `pi_rpc.rs`'s stderr reader. This is the authoritative failure message
   (e.g. `Cannot find module './eval-tasks.ts'`). Trace it to the exact line
   before touching any Rust injector.
2. **Isolate the injected bundle.** Injected extensions live in
   `$TMPDIR/pi-grok-bash-*` (`NSTemporaryDirectory` on macOS) and are deleted by
   self-heal after the crash, so reproduce a clean copy before the directory
   disappears. Verify the bundle loads in isolation:
   `pi -ne --mode rpc --extension <bundle>/index.ts` (must exit 0).
3. **Check injection completeness.** The Rust injector
   (`bash_extension.rs` / `*_extension.rs`) must materialize **every** relative
   import of the TypeScript entry, and the transitive closure of those modules.
   Known regression: splitting the Bash extension into multiple modules
   (`eval.ts`, `bash-tasks.ts`, `eval-tasks.ts`, `prompts.ts`, `shared.ts`,
   `tool-bridge.ts`) added imports the injector did not initially copy, causing
   `Cannot find module './eval-tasks.ts'` at boot. When adding a module, mirror
   it into the injector and extend the injector's single unit test to assert the
   new import and module content.
4. **Rule out tool-name conflicts separately.** `Tool "bash" conflicts with …`
   comes from Pi's `resource-loader.ts` extension-conflict check (two extension
   registrations), independent of bundle loading. It only appears when Pi
   auto-discovers another extension, so reproduce with `-ne` to separate bundle
   errors from cross-extension conflicts.

## Documentation and change control

- Complex work must have a record in `docs/issues/` before implementation.
- Update the relevant Issue after each completed phase.
- Keep `README.md`, `docs/README.zh-CN.md`, `docs/FEATURE_MATRIX.md`, and `docs/VERIFICATION.md` aligned with actual behavior.
- The source-identity verifier allows only declared Pager seams. If a required native seam changes, update its baseline/allowed-seam metadata deliberately; never weaken the verifier broadly.

## Safety

- Do not run destructive Git commands (`reset --hard`, broad restore, force checkout).
- Do not remove source files with `rm`; use `trash` for intentional deletion.
- Do not push, rename, delete, or recreate remote repositories unless the user explicitly authorizes it.
- Treat credentials, tokens, and user session data as private.
