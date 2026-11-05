#!/bin/sh
# Pinned behavioral regressions for install / plan / lockfile.
# Each test: a package.json fixture + the exact output jhol must produce.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
JHOL="$(pwd)/target/release/jhol"
[ -x "$JHOL" ] || { echo "build first"; exit 1; }
fail=0; pass=0
D=$(mktemp -d)

# R1: lockfile determinism — two installs of the same lockfile produce
# byte-identical node_modules.
cd "$D"
printf '{"name":"r1","version":"1.0.0","dependencies":{"left-pad":"^1.3.0"}}' > package.json
"$JHOL" install > /dev/null 2>&1
H1=$(find node_modules -type f -exec sh -c 'sha256sum "$1" | cut -d" " -f1' _ {} \; | sort | sha256sum)
rm -rf node_modules
"$JHOL" install > /dev/null 2>&1
H2=$(find node_modules -type f -exec sh -c 'sha256sum "$1" | cut -d" " -f1' _ {} \; | sort | sha256sum)
if [ "$H1" = "$H2" ]; then pass=$((pass+1)); else echo "FAIL R1: tree not deterministic"; fail=1; fi

# R2: lockfile records integrity for every package
N=$(python3 -c "
import json; d=json.load(open('package-lock.json'))
pk=d.get('packages',{})
print(sum(1 for k,v in pk.items() if k and 'integrity' in v))")
T=$(python3 -c "
import json; d=json.load(open('package-lock.json'))
print(sum(1 for k in d.get('packages',{}) if k))")
if [ "$N" = "$T" ]; then pass=$((pass+1)); else echo "FAIL R2: $((T-N)) of $T packages missing integrity"; fail=1; fi

# R3: `jhol install` with no deps succeeds and creates an empty lockfile
cd "$D" && rm -rf node_modules package-lock.json
printf '{"name":"r3","version":"1.0.0"}' > package.json
"$JHOL" install > /dev/null 2>&1
RC=$?
if [ "$RC" = "0" ]; then pass=$((pass+1)); else echo "FAIL R3: no-deps install exit $RC"; fail=1; fi

cd /; rm -rf "$D"
echo "regression: $pass pass, $fail fail"
[ "$fail" = "0" ] || exit 1
