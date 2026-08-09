#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR/.." rev-parse --show-toplevel)"

# Respect an explicit Cargo override. The repository-managed shared cache is the
# default, not a replacement for CI or developer-provided CARGO_TARGET_DIR.
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  printf 'Cargo target: using CARGO_TARGET_DIR=%s\n' "$CARGO_TARGET_DIR"
  exit 0
fi

if GIT_COMMON_DIR="$(git -C "$REPO_ROOT" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"; then
  :
else
  GIT_COMMON_DIR="$(git -C "$REPO_ROOT" rev-parse --git-common-dir)"
  if [[ "$GIT_COMMON_DIR" != /* ]]; then
    GIT_COMMON_DIR="$REPO_ROOT/$GIT_COMMON_DIR"
  fi
fi

SHARED_TARGET="$GIT_COMMON_DIR/pi-grok-cargo-target"
LOCAL_TARGET="$REPO_ROOT/target"
LOCK_DIR="$GIT_COMMON_DIR/pi-grok-cargo-target.setup-lock"
MIN_FREE_GIB="${CARGO_MIN_FREE_GIB:-20}"

if ! [[ "$MIN_FREE_GIB" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: CARGO_MIN_FREE_GIB must be a positive integer" >&2
  exit 1
fi

check_free_space() {
  local path="$1"
  local available_kib
  available_kib="$(df -Pk "$path" | awk 'NR == 2 { print $4 }')"
  if [[ -z "$available_kib" || "$available_kib" -lt $((MIN_FREE_GIB * 1024 * 1024)) ]]; then
    echo "error: refusing Cargo setup; less than ${MIN_FREE_GIB} GiB is free on $path" >&2
    echo "free space first, or set CARGO_MIN_FREE_GIB to a deliberate lower floor" >&2
    exit 74
  fi
}

check_free_space "$REPO_ROOT"
check_free_space "$GIT_COMMON_DIR"
LOCK_HELD=0

release_lock() {
  if [[ "$LOCK_HELD" == 1 ]]; then
    rmdir "$LOCK_DIR" 2>/dev/null || true
  fi
}
trap release_lock EXIT
trap 'release_lock; exit 130' INT
trap 'release_lock; exit 143' TERM

# Setup is normally sub-second. Serialize the rare first-run migration so two
# newly-created worktrees cannot both try to move their local target directory.
for _attempt in {1..100}; do
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    LOCK_HELD=1
    break
  fi
  sleep 0.1
done
if [[ "$LOCK_HELD" != 1 ]]; then
  echo "error: shared Cargo target setup is locked: $LOCK_DIR" >&2
  echo "another worktree may be initializing it; retry after that command exits" >&2
  exit 1
fi

if [[ -L "$LOCAL_TARGET" ]]; then
  if [[ ! -d "$LOCAL_TARGET" ]]; then
    LINK_TARGET="$(readlink "$LOCAL_TARGET")"
    if [[ "$LINK_TARGET" != /* ]]; then
      LINK_TARGET="$(dirname "$LOCAL_TARGET")/$LINK_TARGET"
    fi
    if [[ "$LINK_TARGET" != "$SHARED_TARGET" ]]; then
      echo "error: broken Cargo target symlink points outside the repository shared cache" >&2
      echo "expected: $SHARED_TARGET" >&2
      echo "actual:   $LINK_TARGET" >&2
      exit 1
    fi
    mkdir -p "$SHARED_TARGET"
  fi
  LOCAL_REAL="$(cd "$LOCAL_TARGET" && pwd -P)"
  SHARED_REAL="$(cd "$SHARED_TARGET" 2>/dev/null && pwd -P || true)"
  if [[ -z "$SHARED_REAL" || "$LOCAL_REAL" != "$SHARED_REAL" ]]; then
    echo "error: $LOCAL_TARGET points outside the repository shared cache" >&2
    echo "expected: $SHARED_TARGET" >&2
    echo "actual:   $LOCAL_REAL" >&2
    exit 1
  fi
  printf 'Cargo target: %s -> %s\n' "$LOCAL_TARGET" "$SHARED_TARGET"
  exit 0
fi

if [[ -e "$LOCAL_TARGET" && ! -d "$LOCAL_TARGET" ]]; then
  echo "error: Cargo target path is not a directory or symlink: $LOCAL_TARGET" >&2
  exit 1
fi
if [[ -e "$SHARED_TARGET" && ! -d "$SHARED_TARGET" ]]; then
  echo "error: shared Cargo target path is not a directory: $SHARED_TARGET" >&2
  exit 1
fi

if [[ -d "$LOCAL_TARGET" && ! -d "$SHARED_TARGET" ]]; then
  # Move through a temporary name in the common Git directory. On the normal
  # same-volume worktree layout this is atomic; across filesystems `mv` copies
  # completely before removing the source and the final rename stays atomic.
  MIGRATING_TARGET="$SHARED_TARGET.migrating.$$"
  if [[ -e "$MIGRATING_TARGET" ]]; then
    echo "error: stale shared Cargo migration path exists: $MIGRATING_TARGET" >&2
    exit 1
  fi
  echo "Migrating Cargo target into the shared Git common directory..."
  mv "$LOCAL_TARGET" "$MIGRATING_TARGET"
  mv "$MIGRATING_TARGET" "$SHARED_TARGET"
elif [[ -d "$LOCAL_TARGET" && -d "$SHARED_TARGET" ]]; then
  # Both trees contain generated artifacts. Keep the common cache and discard
  # only the redundant worktree-local Cargo output through Cargo itself.
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: both local and shared Cargo targets exist, but cargo is unavailable" >&2
    echo "clean the generated local target, then rerun this script" >&2
    exit 1
  fi
  echo "Cleaning redundant worktree-local Cargo target..."
  cargo clean --manifest-path "$REPO_ROOT/Cargo.toml" --target-dir "$LOCAL_TARGET"
  rmdir "$LOCAL_TARGET"
elif [[ ! -d "$SHARED_TARGET" ]]; then
  mkdir -p "$SHARED_TARGET"
fi

ln -s "$SHARED_TARGET" "$LOCAL_TARGET"
printf 'Cargo target: %s -> %s\n' "$LOCAL_TARGET" "$SHARED_TARGET"
