#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
BIN="${GROK_PI_BIN:-$GROK_ROOT/target/debug/grok-pi}"
# Default: system `pi` (min 0.84.3). Override with PI_BIN for a custom CLI.
PI_BIN="${PI_BIN:-pi}"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <project-directory> [Pi arguments...]" >&2
  exit 2
fi
PROJECT_DIR="$(cd "$1" && pwd)"
shift

if [[ ! -x "$BIN" ]]; then
  echo "error: grok-pi is not built: $BIN" >&2
  echo "run ./build.sh first" >&2
  exit 1
fi
if [[ ! -e "$PI_BIN" ]] && ! command -v "$PI_BIN" >/dev/null 2>&1; then
  echo "error: Pi executable not found: $PI_BIN" >&2
  echo "install Pi >= 0.84.3 (npm i -g @earendil-works/pi-coding-agent) or set PI_BIN" >&2
  exit 1
fi

# Remote TUI / Bash default ON inside grok-pi (disable with =0).
if [[ "${PI_GROK_REMOTE_TUI:-1}" != "0" ]]; then
  if [[ ! -f "$GROK_ROOT/extensions/pi-grok-remote-tui/index.ts" ]]; then
    echo "warning: Remote TUI enabled but extensions/pi-grok-remote-tui/index.ts missing" >&2
  else
    echo "Remote TUI: ON · PI_BIN=$PI_BIN (min Pi 0.84.3)" >&2
  fi
fi

ui_args=()
[[ "${GROK_PI_MINIMAL:-0}" == "1" ]] && ui_args+=(--minimal)
[[ "${GROK_PI_FULLSCREEN:-0}" == "1" ]] && ui_args+=(--fullscreen)
[[ "${GROK_PI_NO_ALT_SCREEN:-0}" == "1" ]] && ui_args+=(--no-alt-screen)

exec "$BIN" \
  --pi-bin "$PI_BIN" \
  --pi-cwd "$PROJECT_DIR" \
  "${ui_args[@]}" \
  -- "$@"
