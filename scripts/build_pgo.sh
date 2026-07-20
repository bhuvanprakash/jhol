#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo-pgo >/dev/null 2>&1; then
    echo "cargo-pgo is not installed. Install it with: cargo install cargo-pgo"
    echo "(also requires llvm-profdata from LLVM to be on PATH)"
    exit 1
fi

BENCHES=(
    "bench"
    "bench_jsc"
)

FILES=(
    runtime-benchmarks/fib.js
    runtime-benchmarks/loop.js
    runtime-benchmarks/json.js
    runtime-benchmarks/array.js
    runtime-benchmarks/objects.js
    runtime-benchmarks/strings.js
)

for example in "${BENCHES[@]}"; do
    echo "=== PGO for example: $example ==="

    # Determine whether we need the JSC engine feature.
    features=""
    if [[ "$example" == "bench_jsc" ]]; then
        features="--features jsc-engine"
    fi

    cargo pgo instrument --release --example "$example" $features

    for file in "${FILES[@]}"; do
        ./target/release/examples/"$example" "$file" 1 3 >/dev/null || true
    done

    cargo pgo optimize --release --example "$example" $features
    echo "PGO-optimized binary: target/release/examples/$example"
done

echo "All PGO builds complete."
