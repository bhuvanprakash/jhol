#!/usr/bin/env bash
# Independently runnable check for the blueprint example (Phase B/C gate).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ -n "${JHOL_BIN:-}" ]]; then
  BIN="$JHOL_BIN"
elif [[ -x "$ROOT/target/debug/jhol" ]]; then
  BIN="$ROOT/target/debug/jhol"
elif [[ -x "$ROOT/target/release/jhol" ]]; then
  BIN="$ROOT/target/release/jhol"
elif command -v jhol >/dev/null 2>&1; then
  BIN="$(command -v jhol)"
else
  echo "jhol binary not found; set JHOL_BIN or build --bin jhol" >&2
  exit 1
fi
EX="$ROOT/examples/blueprint"
out="$("$BIN" plan --path "$EX" --json)"
echo "$out" | grep -q '"kind"'
echo "$out" | grep -q 'javascript'
MIXED="$(mktemp -d)"
trap 'rm -rf "$MIXED"' EXIT
cp "$EX/package.json" "$EX/pyproject.toml" "$EX/Cargo.toml" "$MIXED/"
mkdir -p "$MIXED/native/demo-crate/src"
cp "$EX/native/demo-crate/Cargo.toml" "$MIXED/native/demo-crate/"
cp "$EX/native/demo-crate/src/lib.rs" "$MIXED/native/demo-crate/src/"
python3 - "$MIXED/package.json" "$MIXED/package-lock.json" <<'PY'
import json, pathlib, sys
package_path, lock_path = map(pathlib.Path, sys.argv[1:])
package = json.loads(package_path.read_text())
package["dependencies"] = {}
package_path.write_text(json.dumps(package, indent=2) + "\n")
lock = {
    "name": package["name"],
    "version": package["version"],
    "lockfileVersion": 3,
    "requires": True,
    "packages": {"": {"name": package["name"], "version": package["version"]}},
}
lock_path.write_text(json.dumps(lock, indent=2) + "\n")
PY
# Policy rejection must happen before Jhol, uv, or Cargo writes an environment.
if "$BIN" sync --path "$MIXED" --apply --offline >/dev/null 2>&1; then
  echo "verify: expected build-script policy rejection" >&2
  exit 1
fi
test ! -e "$MIXED/.venv"
test ! -e "$MIXED/target"
test ! -e "$MIXED/jhol.project.lock"
applied="$("$BIN" sync --path "$MIXED" --apply --offline --allow-build-scripts --json)"
echo "$applied" | grep -q 'applied and verified javascript adapter'
echo "$applied" | grep -q 'applied and verified python adapter'
echo "$applied" | grep -q 'applied and verified rust adapter'
test -f "$MIXED/package-lock.json"
test -f "$MIXED/uv.lock"
test -f "$MIXED/.venv/pyvenv.cfg"
test -f "$MIXED/Cargo.lock"
compgen -G "$MIXED/target/debug/deps/libblueprint_demo_crate-*.rlib" >/dev/null
# Adding/removing a native lock changes the CLI action-cache key and project lock.
before_cache_count="$(find "$MIXED/.jhol/action-cache" -type f | wc -l | tr -d ' ')"
printf 'example==1.0\n' > "$MIXED/requirements.txt"
with_added_lock="$("$BIN" sync --path "$MIXED" --offline --json)"
echo "$with_added_lock" | python3 -c 'import json,sys; data=json.load(sys.stdin); assert "requirements.txt" in [x["path"] for x in data["lock"]["native_locks"]]'
added_cache_count="$(find "$MIXED/.jhol/action-cache" -type f | wc -l | tr -d ' ')"
test "$added_cache_count" -gt "$before_cache_count"
rm "$MIXED/requirements.txt"
without_added_lock="$("$BIN" sync --path "$MIXED" --offline --json)"
echo "$without_added_lock" | python3 -c 'import json,sys; data=json.load(sys.stdin); assert "requirements.txt" not in [x["path"] for x in data["lock"]["native_locks"]]'
test "$(find "$MIXED/.jhol/action-cache" -type f | wc -l | tr -d ' ')" -eq "$added_cache_count"
"$BIN" sync --path "$EX" >/dev/null
test -f "$EX/jhol.project.lock"
grep -q '"format_version": 1' "$EX/jhol.project.lock"
grep -q 'package-lock.json' "$EX/jhol.project.lock"
sid="$("$BIN" snapshot create --path "$EX" --json | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')"
"$BIN" restore "$sid" --plan --path "$EX" >/dev/null
# Guarded restore must refuse DB auto-apply when requested
if "$BIN" restore "$sid" --apply --databases --path "$EX" >/dev/null 2>&1; then
  echo "verify: expected restore --databases to fail" >&2
  exit 1
fi
"$BIN" db plan --path "$EX" --json | grep -q 001_init
"$BIN" db probe --json | grep -q 'database_url_set'
"$BIN" ai verify --path "$EX" --manifest models/demo.jhol-model.json >/dev/null
"$BIN" ai prepare --path "$EX" --manifest models/demo.jhol-model.json >/dev/null
infer_a="$("$BIN" ai infer --path "$EX" --manifest models/linear-demo.jhol-model.json --config models/linear-demo/config.json --input=-2,1)"
infer_b="$("$BIN" ai infer --path "$EX" --manifest models/linear-demo.jhol-model.json --config models/linear-demo/config.json --input=-2,1)"
test "$infer_a" = "$infer_b"
grep -q ': large$' <<<"$infer_a"
"$BIN" dev --path "$EX" --json | grep -q '"boot_order"'
# Spawn stub listeners (python) then cleanup
"$BIN" dev --path "$EX" --spawn --hold-secs 1 >/dev/null
echo "verify: policy+offline-js-python-rust+lock-invalidation+plan+snapshot+db+ai-infer-repeatable+dev+spawn ok ($BIN)"
