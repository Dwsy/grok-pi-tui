# AGENTS.md — grok-pi (Pi-Grok Native TUI)

## Repository and Git

This checkout (`pi-grok-build/`) is the primary repository root. Linked Git worktrees may share its Git common directory.

```text
origin   https://github.com/Dwsy/grok-pi-tui.git   # Pi-Grok integration branch (main)
upstream https://github.com/xai-org/grok-build.git # Read-only upstream Grok Build
```

- `git merge-base HEAD upstream/main` tracks the synced upstream commit; `SOURCE_REV` holds the corresponding upstream monorepo SHA.
- Never merge an upstream commit directly into `main` without first recording upstream changes via [`.pi/skills/upstream-changelog/SKILL.md`](.pi/skills/upstream-changelog/SKILL.md) (`docs/upstream/UPSTREAM_CHANGELOG.md`) and planning seam reapplication in an isolated worktree.
- Never stage or commit `pi-main` submodule pointer changes unless explicitly requested.
- Default Pi host is system `pi >= 0.99.0` (`npm i -g @earendil-works/pi-coding-agent`), overridable via `--pi-bin` / `PI_BIN`. Optional Pi source lives in the `pi-main` git submodule (follow `pi-main/AGENTS.md` when editing inside it).

## Architecture Invariants

1. **Grok Pager is the only terminal UI.** All visible terminal surfaces come from `xai-grok-pager` or native component crates.
2. **Pi is the only agent core.** Pi owns models, providers, agent loop, tools, extensions, compaction, retries, and sessions.
3. **`pi-grok-adapter` is headless and library-only.** Translates JSONL RPC ↔ ACP only; never owns a terminal, reads keyboard events, or depends on Ratatui/Crossterm.
4. **Reuse native Grok surfaces.** Map Pi capabilities to existing Pager prompt, slash, QuestionView, toast, banner, tool card, diff, and scrollback surfaces — no second TUI or ASCII fallback.
5. **Do not modify Pi source to extend RPC.** Prefer the official Pi extension API (`extensions/pi-grok-*`). Preserve Pi semantics rather than emulating them with direct JSONL edits.
6. **Product-isolated state trees.** Never share stock Grok's `~/.grok` or `<repo>/.grok` roots by default.

Read [`docs/NATIVE_GROK_TUI_ALIGNMENT.md`](docs/NATIVE_GROK_TUI_ALIGNMENT.md) and [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md) before changing protocol or UI behavior.

## Product State Isolation

| Layer | Stock Grok | grok-pi Default | Override |
|---|---|---|---|
| User home | `~/.grok` | `~/.grok-pi` | `$GROK_HOME` |
| Project tree | `<repo>/.grok` | `<repo>/.grok-pi` | `$GROK_PROJECT_DIR` |

- `ensure_default_grok_home()` sets `$GROK_HOME` → `~/.grok-pi` and `$GROK_PROJECT_DIR` → `.grok-pi` when unset.
- Resolve project paths exclusively via `xai_grok_config::project_config_dirname()` / `project_config_dir(root)` — never hardcode `.join(".grok")` in production code.
- No dual-scan of stock trees by default; unit tests without env keep `.grok` so upstream tests stay green.

## Key Paths

| Concern | Path |
|---|---|
| Composition binary | `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` |
| Home / project dir isolation | `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/home.rs`, `crates/codegen/xai-grok-config/src/paths.rs` |
| Pi JSONL RPC & ACP adapter | `crates/codegen/pi-grok-adapter/src/{pi_rpc.rs,model.rs,pi_adapter.rs}` |
| Native Pager & scrollback | `crates/codegen/xai-grok-pager/src/{app/,scrollback/,settings/}` |
| Bundled Pi extensions | `extensions/pi-grok-*/` |
| Upstream sync skill & log | `.pi/skills/upstream-changelog/SKILL.md`, `docs/upstream/UPSTREAM_CHANGELOG.md` |
| Repo RSI skill (pitfalls & runbooks) | `.pi/skills/grok-pi-rsi/SKILL.md` |
| Remote TUI & TS overlays RSI skill | `.pi/skills/grok-pi-remote-tui-rsi/SKILL.md` |
| Release runbook | `docs/usage/release.md` |

## Session and Tree Rules

- Pi owns session files, trees, and the active leaf.
- `/resume` uses the native Grok `SessionPicker`; catalog scanning is on-demand, never startup work.
- Respect Pi's default session root, `--session-dir`, `PI_CODING_AGENT_SESSION_DIR`, and `sessionFile`-derived custom directories.
- `navigateTree()` mutates Pi's in-memory leaf and context; never fake it with `fork`, `switch_session`, or direct JSONL mutation.

## Verification, CI, and Release

**Never run local `cargo` commands (`check`, `test`, `build`, `clippy`, `fmt`, `clean`) — directly or via wrapper scripts (`./build.sh`, `./verify.sh`, `./scripts/cargo-shared.sh`) — unless the user explicitly requests a local Cargo run.**

- **Default verification**: Perform thorough static review (imports, cfg gates, struct literals, exhaustive matches, test helpers) and non-Rust checks (`git diff --check`, Python/script validators). State explicitly which checks were not run locally.
- **Online Linux CI verification**: Verify Rust/Cargo changes on GitHub Actions Linux (`ubuntu-24.04`) via manual dispatch:
  ```bash
  gh workflow run pi-slim-linux-checks.yml --repo Dwsy/grok-pi-tui --ref <branch> -f scope=fast # or all
  ```
- **CI hygiene**: Keep non-release verification workflows manual-only (`on: workflow_dispatch:`, no `push`/`pull_request` triggers).
- **Beta release**: Add `## [X.Y.Z-beta.N] - YYYY-MM-DD` under `## [Unreleased]` in `CHANGELOG.MD`, validate with `python3 scripts/extract-changelog-section.py X.Y.Z-beta.N CHANGELOG.MD --strict -o /tmp/notes.md`, push `main`, then tag and push `vX.Y.Z-beta.N` (beta tags build macOS ARM64 prerelease by default; see [`docs/usage/release.md`](docs/usage/release.md)).

## Recurring Pitfalls and Runbooks

For recurring `pi-slim` feature-gating traps, F2 settings wiring checklists, Pi RPC extension-injection diagnosis (`{GROK_HOME}/logs/pi-rpc-stderr.log`), and Herdr lifecycle invariants, consult [`.pi/skills/grok-pi-rsi/SKILL.md`](.pi/skills/grok-pi-rsi/SKILL.md). For Remote TUI host/resize plumbing, TS overlay sizing/alignment, `jiti` startup/lazy imports, search debounce, and Codex-style TUI conventions, consult [`.pi/skills/grok-pi-remote-tui-rsi/SKILL.md`](.pi/skills/grok-pi-remote-tui-rsi/SKILL.md).

## Safety

- Do not run destructive Git commands (`reset --hard`, broad restore, force checkout).
- Do not remove source files with `rm`; use `trash` for intentional deletion.
- Do not push, tag, or mutate remote repositories unless the user explicitly authorizes it.
- Treat credentials, tokens, and user session data as private.
