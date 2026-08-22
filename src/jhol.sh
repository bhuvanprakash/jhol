#!/bin/sh
# jhol — zero-startup trampoline.
# Checks prebuilt node_modules cache. If exists, symlink and exit 0.
# Falls through to daemon status check, then Rust binary.
PREBUILT_DIR="${JHOL_PREBUILT_DIR:-/tmp/jhol-prebuilt}"
case "${1:-}" in
install|add|ci)
  # Prebuilt cache: hash package.json, check if prebuilt node_modules exists.
  if [ -f package.json ]; then
    HASH=$(shasum -a 256 package.json 2>/dev/null | cut -d' ' -f1)
    if [ -n "$HASH" ] && [ -d "$PREBUILT_DIR/$HASH/node_modules" ]; then
      rm -rf node_modules
      ln -sfn "$PREBUILT_DIR/$HASH" "$PWD/node_modules"
      exit 0
    fi
  fi
  # Status file: if nm_fresh=1, node_modules is correct.
  STATUS="${JHOL_STATUS_FILE:-/tmp/jhol-status.bin}"
  if [ -f "$STATUS" ]; then
    FRESH=$(dd if="$STATUS" bs=1 skip=8 count=1 2>/dev/null | od -A n -t u1 | tr -d ' \n')
    [ "$FRESH" = "1" ] && exit 0
  fi ;;
esac
exec "$(dirname "$0")/jhol-bin" "$@"
