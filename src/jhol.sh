#!/bin/sh
# jhol — zero-startup trampoline.
# Checks daemon status file. If nm_fresh=1, node_modules is correct.
# Exits 0 in ~0.1ms. Only falls through to Rust binary when work needed.
STATUS="${JHOL_STATUS_FILE:-/tmp/jhol-status.bin}"
case "${1:-}" in
install|add|ci)
  if [ -f "$STATUS" ]; then
    FRESH=$(dd if="$STATUS" bs=1 skip=8 count=1 2>/dev/null | od -A n -t u1 | tr -d ' \n')
    [ "$FRESH" = "1" ] && exit 0
  fi ;;
esac
exec "$(dirname "$0")/jhol-bin" "$@"
