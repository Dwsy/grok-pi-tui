#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Cargo has no native target-cache size limit. Stop before the filesystem gets
# critically low instead of letting a large build damage unrelated user data.
GUARD_PATH="${CARGO_DISK_GUARD_PATH:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}}"
CARGO_HOME_PATH="${CARGO_HOME:-$HOME/.cargo}"
MIN_FREE_GIB="${CARGO_MIN_FREE_GIB:-20}"
MAX_TARGET_GIB="${CARGO_TARGET_MAX_GIB:-64}"
if ! [[ "$MAX_TARGET_GIB" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: CARGO_TARGET_MAX_GIB must be a positive integer" >&2
  exit 1
fi
"$SCRIPT_DIR/setup-shared-cargo-target.sh" >&2
if [[ "${CARGO_MAINTENANCE:-1}" != "0" ]]; then
  python3 "$SCRIPT_DIR/cargo-maintenance.py" \
    --target-dir "$GUARD_PATH" \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    --max-target-gib "$MAX_TARGET_GIB" \
    --interval-hours "${CARGO_MAINTENANCE_INTERVAL_HOURS:-6}" \
    --stale-days "${CARGO_MAINTENANCE_STALE_DAYS:-7}" \
    --soft-free-gib "${CARGO_MAINTENANCE_SOFT_FREE_GIB:-40}" \
    --max-dirs "${CARGO_MAINTENANCE_MAX_DIRS:-8}"
fi
# Free-space is cheap to sample continuously. The more expensive recursive
# target-size cap is enforced by cargo-maintenance.py on its periodic cadence.
exec python3 "$SCRIPT_DIR/cargo-disk-guard.py" \
  --path "$GUARD_PATH" \
  --path "$CARGO_HOME_PATH" \
  --min-free-gib "$MIN_FREE_GIB" \
  -- cargo "$@"
