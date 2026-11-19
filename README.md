# grok-pi

**Grok Build's native Rust TUI, powered by an unmodified Pi core — 100% compatible with Pi's extension ecosystem.**

`grok-pi` bridges [Pi](https://pi.dev) (`>= 0.99.0`) into `xai-grok-pager` via a headless JSONL RPC ↔ ACP adapter (`pi-grok-adapter`). Pi owns models, tools, sessions, and packages; Grok Pager owns every terminal surface.

[Releases](https://github.com/Dwsy/grok-pi-tui/releases/latest) · [ZH](docs/README.zh-CN.md) · [Feature Matrix](docs/FEATURE_MATRIX.md) · [Architecture](docs/NATIVE_GROK_TUI_ALIGNMENT.md) · [Verification](docs/VERIFICATION.md) · [Changelog](CHANGELOG.MD)

## Philosophy

- **Full Pi Ecosystem Compatibility** — Third-party Pi extensions, custom tools, hooks, skills, prompt templates, themes (`pi:<name>`), and npm/git packages run unmodified. `ctx.ui.custom` components and Pi UI dialogs (`select`, `input`, `confirm`, `editor`, status, widgets) project directly onto native Pager surfaces via the **Remote TUI bridge**.
- **Zero UI Compromise** — All rendering, scrollback, diffs, tool cards, and modals come from `xai-grok-pager`. No second TUI, no ASCII fallback.
- **Zero Pi Forking** — Pi runs as an untouched system binary (`>= 0.99.0`); host capabilities use Pi's official extension API.
- **Product Isolation** — User and project state live in `~/.grok-pi` and `<repo>/.grok-pi`, isolated from stock Grok.

## Core Features

- **Timeline** — `/timeline` toggles the fullscreen sidebar. `/timeline-config` selects prompts, compactions, file tools, subagent interactions, and custom tool names. Hovering a prompt shows the opening of its final reply and a bounded summary of enabled file changes. Saved rules apply to the loaded branch and live calls.
- **Pi Extensions & Remote TUI**: Full `ctx.ui.custom` keyboard/render/paste bridge, `/reload`, extension/skill/prompt/theme manager, native conflict policy overlays, and `pi:transparent` themes.
- **Native Pager & Viewers**: Streaming Markdown, live side-by-side/word-level diffs, 5-level tool call density, single-click folding, F2 settings, Mermaid PNG/SVG (`Ondine`), Kitty/iTerm2 image viewer, `@file` & skill previews.
- **Models & Web Workbench**: `/model` fast switcher, `/pi-models` transactional editor, `/pi-config web` local workbench, and per-project usage ranking for slash commands, skills, and models.
- **Subagents V2 & Plan Mode**: Foreground/background subagents, `fork: true` live-branch inheritance, Team V2 (`spawn_team`, peer messaging, nested spawn), `enter_plan_mode` / `exit_plan_mode` gating, and `/dashboard`.
- **Shell, Eval & Workflows**: Managed Bash (`system` default, optional `PI_GROK_BASH_BACKEND=omp` with per-command trace cards), persistent JS/Python `eval` (`Codemode` & `Eval v2`), background task promotion/kill, and Rhai workflows.

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
| `PI_GROK_REMOTE_TUI` | `1` | Bridge Pi `ctx.ui.custom` extensions to native Pager |
| `PI_GROK_BASH_BACKEND` | `system` | `system` (`/bin/bash`) or `omp` (`grok-pi-shell-host`) |
| `PI_GROK_SUBAGENTS_V2` | `1` | Enable Team V2 subagent surface |
| `GROK_HOME` / `GROK_PROJECT_DIR` | `~/.grok-pi` / `.grok-pi` | Isolated user and project state directories |

## Build from Source

Requires Rust **1.97.0**, Node.js **>= 22.19.0**, `protoc`, and system Pi:

```bash
./build.sh && ./target/debug/grok-pi
```

See [LICENSE](LICENSE) and [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES).
