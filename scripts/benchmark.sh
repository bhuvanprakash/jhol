#!/usr/bin/env bash
set -euo pipefail

JHOL_BIN=${JHOL_BIN:-target/release/jhol}
SUITE=${SUITE:-small}
PACKAGES=${PACKAGES:-}
FIXTURE_WORKLOADS=${FIXTURE_WORKLOADS:-}
REPEATS=${REPEATS:-5}

CMD=(
  python3 scripts/benchmark.py
  --jhol-bin "$JHOL_BIN"
  --suite "$SUITE"
  --repeats "$REPEATS"
)

if [[ -n "$PACKAGES" ]]; then
  # shellcheck disable=SC2206
  PKG_ARR=($PACKAGES)
  CMD+=(--packages "${PKG_ARR[@]}")
fi

if [[ -n "$FIXTURE_WORKLOADS" ]]; then
  # shellcheck disable=SC2206
  FIXTURE_ARR=($FIXTURE_WORKLOADS)
  CMD+=(--fixture-workloads "${FIXTURE_ARR[@]}")
fi

"${CMD[@]}" "${@}"