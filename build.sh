#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
# Default host: system `pi` (min 0.84.3). Optional: PI_BIN=/path/to/cli.js
PI_BIN="${PI_BIN:-pi}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust/Cargo is required" >&2
  exit 1
fi

"$ROOT/scripts/setup-shared-cargo-target.sh"

# Optional: rebuild the locked pi-main coding-agent checkout when its workspace
# dependencies are provisioned. A freshly initialized submodule has no
# node_modules and must not prevent the Rust composition binary from building.
PI_MAIN_ROOT="$GROK_ROOT/pi-main"
if [[ -f "$PI_MAIN_ROOT/packages/coding-agent/package.json" ]]; then
  if [[ -x "$PI_MAIN_ROOT/node_modules/.bin/tsgo" ]]; then
    echo "Building pi-main coding-agent (submodule, optional)..."
    (cd "$PI_MAIN_ROOT/packages/coding-agent" && npm run build)
  else
    echo "Skipping optional pi-main build (run 'npm ci' in $PI_MAIN_ROOT to provision it)."
  fi
fi

(cd "$GROK_ROOT" && "$ROOT/scripts/cargo-shared.sh" build -p xai-grok-pager-bin --bin grok-pi)

echo "Built: $GROK_ROOT/target/debug/grok-pi"
echo "Pi:    $PI_BIN (min compatible 0.84.3)"
