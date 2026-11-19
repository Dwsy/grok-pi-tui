<p align="center">
  <img width="48%" alt="grok-pi main TUI" src="https://github.com/user-attachments/assets/27ff0124-c1e1-4948-8c2c-9880e6a38732" />
  <img width="48%" alt="grok-pi native views" src="https://github.com/user-attachments/assets/83abc999-08fe-459e-ab06-935eeb0780e6" />
</p>

# grok-pi

**One native terminal UI. One unmodified agent core.**

`grok-pi` bridges [Pi](https://pi.dev) into Grok Build's native Rust TUI (`xai-grok-pager`). Pi owns models, tools, extensions, skills, and sessions; Grok Pager owns every pixel and keystroke.

[Releases](https://github.com/Dwsy/grok-pi-tui/releases/latest) · [中文文档](docs/README.zh-CN.md) · [Feature Matrix](docs/FEATURE_MATRIX.md) · [Architecture](docs/NATIVE_GROK_TUI_ALIGNMENT.md) · [Verification](docs/VERIFICATION.md) · [Changelog](CHANGELOG.MD)

---

## Philosophy

1. **Zero UI Compromise** — All surfaces (scrollback, diffs, tool cards, dialogs, Remote TUI) render natively through `xai-grok-pager`. No second TUI, no ASCII fallback.
2. **Zero Pi Forking** — Pi runs unmodified (`>= 0.99.0`). Capabilities outside Pi RPC are bridged through Pi's official extension API.
3. **Headless Translation** — `pi-grok-adapter` only translates JSONL RPC ↔ ACP. It never touches the terminal or owns UI state.
4. **Product Isolation** — State lives in `~/.grok-pi` and `<repo>/.grok-pi` (`$GROK_HOME` / `$GROK_PROJECT_DIR`), isolated from stock Grok.

```mermaid
flowchart LR
    User[Terminal] <--> Pager[Grok Pager · Native Rust TUI]
    Pager <-->|ACP| Adapter[pi-grok-adapter · Headless Bridge]
    Adapter <-->|JSONL RPC| Pi[Pi · Unmodified Agent Core]
```

---

## Install & Start

Requires **Pi `>= 0.99.0`** (`curl -fsSL https://pi.dev/install.sh | sh` or `npm i -g @earendil-works/pi-coding-agent`).

**macOS / Linux:**
```bash
curl -fsSL https://github.com/Dwsy/grok-pi-tui/releases/latest/download/install.sh | sh
```

**Windows (PowerShell):**
```powershell
irm https://github.com/Dwsy/grok-pi-tui/releases/latest/download/install.ps1 | iex
```

Run from any project directory (`pig` and `pi-grok` are installed as aliases):
```bash
grok-pi                 # start new session (or: pig / pi-grok)
grok-pi --continue      # resume previous session
grok-pi update --channel beta   # switch update channel (stable | beta)
```

---

## Capabilities

| Area | What you get |
|---|---|
| **Native Pager** | Streaming Markdown, live file diffs, single-click card folding, F2 settings, Mermaid PNG/SVG (`Ondine`), Kitty/iTerm2 image viewer, `@file` & skill previews |
| **Model & Config** | `/model` fast switcher, `/pi-models` transactional editor, `/pi-config web` local workbench, per-project usage ranking |
| **Subagents & Plan Mode** | Foreground/background subagents, `fork: true` context inheritance, Team V2 (`spawn_team`, peer messaging, nested spawn), `enter_plan_mode` / `exit_plan_mode` |
| **Shell & Eval** | Managed Bash (`system` default, optional `PI_GROK_BASH_BACKEND=omp` with per-command trace cards), persistent JS/Python `eval`, background tasks & process-tree cleanup |
| **Workflows & Sessions** | Rhai workflows (`~/.grok-pi/workflows`, `<repo>/.grok-pi/workflows`), native `/resume` tree picker, compaction & recap |

---

## Key Options & Environment

| Flag / Variable | Default | Purpose |
|---|---|---|
| `--continue` / `--pi-bin` | — | Resume session / override `pi` binary (`$PI_BIN`) |
| `--no-extensions` (`-ne`) / `--no-bridge-extensions` | off | Disable Pi auto-discovery / bundled host bridges |
| `--extensions-dev <DIR>` | — | Load bundled extensions from disk without rebuilding |
| `PI_GROK_BASH_BACKEND` | `system` | Shell backend: `system` (`/bin/bash`) or `omp` (`grok-pi-shell-host`) |
| `PI_GROK_SUBAGENTS_V2` | `1` | Enable Team V2 subagent surface |
| `PI_GROK_REMOTE_TUI` | `1` | Bridge Pi `ctx.ui.custom` components to native Pager |
| `GROK_HOME` / `GROK_PROJECT_DIR` | `~/.grok-pi` / `.grok-pi` | Isolated user and project state directories |

Pass raw Pi CLI flags after `--` (e.g. `grok-pi -- --model openai/gpt-4o`).

---

## Build from Source

Requires Rust **1.97.0**, Node.js **>= 22.19.0**, `protoc`, and system Pi:

```bash
./build.sh
./target/debug/grok-pi
./verify.sh
```

See [FEATURE_MATRIX.md](docs/FEATURE_MATRIX.md), [VERIFICATION.md](docs/VERIFICATION.md), [CHANGELOG.MD](CHANGELOG.MD), and [LICENSE](LICENSE).
