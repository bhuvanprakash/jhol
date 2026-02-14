#!/usr/bin/env bash
set -euo pipefail

JHOL_BIN=${JHOL_BIN:-target/release/jhol}
PACKAGES=${PACKAGES:-"lodash axios chalk"}
REPEATS=${REPEATS:-3}

python3 scripts/benchmark.py \
  --jhol-bin "$JHOL_BIN" \
  --packages $PACKAGES \
  --repeats "$REPEATS" \
  "${@}"