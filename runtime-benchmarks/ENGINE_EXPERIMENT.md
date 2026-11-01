# JS Engine Backend Experiment

## Setup

- Machine: Apple Silicon (macOS)
- Rust: 1.86.0
- jhol-runtime: QuickJS backend, release build
- Bun: 1.3.14
- Node: 26.4.0
- JSC: macOS system JavaScriptCore.framework via `javascriptcore` crate 0.0.6
- Starlight: failed to build on Rust 1.86 (unmaintained, old `num-bigint` API)
- Boa: 0.19.1 built but array.js was ~28× slower than jhol; abandoned

## Median execution time (ms), lower is better

| benchmark | jhol (QuickJS) | Bun | Node | JSC | jhol/JSC |
|-----------|----------------|-----|------|-----|----------|
| array.js  | 56.73  | 15.49  | 35.84  | 43.20  | 1.31 |
| fib.js    | 743.30 | 45.52  | 101.33 | 416.66 | 1.78 |
| json.js   | 328.40 | 35.90  | 63.39  | 25.47  | 12.89 |
| loop.js   | 214.78 | 16.03  | 78.66  | 148.90 | 1.44 |
| objects.js| 306.37 | 426.15 | 761.51 | 557.81 | 0.55 |
| strings.js| 8.87   | 9.64   | 28.06  | 2.48   | 3.58 |

## Takeaways

1. **Starlight** is effectively dead — does not compile on current stable Rust.
2. **Boa** compiles but is currently much slower than QuickJS for these workloads.
3. **JSC** (plain system framework) is faster than QuickJS on 5/6 micro-benchmarks and only slower on `objects.js`.
4. **Bun** is still fastest on compute-heavy workloads because it adds its own JIT/optimizations on top of JSC.
5. JSC is available on macOS/iOS and can be provided by WebKitGTK on Linux, but not on Windows.

## Recommendation

Add an optional `jsc-engine` Cargo feature that swaps the JS backend to JavaScriptCore on Apple/Linux while keeping QuickJS as the default portable backend.
