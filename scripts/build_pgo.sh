#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo pgo instrument --release --example bench
./target/release/examples/bench runtime-benchmarks/fib.js 1 3 >/dev/null
./target/release/examples/bench runtime-benchmarks/loop.js 1 3 >/dev/null
./target/release/examples/bench runtime-benchmarks/json.js 1 3 >/dev/null
./target/release/examples/bench runtime-benchmarks/array.js 1 3 >/dev/null
./target/release/examples/bench runtime-benchmarks/objects.js 1 3 >/dev/null
./target/release/examples/bench runtime-benchmarks/strings.js 1 3 >/dev/null
cargo pgo optimize --release --example bench

echo "PGO-optimized binary: target/release/examples/bench"
