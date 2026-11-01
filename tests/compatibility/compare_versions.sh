#!/usr/bin/env bash
# Correctness harness: same package.json → npm, Bun (if present), and jhol
# must agree on top-level resolved versions for declared dependencies.
# Blueprint Phase A — evidence artifact, not a perf comparison.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIX="${TMPDIR:-/tmp}/jhol-compat-$$"
BIN="${JHOL_BIN:-$ROOT/target/debug/jhol}"
# Always absolute so subshells after `cd` still find the binary.
if [[ "$BIN" != /* ]]; then
  BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
fi
mkdir -p "$FIX"
trap 'rm -rf "$FIX"' EXIT

cat >"$FIX/package.json" <<'EOF'
{
  "name": "jhol-compat-fixture",
  "private": true,
  "dependencies": {
    "ms": "2.1.3",
    "is-number": "7.0.0"
  }
}
EOF

need() { command -v "$1" >/dev/null 2>&1 || { echo "skip: $1 not installed"; return 1; }; }

versions_from_nm() {
  local dir="$1"
  python3 - <<PY
import json, pathlib
root = pathlib.Path("$dir") / "node_modules"
for name in ("ms", "is-number"):
    p = root / name / "package.json"
    if not p.exists():
        print(f"{name}=MISSING")
        continue
    print(f"{name}=" + json.loads(p.read_text())["version"])
PY
}

run_tool() {
  local tool="$1"
  local work="$FIX/$tool"
  mkdir -p "$work"
  cp "$FIX/package.json" "$work/"
  (
    cd "$work"
    case "$tool" in
      npm)
        npm install --ignore-scripts --no-audit --no-fund >/dev/null 2>&1
        ;;
      bun)
        bun install >/dev/null 2>&1
        ;;
      jhol)
        export JHOL_CACHE_DIR="$FIX/jhol-cache"
        export JHOL_NO_DAEMON=1 JHOL_AUTO_DAEMON=0
        mkdir -p "$JHOL_CACHE_DIR"
        "$BIN" install --no-scripts >/dev/null
        ;;
    esac
  )
  versions_from_nm "$work"
}

echo "== jhol ($BIN) =="
if [[ ! -x "$BIN" ]]; then
  (cd "$ROOT" && cargo build -q --bin jhol)
fi
JHOL_OUT="$(run_tool jhol)"
echo "$JHOL_OUT"

EXPECTED_MS="ms=2.1.3"
EXPECTED_IN="is-number=7.0.0"
echo "$JHOL_OUT" | grep -q "$EXPECTED_MS" || { echo "FAIL: jhol ms"; exit 1; }
echo "$JHOL_OUT" | grep -q "$EXPECTED_IN" || { echo "FAIL: jhol is-number"; exit 1; }

if need npm; then
  echo "== npm =="
  NPM_OUT="$(run_tool npm)"
  echo "$NPM_OUT"
  echo "$NPM_OUT" | grep -q "$EXPECTED_MS" || { echo "FAIL: npm ms mismatch"; exit 1; }
  echo "$NPM_OUT" | grep -q "$EXPECTED_IN" || { echo "FAIL: npm is-number mismatch"; exit 1; }
fi

if need bun; then
  echo "== bun =="
  BUN_OUT="$(run_tool bun)"
  echo "$BUN_OUT"
  echo "$BUN_OUT" | grep -q "$EXPECTED_MS" || { echo "FAIL: bun ms mismatch"; exit 1; }
  echo "$BUN_OUT" | grep -q "$EXPECTED_IN" || { echo "FAIL: bun is-number mismatch"; exit 1; }
fi

echo "PASS: pinned versions agree across available tools"
