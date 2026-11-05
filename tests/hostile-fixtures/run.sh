#!/bin/sh
# Hostile-fixture E2E gate: a local registry serves tarballs crafted to
# escape the extraction sandbox. The real jhol binary must refuse every
# attack. Complements the in-crate adversarial_install.rs unit tests.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
JHOL="$(pwd)/target/release/jhol"
[ -x "$JHOL" ] || { echo "build first: cargo build --release --features jholvm"; exit 1; }
PORT=4891
fail=0; pass=0

python3 tests/hostile-fixtures/fixtures/serve_hostile.py "$PORT" &
SRV=$!
sleep 1
trap 'kill $SRV 2>/dev/null' EXIT

for scenario in traverse absolute backslash symlink_escape; do
  D=$(mktemp -d)
  cd "$D"
  printf '{"name":"victim","version":"1.0.0"}' > package.json
  JHOL_REGISTRY="http://127.0.0.1:$PORT" \
    "$JHOL" install "hostile-$scenario" --native-only --no-scripts \
    >install.log 2>&1
  RC=$?
  ATTACKED=false
  case "$scenario" in
    traverse)      [ -f "$D/escaped.txt" ] && ATTACKED=true ;;
    absolute)      [ -f /tmp/jhol-hostile-absolute.txt ] && ATTACKED=true ;;
    backslash)     [ -f "$D/evil.js" ] && ATTACKED=true ;;
    symlink_escape)[ -e "$D/outside" ] || [ -L "$D/proj/escape" ] && ATTACKED=true ;;
  esac
  if [ "$ATTACKED" = "true" ]; then
    echo "FAIL $scenario: attack succeeded (install exit $RC)"; fail=1
  else
    echo "ok   $scenario: attack failed closed (install exit $RC)"
    pass=$((pass+1))
  fi
  cd /
  rm -rf "$D"
done

# Control: a clean tarball must install successfully
D=$(mktemp -d); cd "$D"
printf '{"name":"victim","version":"1.0.0"}' > package.json
JHOL_REGISTRY="http://127.0.0.1:$PORT" \
  "$JHOL" install "hostile-clean" --native-only --no-scripts > install.log 2>&1
RC=$?
FILES=$(find node_modules -type f 2>/dev/null | wc -l | tr -d ' ')
if [ "$RC" = "0" ] && [ "$FILES" -gt 0 ]; then
  echo "ok   clean (control): install succeeded ($FILES files)"
  pass=$((pass+1))
else
  echo "FAIL clean: install broke (exit $RC, $FILES files)"; fail=1
fi
cd /; rm -rf "$D"

echo "hostile fixtures: $pass pass, $fail fail"
[ "$fail" = "0" ] || exit 1
