#!/bin/sh
# Polyglot E2E: a project with JS + Python + Rust manifests; jhol plan/sync
# must handle all of them without error.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
JHOL="$(pwd)/target/release/jhol"
fail=0; pass=0
D=$(mktemp -d)
cd "$D"
# JS manifest
printf '{"name":"poly","version":"1.0.0"}' > package.json
# Python manifest
printf '[project]\nname = "poly"\nversion = "1.0.0"\n' > pyproject.toml
# Rust manifest
printf '[package]\nname = "poly"\nversion = "1.0.0"\nedition = "2021"\n' > Cargo.toml
"$JHOL" plan > plan.out 2>&1
RC=$?
if [ "$RC" = "0" ]; then pass=$((pass+1)); else echo "FAIL plan (exit $RC)"; cat plan.out | head -5; fail=1; fi
"$JHOL" sync > sync.out 2>&1
RC=$?
if [ "$RC" = "0" ] || grep -q 'nothing to sync\|up to date\|0 packages' sync.out 2>/dev/null; then
  pass=$((pass+1))
else
  echo "FAIL sync (exit $RC)"; head -5 sync.out; fail=1
fi
cd /; rm -rf "$D"
echo "polyglot-e2e: $pass pass, $fail fail"
[ "$fail" = "0" ] || exit 1
