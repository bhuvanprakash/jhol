#!/bin/sh
# Usage: sh run_corpus.sh [command-that-runs-one-js-file]
# Examples:
#   sh run_corpus.sh node              # reference check with Node
#   sh run_corpus.sh /path/to/jholvm   # conformance check with JholVM
CMD="${1:-node}"
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
pass=0
fail=0
total=0
out=$(mktemp)
trap 'rm -f "$out"' EXIT
for f in "$DIR"/[0-9][0-9]-*.js; do
  total=$((total + 1))
  base=${f%.js}
  if "$CMD" "$f" > "$out" 2>/dev/null; then
    if diff -q "$base.expected" "$out" >/dev/null 2>&1; then
      echo "PASS ${f##*/}"
      pass=$((pass + 1))
    else
      echo "FAIL ${f##*/} (output mismatch)"
      fail=$((fail + 1))
    fi
  else
    echo "FAIL ${f##*/} (runtime error)"
    fail=$((fail + 1))
  fi
done
echo "$pass/$total passed"
[ "$fail" -eq 0 ]
