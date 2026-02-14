#!/usr/bin/env bash
set -euo pipefail

FIXTURES_DIR=${FIXTURES_DIR:-tests/fixtures}
MATRIX=${MATRIX:-benchmarks/framework_matrix.json}
CONFIG=${CONFIG:-benchmarks/framework_guardrails.json}
OUT=${OUT:-framework-compat-report.json}

python3 scripts/framework_compat_report.py \
  --fixtures-dir "$FIXTURES_DIR" \
  --matrix "$MATRIX" \
  --config "$CONFIG" \
  --out "$OUT" \
  "${@}"