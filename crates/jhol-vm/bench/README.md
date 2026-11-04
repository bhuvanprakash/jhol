# JholVM Benchmarks

JS benchmark fixtures + runner comparing JholVM against V8 (node) and bun.

## Run
    bench/run_bench.sh --node --runs 2       # reference timings
    bench/run_bench.sh --jholvm --runs 3     # JholVM only
    bench/run_bench.sh --all --runs 3 --json out.json
Engine selection: JHOL_RUNTIME_ENGINE=qjs|jsc|jholvm drives the `jhol` CLI;
node/bun run the .js files directly.

## Fixtures (what each measures)
- fib_recursive.js    naive recursive fib(30) with a LITERAL argument. The VM
                     constant-folds a pure call with a compile-time-known
                     argument, so this row is bound by process startup and does
                     NOT measure recursion. Measured fib(38) (126,491,971 calls,
                     counts match node): folded <0.005 s; with `let n = 38` (JIT,
                     pure) 0.14 s vs bun 0.17 s; with a captured counter (impure,
                     interpreter fallback) 4.46 s = ~26x bun.
- iter_sum.js         tight integer loop 0..50000000
- string_build.js     string concat + full char-scan checksum
- object_props.js     200k allocs, 4-prop read pattern
- polymorphic_props.js 3 shapes + dynamic property adds
- array_ops.js        map/filter/reduce pipeline over 200k
- closure_calls.js    1M closure invocations (upvalue state)
- method_calls.js     1M method dispatches (add/mul)
- json_roundtrip.js   JSON.stringify/parse of 5000 entries
- sort_bench.js       LCG fill + numeric sort of 100k ints
- gc_stress.js        2M short-lived object allocations

## Reading output
- Markdown table: rows = fixture, columns = backend, cell = median ms.
- "Speedup vs node" sums JholVM time vs node (>1 = JholVM slower).
- Missing backend binary => `n/a`; --json writes machine-readable stats.
- expected_outputs.txt = exact stdout node must produce (regression check).
- Exit 1 means a fixture failed under node; otherwise exit 0.
