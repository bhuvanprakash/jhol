#!/bin/sh
# jhol-core (installer) quality gate.
#
# The engine has crates/jhol-vm/verify.sh; this is the same discipline for the
# package manager: unit tests, the adversarial tarball corpus, and a
# real-world end-to-end offline install with tree determinism.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
fail=0
note() { printf '%s\n' "$*"; }

note "== build =="
cargo build -q --release -p jhol-core 2>&1 | grep -E '^error' && exit 1
cargo build -q --release --features jholvm 2>&1 | grep -E '^error' && exit 1

note "== unit tests =="
if cargo test -q -p jhol-core --lib 2>&1 | tail -3 | grep -q 'FAILED\|error'; then
  note "  FAIL unit tests"; fail=1
else
  note "  ok   unit tests"
fi

note "== adversarial install corpus =="
if cargo test -q -p jhol-core --lib adversarial 2>&1 | tail -3 | grep -q 'FAILED\|error'; then
  note "  FAIL adversarial"; fail=1
else
  note "  ok   adversarial tarballs/manifests/receipts"
fi

note "== end-to-end: offline install + determinism =="
E2E=$(mktemp -d)
cp -r bench_cold/package.json bench_cold/package-lock.json "$E2E/" 2>/dev/null
WORK="$PWD"
cd "$E2E" || exit 1
# First install (offline, from the content store) — time it.
T0=$(python3 -c 'import time; print(time.time())')
"$WORK/target/release/jhol" install --offline --native-only >install1.log 2>&1
RC1=$?
T1=$(python3 -c 'import time; print(time.time())')
DUR1=$(python3 -c "print(f'{$T1-$T0:.2f}')")
if [ "$RC1" != "0" ]; then
  note "  FAIL offline install (exit $RC1)"; head -10 install1.log; fail=1
else
  note "  ok   offline install (${DUR1}s, $(find node_modules -type f | wc -l | tr -d ' ') files)"
  # Determinism: second install into a fresh tree must be byte-identical.
  rm -rf node_modules
  "$WORK/target/release/jhol" install --offline --native-only >install2.log 2>&1
  H1=$(find node_modules -type f -exec sha256sum {} \; | sed 's|node_modules/||' | sort | sha256sum)
  rm -rf node_modules
  "$WORK/target/release/jhol" install --offline --native-only >install3.log 2>&1
  H2=$(find node_modules -type f -exec sha256sum {} \; | sed 's|node_modules/||' | sort | sha256sum)
  if [ "$H1" = "$H2" ]; then
    note "  ok   deterministic tree (byte-identical across installs)"
  else
    note "  FAIL tree differs between installs"; fail=1
  fi

  # Cache GC safety: sweeping with this lockfile must keep everything the
  # lockfile references, so an offline install still succeeds afterwards.
  # (A first version of cache gc deleted every cached tarball while reporting
  # "0 reachable" — a double-prefixed SRI comparison. The unit tests pin the
  # comparison; this pins the end-to-end consequence.)
  BEFORE=$(du -sk "$HOME/.jhol-cache" 2>/dev/null | cut -f1)
  "$WORK/target/release/jhol" cache gc --from package-lock.json >gc.log 2>&1
  rm -rf node_modules
  "$WORK/target/release/jhol" install --offline --native-only >gc_install.log 2>&1
  RCG=$?
  NGFILES=$(find node_modules -type f 2>/dev/null | wc -l | tr -d ' ')
  if [ "$RCG" = "0" ] && [ "$NGFILES" -gt 100 ]; then
    note "  ok   cache gc keeps referenced entries (offline install after gc: $NGFILES files)"
  else
    note "  FAIL offline install broke after cache gc (exit $RCG, $NGFILES files)"
    head -5 gc_install.log
    fail=1
  fi
  AFTER=$(du -sk "$HOME/.jhol-cache" 2>/dev/null | cut -f1)
  note "  info cache $BEFORE KB -> $AFTER KB after gc"
fi
cd "$WORK"
rm -rf "$E2E"

note ""
if [ "$fail" = "0" ]; then note "INSTALLER GATE: PASS"; else note "INSTALLER GATE: FAIL"; fi
exit $fail
