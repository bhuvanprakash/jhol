#!/bin/sh
# One entry point for "is the whole tree green?"
#
# CI runs `cargo test`, which never builds with `--all-features` and never runs the
# two quality gates — so the feature-gated half of the workspace, and every claim in
# PRODUCTION-READINESS.md, was checked only by hand. This is that missing entry point,
# and it is also the flake hunter: ROUNDS=5 re-runs the suites and fails if any round
# disagrees, which is what an unpinned flake looks like from the outside.
#
#   sh scripts/verify-all.sh                 # one round, everything
#   ROUNDS=5 sh scripts/verify-all.sh        # five rounds, for flake hunting
#   VERIFY_FAST=1 sh scripts/verify-all.sh   # skip the all-features build
#   VERIFY_SKIP_GATES=1 sh scripts/verify-all.sh
#
# `node` is the engine corpus oracle; a machine without it has the engine gate
# reported and skipped rather than failed.
set -u
cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)" || exit 1

ROUNDS=${ROUNDS:-1}
fail=0
say() { printf '%s\n' "$*"; }
step() { say ""; say "== $* =="; }

if [ "${VERIFY_FAST:-0}" != "1" ]; then
  step "cargo test --workspace --all-features"
  if ! cargo test --workspace --all-features --lib 2>&1 | tail -20; then
    say "FAIL --workspace --all-features"
    fail=1
  fi
else
  say "skipped: --workspace --all-features (VERIFY_FAST=1)"
fi

if [ "${VERIFY_SKIP_GATES:-0}" != "1" ]; then
  step "installer gate (crates/jhol-core/verify.sh)"
  if ! sh crates/jhol-core/verify.sh 2>&1 | tail -6; then
    say "FAIL installer gate"
    fail=1
  fi

  if command -v node >/dev/null 2>&1; then
    step "engine gate (crates/jhol-vm/verify.sh)"
    if ! sh crates/jhol-vm/verify.sh 2>&1 | tail -8; then
      say "FAIL engine gate"
      fail=1
    fi
  else
    say "skipped: engine gate needs node (the corpus oracle)"
  fi
fi

round=2
while [ "$round" -le "$ROUNDS" ]; do
  step "round $round of $ROUNDS"
  for crate in jhol-core jhol-vm jhol-treecheck jhol-ai; do
    if ! out=$(cargo test -q -p "$crate" --lib 2>&1); then
      say "FAIL $crate (round $round)"
      printf '%s\n' "$out" | grep -E 'FAILED|panicked at' | head -6
      fail=1
    fi
  done
  round=$((round + 1))
done

say ""
if [ "$fail" -ne 0 ]; then
  say "VERIFY-ALL: FAIL"
  exit 1
fi
say "VERIFY-ALL: PASS (rounds: $ROUNDS)"
