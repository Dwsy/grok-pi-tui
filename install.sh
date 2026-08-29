#!/usr/bin/env sh
# grok-pi installer (Unix)
#
# One-line install (latest):
#   curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh
#
# Pin a version:
#   curl -fsSL https://github.com/Dwsy/grok-pi/releases/download/v0.0.1/install.sh | GROK_PI_VERSION=v0.0.1 sh
#
# Env overrides:
#   GROK_PI_VERSION=v0.0.1|latest     (default: latest)
#   GROK_PI_INSTALL_DIR=$HOME/.local/bin
#   GROK_PI_REPO=Dwsy/grok-pi
#   GROK_PI_SKIP_PI_HINT=1            skip Pi host install hint
#   GROK_PI_FORCE=1                   reinstall even if already present
#
# Supported release assets:
#   grok-pi-macos-aarch64.tar.gz
#   grok-pi-macos-x86_64.tar.gz
#   grok-pi-linux-x86_64.tar.gz
#   grok-pi-linux-aarch64.tar.gz
set -eu

REPOSITORY="${GROK_PI_REPO:-Dwsy/grok-pi}"
VERSION="${GROK_PI_VERSION:-latest}"
INSTALL_DIR="${GROK_PI_INSTALL_DIR:-$HOME/.local/bin}"
SKIP_PI_HINT="${GROK_PI_SKIP_PI_HINT:-0}"
FORCE="${GROK_PI_FORCE:-0}"

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

info() {
  printf '%s\n' "$*"
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "'$1' is required"
}

normalize_arch() {
  # stdout: aarch64 | x86_64
  m="$(uname -m)"
  case "$m" in
    arm64|aarch64) printf '%s\n' "aarch64" ;;
    x86_64|amd64) printf '%s\n' "x86_64" ;;
    *) fail "unsupported CPU architecture: $m" ;;
  esac
}

detect_asset() {
  os="$(uname -s)"
  arch="$(normalize_arch)"
  case "$os" in
    Darwin)
      printf '%s\n' "grok-pi-macos-${arch}.tar.gz"
      ;;
    Linux)
      printf '%s\n' "grok-pi-linux-${arch}.tar.gz"
      ;;
    *)
      fail "$os is unsupported on this installer; on Windows use install.ps1"
      ;;
  esac
}

resolve_url() {
  asset="$1"
  case "$VERSION" in
    latest)
      printf '%s\n' "https://github.com/${REPOSITORY}/releases/latest/download/${asset}"
      ;;
    v*)
      printf '%s\n' "https://github.com/${REPOSITORY}/releases/download/${VERSION}/${asset}"
      ;;
    *)
      # Allow bare semver like 0.0.9 → v0.0.9
      case "$VERSION" in
        [0-9]*)
          printf '%s\n' "https://github.com/${REPOSITORY}/releases/download/v${VERSION}/${asset}"
          ;;
        *)
          fail "GROK_PI_VERSION must be 'latest', 'vX.Y.Z', or 'X.Y.Z' (got: $VERSION)"
          ;;
      esac
      ;;
  esac
}

path_has_dir() {
  dir="$1"
  case ":${PATH}:" in
    *":${dir}:"*) return 0 ;;
    *) return 1 ;;
  esac
}

print_path_hint() {
  dir="$1"
  if path_has_dir "$dir"; then
    return 0
  fi
  info ""
  info "Add $dir to PATH, then open a new terminal:"
  info "  export PATH=\"$dir:\$PATH\""
  shell_name="$(basename "${SHELL:-}")"
  case "$shell_name" in
    zsh)  rc="$HOME/.zshrc" ;;
    bash) rc="$HOME/.bashrc" ;;
    fish) rc="$HOME/.config/fish/config.fish" ;;
    *)    rc="" ;;
  esac
  if [ -n "$rc" ] && [ "$shell_name" != "fish" ]; then
    info "  # optional permanent:"
    info "  echo 'export PATH=\"$dir:\$PATH\"' >> $rc"
  elif [ "$shell_name" = "fish" ]; then
    info "  # optional permanent (fish):"
    info "  fish_add_path $dir"
  fi
}

check_pi_host() {
  if [ "$SKIP_PI_HINT" = "1" ]; then
    return 0
  fi
  info ""
  if command -v pi >/dev/null 2>&1; then
    ver="$(pi --version 2>/dev/null | head -n 1 || true)"
    if [ -n "$ver" ]; then
      info "Pi host found: $ver"
    else
      info "Pi host found on PATH."
    fi
    return 0
  fi
  info "Pi host not found on PATH (required: Pi >= 0.84.3)."
  info "Install Pi (recommended):"
  info "  curl -fsSL https://pi.dev/install.sh | sh"
  info "Or:"
  info "  npm install --global @earendil-works/pi-coding-agent"
}

# ── main ────────────────────────────────────────────────────────────────────

need_cmd curl
need_cmd tar
need_cmd uname
need_cmd mktemp

asset="$(detect_asset)"
url="$(resolve_url "$asset")"

info "grok-pi installer"
info "  repo:    $REPOSITORY"
info "  version: $VERSION"
info "  asset:   $asset"
info "  install: $INSTALL_DIR/grok-pi"
info ""

if [ -x "$INSTALL_DIR/grok-pi" ] && [ "$FORCE" != "1" ]; then
  existing="$("$INSTALL_DIR/grok-pi" --version 2>/dev/null | head -n 1 || true)"
  if [ -n "$existing" ]; then
    info "Existing install: $existing"
    info "Reinstall with GROK_PI_FORCE=1 if needed."
  fi
fi

tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t grok-pi-install)"
cleanup() {
  rm -rf "$tmpdir"
}
trap cleanup EXIT INT HUP TERM

info "Downloading $url ..."
# --fail: HTTP errors; --location: redirects; --retry for flaky links
if ! curl --fail --location --retry 3 --retry-delay 1 --progress-bar --show-error \
  "$url" -o "$tmpdir/$asset"; then
  fail "download failed for $asset ($VERSION). Check that this platform asset exists on the release."
fi

tar -xzf "$tmpdir/$asset" -C "$tmpdir"

if [ ! -f "$tmpdir/grok-pi" ]; then
  fail "archive did not contain grok-pi"
fi

mkdir -p "$INSTALL_DIR"
if command -v install >/dev/null 2>&1; then
  install -m 755 "$tmpdir/grok-pi" "$INSTALL_DIR/grok-pi"
else
  cp "$tmpdir/grok-pi" "$INSTALL_DIR/grok-pi"
  chmod 755 "$INSTALL_DIR/grok-pi"
fi

# Convenience alias used by some docs / muscle memory.
ln -sf "$INSTALL_DIR/grok-pi" "$INSTALL_DIR/pi-grok"
ln -sf "$INSTALL_DIR/grok-pi" "$INSTALL_DIR/pig"

info ""
info "Installed $INSTALL_DIR/grok-pi (aliases: pig, pi-grok)"

if "$INSTALL_DIR/grok-pi" --help >/dev/null 2>&1; then
  info "Binary responds to --help."
else
  info "warning: binary installed but --help probe failed (still may work in a TTY)."
fi

print_path_hint "$INSTALL_DIR"
check_pi_host

info ""
info "Run:"
info "  grok-pi"
info "  # or: pig"
info "  # or: pi-grok"
info "  # continue last session: grok-pi --continue"
info "  # custom Pi host: grok-pi --pi-bin /path/to/pi"
