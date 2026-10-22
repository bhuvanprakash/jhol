#!/usr/bin/env bash
# Release build that cannot oversubscribe a small machine.
#
# `cargo build --release` here defaults to thin LTO + `codegen-units = 1`, which
# peaks far beyond this laptop's headroom (it has crashed the machine twice).
# This wraps the build in memory-bounded settings plus a watchdog. The binary is
# functionally identical; it is simply less optimised. Rebuild with the default
# profile on a machine with headroom (or in CI) for the shipping artefact.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-false}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}"
exec python3 "$ROOT/scripts/mem_guard.py" --threshold "${JHOL_MEM_LIMIT:-95}" -- \
  cargo build --release --bin jhol "$@"
