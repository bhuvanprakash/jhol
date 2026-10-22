#!/bin/sh
# All shortcut validation lives in the Rust proof implementation. This wrapper
# delegates every invocation to the packaged CLI and never links shared trees.
BIN="$(dirname "$0")/jhol-bin"
if [ ! -x "$BIN" ]; then
  BIN="$(dirname "$0")/../target/release/jhol"
  [ -x "$BIN" ] || BIN="$(command -v jhol-bin 2>/dev/null)"
  [ -x "$BIN" ] || BIN="$(command -v jhol 2>/dev/null)"
fi
exec "$BIN" "$@"
