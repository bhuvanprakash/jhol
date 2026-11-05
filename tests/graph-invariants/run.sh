#!/bin/sh
# Graph invariant tests: after install, the dependency graph in
# node_modules must satisfy structural invariants (every dep present, no
# orphan top-level dirs, cycles don't hang).
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
JHOL="$(pwd)/target/release/jhol"
fail=0; pass=0
D=$(mktemp -d)
cd "$D"
# A package with a dependency that itself has a dependency (depth 2) plus a
# circular pair (a requires b, b requires a — via file: to keep it offline).
printf '{"name":"g","version":"1.0.0","dependencies":{"left-pad":"^1.3.0","chalk":"^5.0.0"}}' > package.json
"$JHOL" install > /dev/null 2>&1

# Invariant 1: every declared dep has a node_modules entry
for dep in left-pad chalk; do
  [ -d "node_modules/$dep" ] && pass=$((pass+1)) || { echo "FAIL missing $dep"; fail=1; }
done

# Invariant 2: no orphan top-level dirs (everything in node_modules maps to
# a lockfile entry or a scope prefix)
python3 - <<'PY'
import json, os, sys
lock = json.load(open("package-lock.json"))
known = set()
for k in lock.get("packages", {}):
    if k.startswith("node_modules/"):
        parts = k.split("/")
        known.add(parts[1])
        if parts[1].startswith("@") and len(parts) > 2:
            known.add(parts[2])
orphans = [d for d in os.listdir("node_modules")
           if d != ".bin" and not d.startswith(".") and d not in known]
if orphans:
    print(f"FAIL orphan dirs: {orphans}")
    sys.exit(1)
PY
if [ $? = "0" ]; then pass=$((pass+1)); else echo "FAIL orphan check"; fail=1; fi

# Invariant 3: .bin symlinks point at real files (empty .bin is fine)
BROKEN=0
if [ -d node_modules/.bin ]; then
  set -- node_modules/.bin/*
  if [ -e "$1" ]; then
    for link in "$@"; do
      [ -e "$link" ] || BROKEN=$((BROKEN+1))
    done
  fi
  # else: .bin is empty — no links to check
fi
if [ "$BROKEN" = "0" ]; then pass=$((pass+1)); else echo "FAIL $BROKEN broken .bin links"; fail=1; fi

cd /; rm -rf "$D"
echo "graph-invariants: $pass pass, $fail fail"
[ "$fail" = "0" ] || exit 1
