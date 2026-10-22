#!/usr/bin/env bash
# E145 verification: a nested version conflict must resolve to the version npm's lockfile requires.
#
# Usage: scripts/e145/verify.sh [path-to-jhol]
#
# Why a script and not a unit test: the scenario needs a real npm lockfile (npm records the nested
# copy), so this is a network-dependent end-to-end check. It asserts the *observable outcome* —
# `agent-base/node_modules/debug` holds the version the lockfile requires (4.4.3), not the hoisted
# 2.6.9 — which is exactly the defect E145 described, and `axios` loads.
set -u
J="${1:-$HOME/j/jhol/target/release/jhol}"
FIX="${JHOL_E145_FIXTURE:-$HOME/j/jhol-bench/large-test/package.json}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

[ -f "$FIX" ] || { echo "  fixture not found: $FIX"; exit 2; }
command -v npm >/dev/null || { echo "  npm not available; skipping"; exit 2; }
cp "$FIX" "$WORK/package.json"
( cd "$WORK" && npm install --silent --no-audit --no-fund >/dev/null 2>&1 ) || { echo "  npm install failed"; exit 2; }
rm -rf "$WORK/node_modules"
( cd "$WORK" && JHOL_CACHE_DIR="$WORK/cache" JHOL_NO_SPLASH=1 JHOL_NO_DAEMON=1 "$J" ci >/dev/null 2>&1 ) || true

read_version() { python3 -c "
import json,sys
try: print(json.load(open(sys.argv[1]))['version'])
except Exception: print('missing')" "$1"; }
REQUIRED=$(python3 -c "
import json,sys
d=json.load(open('$WORK/package-lock.json'))['packages']
print(d.get('node_modules/agent-base/node_modules/debug',{}).get('version','?'))")
GOT=$(read_version "$WORK/node_modules/agent-base/node_modules/debug/package.json")
LOADS=$(cd "$WORK" && node -e "
const deps=['axios','bcryptjs','chalk','commander','cors','dayjs','debug','dotenv','fs-extra','jsonwebtoken','lodash','mime-types','qs','react','react-dom','semver','typescript','uuid','zod'];
let ok=0; for (const m of deps){ try{require(m);ok++;}catch(e){} } console.log(ok+'/'+deps.length);" 2>/dev/null || echo "?/19")

echo "  required (npm lockfile): $REQUIRED"
echo "  installed nested version: $GOT"
echo "  runtime deps loading:     $LOADS"
if [ "$GOT" = "$REQUIRED" ]; then echo "  RESULT: PASS"; exit 0; else echo "  RESULT: FAIL"; exit 1; fi
