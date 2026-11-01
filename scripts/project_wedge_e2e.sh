#!/bin/bash
# End-to-end check of the project wedge: plan, sync, snapshot, diff, restore, dev.
# usage: scripts/project_wedge_e2e.sh [path/to/jhol]   (default: target/release/jhol)
# Works in a temp HOME and temp project; touches nothing else. Exits non-zero on the first failure.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
J="${1:-$ROOT/target/release/jhol}"
[ -x "$J" ] || { echo "no executable jhol at $J"; exit 2; }
TMP="$(mktemp -d "${TMPDIR:-/tmp}/jhol-wedge.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
export HOME="$TMP/home" JHOL_CACHE_DIR="$TMP/home/c" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 CI=1
W="$TMP/app"; mkdir -p "$HOME" "$W"; cd "$W" || exit 1
fail(){ echo "FAIL: $*"; exit 1; }
step(){ echo; echo "== $*"; }

cat > package.json <<'JSON'
{"name":"wedge-e2e","version":"1.0.0","dependencies":{"react":"18.2.0","react-dom":"18.2.0"}}
JSON
cp package.json "$TMP/package.orig.json"

step "plan is read-only"
"$J" plan >"$TMP/plan.txt" 2>&1 || fail "plan"
grep -q "no side effects" "$TMP/plan.txt" || fail "plan did not state it is read-only"
[ -e node_modules ] && fail "plan created node_modules"

step "sync --apply installs and saves its receipt"
"$J" sync --apply >"$TMP/sync.txt" 2>&1 || { cat "$TMP/sync.txt"; fail "sync --apply"; }
ls .jhol/receipts | grep -qv latest || fail "sync saved no receipt"

step "snapshot captures manifest, lock and the sync receipt"
"$J" snapshot create >"$TMP/snap.txt" 2>&1 || fail "snapshot create"
SID=$(sed -n 's/^created snapshot \([^ ]*\) .*/\1/p' "$TMP/snap.txt"); [ -n "$SID" ] || fail "no snapshot id"
"$J" snapshot show "$SID" --json >"$TMP/show.json" || fail "snapshot show"
python3 - "$TMP/show.json" <<'PY' || fail "snapshot content"
import json,sys
s=json.load(open(sys.argv[1]))
assert s.get('manifest') and 'react' in s['manifest'], 'manifest missing'
assert s.get('package_lock') and '"packages"' in s['package_lock'], 'package-lock missing'
assert s['receipt']['command']=='sync', 'receipt is not the sync receipt'
PY

step "diff right after snapshot: no changes"
"$J" diff >"$TMP/d0.txt" 2>&1 || fail "diff"
grep -q "nothing to sync" "$TMP/d0.txt" || fail "expected no-change message"
"$J" diff --exit-code >/dev/null 2>&1 || fail "--exit-code must be 0 with no changes"

step "edit react and sync: diff shows the package change"
sed -i.bak 's/"react":"18.2.0"/"react":"18.3.1"/' package.json && rm -f package.json.bak
"$J" sync --apply >"$TMP/sync2.txt" 2>&1 || { cat "$TMP/sync2.txt"; fail "second sync"; }
"$J" diff >"$TMP/d1.txt" 2>&1
grep -q "node_modules/react  18.2.0 -> 18.3.1" "$TMP/d1.txt" || fail "react change not reported"
"$J" diff --exit-code >/dev/null 2>&1; [ $? -eq 1 ] || fail "--exit-code must be 1 when sync is needed"

step "restore refuses while package.json differs"
"$J" restore "$SID" --plan >"$TMP/rplan.txt" 2>&1
grep -q "blocked: package.json differs" "$TMP/rplan.txt" || fail "restore plan must show the block"
"$J" restore "$SID" --apply >/dev/null 2>&1 && fail "restore --apply must refuse on a changed manifest"

step "restore after reverting package.json restores the lock, not node_modules"
cp "$TMP/package.orig.json" package.json
"$J" restore "$SID" --apply >"$TMP/rapply.txt" 2>&1 || { cat "$TMP/rapply.txt"; fail "restore --apply"; }
python3 -c "
import json,sys
v=json.load(open('package-lock.json'))['packages']['node_modules/react']['version']
sys.exit(0 if v=='18.2.0' else 1)" || fail "restored lock does not hold react 18.2.0"
"$J" diff >"$TMP/d2.txt" 2>&1
grep -q "installed 18.3.1, lock 18.2.0" "$TMP/d2.txt" || fail "installed tree drift not reported after restore"
grep -q "next: jhol sync --apply" "$TMP/d2.txt" || fail "diff must ask for sync after restore"

step "sync --apply materialises the restored tree"
"$J" sync --apply >"$TMP/sync3.txt" 2>&1 || { cat "$TMP/sync3.txt"; fail "sync after restore"; }
"$J" diff >"$TMP/d3.txt" 2>&1
grep -q "nothing to sync" "$TMP/d3.txt" || fail "nothing should be pending after sync"
node -p "require('./node_modules/react/package.json').version" 2>/dev/null | grep -q "^18.2.0$" \
  || fail "react 18.2.0 not installed after restore + sync"

step "dev: boot order from jhol.toml, and a helpful message without one"
"$J" dev >"$TMP/dev-none.txt" 2>&1; [ $? -ne 0 ] || fail "dev without jhol.toml should fail"
grep -q "Declare services" "$TMP/dev-none.txt" || fail "dev should show an example config"
cat > jhol.toml <<'TOML'
schema = 1

[[service]]
name = "db"
kind = "process"
run = ["node", "-e", "setInterval(()=>{},1000)"]
ready_tcp = "127.0.0.1:1"

[[service]]
name = "api"
kind = "process"
needs = ["db"]
run = ["node", "-e", "setInterval(()=>{},1000)"]
TOML
"$J" dev >"$TMP/dev.txt" 2>&1 || { cat "$TMP/dev.txt"; fail "dev order"; }
grep -q "boot: db → api" "$TMP/dev.txt" || fail "boot order should start db before api"

echo; echo "PROJECT WEDGE E2E PASS"
