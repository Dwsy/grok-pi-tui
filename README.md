# grok-pi

**One native terminal UI. One unmodified agent core.**

`grok-pi` runs [Pi](https://pi.dev) inside Grok Build's native Rust TUI (`xai-grok-pager`) via a headless JSONL RPC ↔ ACP bridge (`pi-grok-adapter`).

[Releases](https://github.com/Dwsy/grok-pi-tui/releases/latest) · [ZH](docs/README.zh-CN.md) · [Feature Matrix](docs/FEATURE_MATRIX.md) · [Architecture](docs/NATIVE_GROK_TUI_ALIGNMENT.md) · [Verification](docs/VERIFICATION.md) · [Changelog](CHANGELOG.MD)

## Philosophy

- **Zero UI Compromise** — Every visible surface renders natively in `xai-grok-pager`. No second TUI, no ASCII fallback.
- **Zero Pi Forking** — Pi runs unmodified (`>= 0.99.0`). Host capabilities bridge through Pi's official extension API.
- **Headless Bridge** — `pi-grok-adapter` translates JSONL RPC ↔ ACP only, with no terminal or UI ownership.
- **Product Isolation** — State lives in `~/.grok-pi` and `<repo>/.grok-pi`, isolated from stock Grok.

## Core Features

- **Native TUI & Viewers**: Streaming Markdown, live file diffs, single-click card folding, F2 settings, Mermaid PNG/SVG (`Ondine`), Kitty/iTerm2 image viewer, `@file` & skill previews.
- **Models & Config**: `/model` fast switcher, `/pi-models` transactional editor, `/pi-config web` local workbench, per-project usage ranking.
- **Subagents & Plan Mode**: Foreground/background subagents, `fork: true` context inheritance, Team V2 (`spawn_team`, peer messaging, nested spawn), `enter_plan_mode` / `exit_plan_mode`.
- **Shell & Eval**: Managed Bash (`system` default, optional `PI_GROK_BASH_BACKEND=omp` with per-command trace cards), persistent JS/Python `eval`, background task lifecycle & process-tree cleanup.
- **Workflows & Sessions**: Rhai workflows (`~/.grok-pi/workflows`, `<repo>/.grok-pi/workflows`), native `/resume` tree picker, compaction & recap.

## Install & Start

Requires **Pi `>= 0.99.0`** (`curl -fsSL https://pi.dev/install.sh | sh`).

```bash
# macOS / Linux
curl -fsSL https://github.com/Dwsy/grok-pi-tui/releases/latest/download/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://github.com/Dwsy/grok-pi-tui/releases/latest/download/install.ps1 | iex
```

```bash
grok-pi                         # start (aliases: pig, pi-grok)
grok-pi --continue              # resume last session
grok-pi update --channel beta   # stable | beta
```

## Key Options & Env

| Option / Env | Default | Purpose |
|---|---|---|
| `--continue` / `--pi-bin` | — | Resume session / override `pi` binary (`$PI_BIN`) |
| `--no-extensions` (`-ne`) / `--no-bridge-extensions` | off | Disable Pi auto-discovery / bundled host bridges |
| `--extensions-dev <DIR>` | — | Load bundled extensions from disk without rebuilding |
| `PI_GROK_BASH_BACKEND` | `system` | `system` (`/bin/bash`) or `omp` (`grok-pi-shell-host`) |
| `PI_GROK_SUBAGENTS_V2` / `PI_GROK_REMOTE_TUI` | `1` / `1` | Team V2 subagents / Pi `ctx.ui.custom` native bridge |
| `GROK_HOME` / `GROK_PROJECT_DIR` | `~/.grok-pi` / `.grok-pi` | Isolated user and project state directories |

## Build from Source

Requires Rust **1.97.0**, Node.js **>= 22.19.0**, `protoc`, and system Pi:

```bash
./build.sh && ./target/debug/grok-pi
```

See [LICENSE](LICENSE) and [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES).
