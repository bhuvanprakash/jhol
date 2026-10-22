#!/usr/bin/env bash
set -euo pipefail

REPORT=${REPORT:-week1-baseline-report.json}
CONFIG=${CONFIG:-benchmarks/week1_guardrails.json}

python3 scripts/bench/guardrails.py guardrails \
  --report "$REPORT" \
  --config "$CONFIG" \
  "${@}"