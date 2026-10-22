#!/usr/bin/env bash
# One-shot local gate for JHOL_ORGANIZATION_ARCHITECTURE_BLUEPRINT.md
# (Phases B–D foundation + Phase A CLI/help evidence). No commit.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== foundation lib tests =="
cargo test \
  -p jhol-graph -p jhol-adapters -p jhol-receipt -p jhol-policy \
  -p jhol-task -p jhol-env -p jhol-diff -p jhol-ai -p jhol-db \
  -p jhol-remote \
  --lib --quiet

echo "== build bin =="
cargo build -q --bin jhol

BIN="$ROOT/target/debug/jhol"
echo "== CLI help / dispatch =="
HELP="$("$BIN" --help || true)"
for cmd in plan diff sync snapshot restore db ai dev; do
  grep -qE "^[[:space:]]+${cmd}[[:space:]]" <<<"$HELP" \
    || { echo "missing top-level command in --help: $cmd" >&2; exit 1; }
done
DBHELP="$("$BIN" db --help || true)"
grep -q plan <<<"$DBHELP"
grep -q probe <<<"$DBHELP"
AIHELP="$("$BIN" ai --help || true)"
grep -q verify <<<"$AIHELP"
grep -q infer <<<"$AIHELP"
echo "== blueprint verify =="
JHOL_BIN="$BIN" examples/blueprint/verify.sh

echo "== core lib (default features) =="
cargo test -p jhol-core --lib --quiet

echo "blueprint_gate: OK"
