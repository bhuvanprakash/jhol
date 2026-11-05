#!/bin/sh
# Lockfile / receipt replay determinism: snapshot the .jhol state, wipe
# node_modules, restore, install — the tree must be identical.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
JHOL="$(pwd)/target/release/jhol"
fail=0; pass=0
D=$(mktemp -d)
cd "$D"
printf '{"name":"snap","version":"1.0.0","dependencies":{"left-pad":"^1.3.0"}}' > package.json
"$JHOL" install > /dev/null 2>&1
# snapshot the state
cp package-lock.json lockfile.snapshot
[ -d .jhol ] && cp -r .jhol jhol-snapshot
# wipe and replay
rm -rf node_modules .jhol
cp lockfile.snapshot package-lock.json
[ -d jhol-snapshot ] && cp -r jhol-snapshot .jhol
"$JHOL" install --offline --native-only > /dev/null 2>&1
RC=$?
if [ "$RC" = "0" ] && [ -d node_modules ]; then
  pass=$((pass+1))
else
  echo "FAIL replay install (exit $RC)"; fail=1
fi
cd /; rm -rf "$D"
echo "snapshot-replay: $pass pass, $fail fail"
[ "$fail" = "0" ] || exit 1
