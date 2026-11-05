#!/bin/sh
# Feature-matrix checks: each optional feature combination must at least
# compile and serve `jhol --version`.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
fail=0; pass=0
for feats in \
  "runtime" \
  "runtime jholvm" \
  "runtime js-engine" \
  "bundle bundle-swc" \
  "runtime bundle jholvm" \
; do
  echo "  checking: --features $feats"
  if cargo check -q --features "$feats" 2>&1 | grep -qE '^error'; then
    echo "  FAIL: --features $feats"; fail=1
  else
    pass=$((pass+1))
  fi
done
echo "feature-matrix: $pass pass, $fail fail"
[ "$fail" = "0" ] || exit 1
