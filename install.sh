#!/bin/sh
# jhol installer — https://github.com/bhuvanprakash/jhol
#
#   curl -fsSL https://raw.githubusercontent.com/bhuvanprakash/jhol/main/install.sh | sh
#
# Flags (also honoured as environment variables):
#   --dir <path>      install directory            (JHOL_INSTALL_DIR, default ~/.local/bin)
#   --version <ver>   release to install           (JHOL_VERSION, default: latest)
#   --source          build from source with cargo (JHOL_INSTALL_FROM_SOURCE=1)
#   --no-color        plain output                 (NO_COLOR / not a tty)
#
# Design notes: no emoji — box drawing, weight and colour carry the layout. The
# download renders its own progress bar so the first thing a new user sees is
# legible rather than a wall of curl output.
set -eu

REPO="bhuvanprakash/jhol"
BIN_NAME="jhol"

# ── presentation ────────────────────────────────────────────────────────────
# Purple accent stays 38;5;141. No emoji. Dim + weight — not a rainbow.
# Progress bar in download() is intentionally untouched.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != "dumb" ]; then
  C_RESET=$(printf '\033[0m');  C_DIM=$(printf '\033[2m')
  C_BOLD=$(printf '\033[1m');   C_ACC=$(printf '\033[38;5;141m')
  C_ERR=$(printf '\033[38;5;203m')
else
  C_RESET=''; C_DIM=''; C_BOLD=''; C_ACC=''; C_ERR=''
fi
WIDTH=52

say()  { printf '%s\n' "$*"; }
info() { printf '  %s%s%s\n' "$C_DIM" "$*" "$C_RESET"; }
row()  { printf '  %s%-10s%s %s\n' "$C_DIM" "$1" "$C_RESET" "$2"; }
step() { printf '  %s·%s  %s\n' "$C_ACC" "$C_RESET" "$1"; }
ok()   { printf '  %s·%s  %s\n' "$C_ACC" "$C_RESET" "$1"; }
warn() { printf '  %s!%s  %s\n' "$C_DIM" "$C_RESET" "$1" >&2; }
die()  { printf '  %serror%s  %s\n' "$C_ERR" "$C_RESET" "$1" >&2; exit 1; }

hrule() {
  i=0; line=''
  while [ "$i" -lt "$WIDTH" ]; do line="${line}─"; i=$((i + 1)); done
  printf '%s' "$line"
}

rule() {
  printf '  %s%s%s\n' "$C_DIM" "$(hrule)" "$C_RESET"
}

# Open header: wordmark first, one quiet tagline, thin rule. No heavy box.
header() {
  printf '\n'
  printf '  %s%sjhol%s\n' "$C_BOLD" "$C_ACC" "$C_RESET"
  printf '  %scache-first · offline · npm-compatible%s\n' "$C_DIM" "$C_RESET"
  printf '\n'
  rule
  printf '\n'
}

# ── arguments ───────────────────────────────────────────────────────────────
INSTALL_DIR="${JHOL_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${JHOL_VERSION:-latest}"
FROM_SOURCE="${JHOL_INSTALL_FROM_SOURCE:-0}"

while [ $# -gt 0 ]; do
  case "$1" in
    --dir)     INSTALL_DIR="${2:?--dir needs a path}"; shift 2 ;;
    --dir=*)   INSTALL_DIR="${1#*=}"; shift ;;
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    --version=*) VERSION="${1#*=}"; shift ;;
    --source)  FROM_SOURCE=1; shift ;;
    --no-color) C_RESET=''; C_DIM=''; C_BOLD=''; C_ACC=''; C_ERR=''; shift ;;
    -h|--help)
      say "jhol installer"
      say "  --dir <path>       install directory (default ~/.local/bin)"
      say "  --version <ver>    release to install (default: latest)"
      say "  --source           build from source with cargo"
      say "  --no-color         plain output"
      exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

# ── platform ────────────────────────────────────────────────────────────────
detect_platform() {
  os=$(uname -s 2>/dev/null || echo unknown)
  arch=$(uname -m 2>/dev/null || echo unknown)
  case "$os" in
    Darwin) OS=macos ;;
    Linux)  OS=linux ;;
    MINGW*|MSYS*|CYGWIN*) OS=windows ;;  # Windows users run curl | sh in Git Bash
    *) die "unsupported operating system: $os (jhol ships macOS, Linux and Windows builds)" ;;
  esac
  case "$arch" in
    arm64|aarch64) ARCH=arm64 ;;
    x86_64|amd64)  ARCH=x64 ;;
    *) die "unsupported architecture: $arch" ;;
  esac
  # Per-arch asset when the release ships one, falling back to the historical
  # per-OS name so older releases keep working. Windows gets the binary through
  # this same curl flow - never a GUI double-click.
  ASSET="jhol-$OS-$ARCH"
  ASSET_FALLBACK="jhol-$OS"
  [ "$OS" = "windows" ] && ASSET="$ASSET.exe" && ASSET_FALLBACK="$ASSET_FALLBACK.exe"
  # what the binary is called once installed
  case "$OS" in windows) BIN_NAME="jhol.exe" ;; esac
}

have() { command -v "$1" >/dev/null 2>&1; }

# ── download with a rendered progress bar ───────────────────────────────────
# curl's own progress is noisy; this polls the growing file and draws one line that
# is rewritten in place. Falls back to a plain message when not a terminal.
download() {
  url=$1; dest=$2
  if [ ! -t 1 ]; then
    curl -fsSL "$url" -o "$dest" 2>/dev/null || return 1
    return 0
  fi

  total=$(curl -fsSLI "$url" 2>/dev/null | tr -d '\r' | awk 'tolower($1)=="content-length:"{print $2}' | tail -1)
  curl -fsSL "$url" -o "$dest" 2>/dev/null &
  pid=$!
  spin='|/-\'
  i=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ -n "${total:-}" ] && [ "$total" -gt 0 ] 2>/dev/null; then
      have_bytes=$(wc -c < "$dest" 2>/dev/null || echo 0)
      pct=$((have_bytes * 100 / total))
      [ "$pct" -gt 100 ] && pct=100
      filled=$((pct * 34 / 100))
      bar=""
      j=0
      while [ "$j" -lt 34 ]; do
        if [ "$j" -lt "$filled" ]; then bar="$bar█"; else bar="$bar░"; fi
        j=$((j + 1))
      done
      mb_have=$(awk -v b="$have_bytes" 'BEGIN{printf "%.1f", b/1048576}')
      mb_tot=$(awk -v b="$total" 'BEGIN{printf "%.1f", b/1048576}')
      printf '\r  %s%s%s  %s%3d%%%s  %s%s / %s MB%s ' \
        "$C_ACC" "$bar" "$C_RESET" "$C_DIM" "$pct" "$C_RESET" "$C_DIM" "$mb_have" "$mb_tot" "$C_RESET"
    else
      i=$(( (i + 1) % 4 ))
      printf '\r  %s%s%s  %sdownloading%s ' "$C_ACC" "$(printf '%s' "$spin" | cut -c$((i+1)))" "$C_RESET" "$C_DIM" "$C_RESET"
    fi
    sleep 0.15
  done
  printf '\r%*s\r' 78 ''
  wait "$pid" || return 1
  return 0
}

# ── install from a release asset ────────────────────────────────────────────
try_release() {
  if [ "$VERSION" = "latest" ]; then
    url="https://github.com/$REPO/releases/latest/download/$ASSET"
  else
    url="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
  fi
  tmp=$(mktemp "${TMPDIR:-/tmp}/jhol.XXXXXX") || die "cannot create a temporary file"
  trap 'rm -f "$tmp"' EXIT INT TERM
  row "asset" "$ASSET"
  if ! download "$url" "$tmp"; then
    if [ -n "${ASSET_FALLBACK:-}" ]; then
      # Historical single-asset-per-OS release: keep installing from it.
      ASSET="$ASSET_FALLBACK"
      url="https://github.com/$REPO/releases/latest/download/$ASSET"
      [ -n "${VERSION:-}" ] && [ "$VERSION" != "latest" ] && \
        url="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
      row "asset" "$ASSET  (older release layout)"
      download "$url" "$tmp" || { rm -f "$tmp"; return 1; }
    else
      rm -f "$tmp"
      return 1
    fi
  fi
  size=$(wc -c < "$tmp" | tr -d ' ')
  [ "$size" -gt 100000 ] || { warn "download looks wrong (${size} bytes)"; rm -f "$tmp"; return 1; }
  ok "downloaded $(awk -v b="$size" 'BEGIN{printf "%.1f MB", b/1048576}')"
  install_binary "$tmp"
  return 0
}

# ── install from source (fallback, and --source) ────────────────────────────
from_source() {
  have cargo || die "no release is published yet and cargo is not installed — install Rust from https://rustup.rs and re-run"
  row "build" "from source with cargo (a few minutes)"
  if [ -f "Cargo.toml" ] && grep -q 'name = "jhol"' Cargo.toml 2>/dev/null; then
    info "using the working tree in $(pwd)"
    cargo install --path . --root "${JHOL_CARGO_ROOT:-$HOME/.jhol}" --quiet \
      || die "cargo install failed"
    src="${JHOL_CARGO_ROOT:-$HOME/.jhol}/bin/$BIN_NAME"
  else
    cargo install --git "https://github.com/$REPO.git" --root "${JHOL_CARGO_ROOT:-$HOME/.jhol}" --quiet \
      || die "cargo install failed"
    src="${JHOL_CARGO_ROOT:-$HOME/.jhol}/bin/$BIN_NAME"
  fi
  [ -x "$src" ] || die "cargo reported success but $src is missing"
  install_binary "$src"
}

install_binary() {
  src=$1
  mkdir -p "$INSTALL_DIR" || die "cannot create $INSTALL_DIR"
  cp "$src" "$INSTALL_DIR/$BIN_NAME.tmp.$$" || die "cannot write to $INSTALL_DIR"
  chmod 755 "$INSTALL_DIR/$BIN_NAME.tmp.$$"
  mv "$INSTALL_DIR/$BIN_NAME.tmp.$$" "$INSTALL_DIR/$BIN_NAME"
  ok "installed  $INSTALL_DIR/$BIN_NAME"
}

path_hint() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac
  printf '\n'
  warn "not on PATH — add this to your shell profile:"
  printf '    %sexport PATH="%s:$PATH"%s\n' "$C_DIM" "$INSTALL_DIR" "$C_RESET"
}

finish() {
  version=$("$INSTALL_DIR/$BIN_NAME" --version 2>/dev/null || echo "jhol")
  printf '\n'
  rule
  printf '  %sready%s   %s%s%s\n' "$C_BOLD" "$C_RESET" "$C_ACC" "$version" "$C_RESET"
  printf '\n'
  printf '  %snext%s\n' "$C_DIM" "$C_RESET"
  row "init"    "jhol init"
  row "install" "jhol install"
  row "gui"     "jhol gui"
  row "docs"    "https://github.com/$REPO"
  printf '\n'
}

# ── run ─────────────────────────────────────────────────────────────────────
header
detect_platform
row "platform" "$OS-$ARCH"
row "dest"     "$INSTALL_DIR"
row "version"  "$VERSION"
printf '\n'

if [ "$FROM_SOURCE" = "1" ]; then
  from_source
elif try_release; then
  :
else
  warn "no prebuilt release for $OS/$ARCH — building from source"
  printf '\n'
  from_source
fi

path_hint
finish
