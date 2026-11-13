#!/bin/sh
# JholVM quality gate.
#
# Every claim about this engine rests on these checks. Run before any release;
# they were added after a parser bug survived a green 39-file corpus and only
# showed up against real packages in node_modules.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)" || exit 1
BIN=target/release/examples
CLI=target/release/jhol
fail=0
note() { printf '%s\n' "$*"; }
run() { # run <label> <expected> <actual>
  if [ "$2" = "$3" ]; then note "ok   $1 ($2)"; else note "FAIL $1 (expected $2, got $3)"; fail=1; fi
}

note "== build =="
cargo build -q --release --features jholvm || exit 1
cargo build -q -p jhol-vm --release --examples || exit 1

note "== unit tests =="
cargo test -q -p jhol-vm --lib -- --test-threads=1 >/dev/null 2>&1 && note "ok   unit tests" || { note "FAIL unit tests"; fail=1; }

note "== corpus vs node =="
pass=0; total=0
for f in crates/jhol-vm/tests/js/*.js; do
  b=$(basename "$f" .js); exp="crates/jhol-vm/tests/js/$b.expected"
  [ -f "$exp" ] || continue
  total=$((total+1))
  got=$(JHOL_RUNTIME_ENGINE=jholvm "$CLI" run "$f" 2>&1)
  [ "$got" = "$(cat "$exp")" ] && pass=$((pass+1)) || note "  MISMATCH $b"
done
run "corpus" "$total" "$pass"

note "== formatter: round-trip, idempotency, width =="
pass=0; total=0; bad=0; wide=0
for f in crates/jhol-vm/tests/js/*.js crates/jhol-vm/bench/*.js; do
  out=$(mktemp)
  "$BIN/fmt" "$f" > "$out" 2>/dev/null || { note "  FMT-ERR $(basename "$f")"; bad=$((bad+1)); rm -f "$out"; continue; }
  [ "$(awk 'length($0)>80' "$out" | wc -l | tr -d ' ')" = "0" ] || { wide=$((wide+1)); note "  WIDE $(basename "$f")"; }
  out2=$(mktemp); "$BIN/fmt" "$out" > "$out2" 2>/dev/null
  cmp -s "$out" "$out2" || { bad=$((bad+1)); note "  NON-IDEMPOTENT $(basename "$f")"; }
  rm -f "$out" "$out2"
done
run "formatter defects" "0" "$bad"
run "lines over 80 cols" "0" "$wide"

for f in crates/jhol-vm/tests/js/*.js; do
  b=$(basename "$f" .js); exp="crates/jhol-vm/tests/js/$b.expected"
  [ -f "$exp" ] || continue
  total=$((total+1))
  out=$(mktemp); "$BIN/fmt" "$f" > "$out" 2>/dev/null
  got=$("$BIN/run" "$out" 2>&1)
  [ "$got" = "$(cat "$exp")" ] && pass=$((pass+1)) || note "  FMT-CHANGED-SEMANTICS $b"
  rm -f "$out"
done

note "== real-world node_modules (parse/format/re-parse/idempotent) =="
NM=bench_cold/node_modules
if [ -d "$NM" ]; then
  out=$("$BIN/realfmt" "$NM" 2000 2>&1) || fail=1
  printf '%s\n' "$out" | tail -n 1
else
  note "  (no node_modules present — skipped)"
fi

note "== engine correctness vs node (adversarial) =="
ADV="$(cd "$(dirname -- "$0")" && pwd)/tests/adv"
if [ -d "$ADV" ]; then
  apass=0; axfail=0; afail=0
  for f in "$ADV"/adv-*.js; do
    b="${f%.js}"
    got=$("$BIN/run" "$f" 2>&1)
    if [ "$got" = "$(cat "$b.expected")" ]; then
      apass=$((apass+1))
    elif [ -f "$b.xfail" ]; then
      axfail=$((axfail+1))
    else
      afail=$((afail+1)); note "  ADV-FAIL $(basename "$f")"; fail=1
    fi
  done
  note "  ok   adversarial fixtures ($apass pass, $axfail known, $afail fail)"
else
  note "  (no adversarial fixtures — skipped)"
fi

note ""
if [ "$fail" = "0" ]; then note "GATE: PASS"; else note "GATE: FAIL"; fi
exit $fail
