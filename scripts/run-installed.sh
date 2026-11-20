#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GROK_ROOT="$ROOT"
BIN="${GROK_PI_BIN:-$GROK_ROOT/target/debug/grok-pi}"
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
if ! command -v "$PI_BIN" >/dev/null 2>&1; then
  echo "error: Pi executable not found: $PI_BIN" >&2
  exit 1
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
