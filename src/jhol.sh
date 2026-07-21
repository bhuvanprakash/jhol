#!/bin/sh
# jhol — zero-startup shell trampoline.
# Checks daemon status file BEFORE loading the Rust binary.
# If node_modules is fresh, exits 0 immediately in ~0.1ms.
STATUS="${JHOL_STATUS_FILE:-/tmp/jhol-status.bin}"
if [ -f "$STATUS" ] && [ "${1:-}" = "install" ] || [ "${1:-}" = "add" ] || [ "${1:-}" = "ci" ]; then
  NM_FRESH=$(dd if="$STATUS" bs=1 skip=8 count=1 2>/dev/null | od -A n -t x1 | tr -d ' \n')
  if [ "$NM_FRESH" = "01" ]; then
    HASH=$(dd if="$STATUS" bs=1 skip=9 count=8 2>/dev/null | od -A n -t x1 | tr -d ' \n')
    FP="${PWD}/node_modules/.nm_fingerprint"
    if [ -f "$FP" ]; then
      STORED=$(od -A n -t x1 "$FP" 2>/dev/null | head -1 | tr -d ' \n' | cut -c1-16)
      if [ "$STORED" = "$HASH" ]; then
        exit 0
      fi
    fi
  fi
fi
exec "$(dirname "$0")/jhol" "$@"
