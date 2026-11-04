# jhol 2.0.0 "Damascus" — Release Report

| | |
|---|---|
| Product | jhol — cache-first, npm-compatible package manager with an embedded JavaScript runtime |
| Release | **2.0.0 "Damascus"** |
| Report date | 2026-11-03 |
| Status | Release candidate. Verified on the dev machine. **Not tagged or committed** (owner rule: agents do not write to git). |
| Engine | JholVM — jhol's own JavaScript engine (lexer, parser, compiler, VM, GC, JIT), written from scratch |
| Oracle | node (behaviour). No competitor source was opened at any point. |

---

## 1. Summary

jhol 2.0.0 is the release where the engine stops being a side project and becomes part of the product. JholVM runs inside `jhol run`, passes the full workspace suite, ships in the default build, and sits at parity with bun on the shipped benchmark suite while running about 7.6× faster than node on the same machine.

Parity is the honest headline, not domination. Across eleven benchmarks jhol wins five against bun, ties one, and loses five. We say so in this report because a release that understates its losses is harder to trust on its wins.

What this release delivers:

- A JavaScript engine that is correct against node on the project's corpus and fast on the benchmark suite.
- A default build that actually contains the engine. Before this release a default build could not run it at all (section 3.1).
- A green workspace: 1008 tests passing, zero failing, on the release tree.
- A repository that passes its own format gate (zero diffs). It did not before this session.

---

## 2. What ships in 2.0.0

Drawn from the 2.0.0 section of `CHANGELOG.md`.

### 2.1 Breaking changes (each verified byte-identical against node)

- `JSON.parse` is strict: `truX`, `01`, `1.`, `.5`, bare `-`, raw control characters and trailing garbage now throw, as in node.
- `Number(undefined)` is `NaN` (was `0`). `Number()` and `Number(null)` remain `0`.
- `toFixed` rounds ties away from zero on the exact decimal expansion: `(0.5).toFixed(0)` is `"1"`.
- `in` and `for-in` treat array elisions as holes: `1 in [1, , 3]` is `false`.
- Console output follows node's inspect style (quoted strings in containers, `-0` printed as `-0`).
- Engine-internal slots (`#get:` / `#set:`) are filtered from `Object.keys` and `getOwnPropertyNames`.
- Writes at array indices ≥ 4096 now stick, and numeric object keys resolve under either spelling.

### 2.2 Added

- `Date` with epoch arithmetic, ISO parsing, local and UTC accessors, and string formatting.
- `WeakMap` / `WeakSet` (identity-keyed; GC-naive retention documented).
- `Symbol.for`, `Symbol.keyFor`, well-known symbols.
- `atob` / `btoa`, `crypto.getRandomValues`, `crypto.randomUUID`, `Object.is`, the `Function` binding, `EvalError`, `URIError`.

### 2.3 Performance

- The JIT learned to allocate. Flush-to-frame root maps let the collector see values held in JIT registers, so region compilation covers object literals, property access and native calls, not only numeric loops. `jit.rs` grew from roughly 400 to roughly 4,100 lines.

### 2.4 Installer

- Script receipts: install scripts are approved per `name@version` plus the exact script text plus the tarball SRI. A republished tarball, a changed script line or a version bump invalidates the approval. This addresses the install-script supply-chain path directly.

---

## 3. What this session added (2026-11-03)

These changes sit on top of the 2.0.0 feature set and were made during release hardening.

### 3.1 The engine ships in the default build

`jholvm` was reachable only through an optional feature. `jhol-runtime` compiled with `default, jsc-engine, mimalloc, native-engine, swc-transpile` and no `jholvm`, so `JHOL_RUNTIME_ENGINE=jholvm` failed in a default build with:

```
requested an engine this build does not include; rebuild with `--features jholvm`
```

The fix is one line in the root `Cargo.toml`: `jholvm` is now in `default`. Verified:

- `cargo tree -p jhol` lists `jholvm` in the `jhol-runtime` feature set.
- A plain default run of the engine fixture prints `832040` and exits 0.
- Release binary grew from 18.4 MB to 19.1 MB, consistent with the engine being compiled in.

### 3.2 Test-suite repairs

- **Stale assertion.** `test_install_offline_without_cache_fails` expected `not in cache`. The store was renamed, the message became `not in the content store`, and the test failed although the behaviour was correct (exit 1, nothing installed). The assertion now checks the semantics: the message identifies an offline miss and names the package.
- **Flaky daemon socket tests.** The fixture created its directory in shared `/tmp` with `create_dir(...).expect(...)`. Under disk pressure that panics. Replaced with `tempfile::Builder::tempdir()`: per-test directory, removed on drop, no `/tmp` race. 5/5 pass.

### 3.3 Format gate

The repository had never passed `cargo fmt --all --check`: 91 files drifted. Before formatting, the 13 dirty files were snapshotted. The result was checked with a byte-diff: whitespace-only changes in 7 files and argument-order or module-order reshuffles in 6. No logic changed. The gate now reports zero diffs.

### 3.4 Benchmark record corrected

`HOW-WE-WON.md` claimed 11 of 11 wins and a 1.42× lead over bun. That did not reproduce. The page now reports the measured result (section 5), keeps the earlier session's table labelled as measured under load 12.6, and withdraws the 11/11 headline as a current claim.

### 3.5 Documentation

- `CHANGELOG.md`: new top section `[Unreleased] — the engine ships in the box`.
- `docs/engineering/PRODUCTION_READY_2026-11-03.md`: dated fix ledger with evidence.
- `docs/reports/`: benchmark JSON, two-page benchmark report (PDF and HTML), and this release report.
- `crates/jhol-vm/bench/README.md`: fixture note explaining what the `fib_recursive` row does and does not measure.

---

## 4. Verification

| Gate | Result | Notes |
|---|---|---|
| `cargo check --workspace --all-targets` | exit 0 | warnings only |
| `cargo test --workspace --no-fail-fast` | **1008 passed / 0 failed / 16 ignored** | measured on the release tree, from a clean debug cache |
| `cargo fmt --all --check` | 0 diffs | was 91 files before this session |
| Release build, default features | exit 0, 19,077,456 B | safe wrapper, memory-bounded |
| Engine in default build | `832040`, exit 0 | fixture `crates/jhol-vm/bench/fib_recursive.js` |
| Install smoke | 1 package in 27 ms, exit 0 | `is-number@7.0.0`, fresh cache |
| Installed binary | `~/.cargo/bin/jhol` reports `jhol 2.0.0` | |

Corpus figures quoted in the public report (44/44 CLI examples byte-identical to node; engine corpus 1,483/1,483; 86/86 engine unit tests) come from the project record in `HOW-WE-WON.md`. They were not re-run in this session, and this report does not present them as re-measured.

---

## 5. Benchmarks

### 5.1 Method

- Harness: `crates/jhol-vm/bench/run_bench.sh --all --runs 3`. Each engine runs each fixture three times; the reported figure is the median wall-clock time of the whole process.
- Engines: jholvm (`jhol run` with `JHOL_RUNTIME_ENGINE=jholvm`), bun 1.4.4, node, and the QuickJS and JavaScriptCore backends of the same `jhol` binary.
- Eleven fixtures: array_ops, closure_calls, fib_recursive, gc_stress, iter_sum, json_roundtrip, method_calls, object_props, polymorphic_props, sort_bench, string_build.
- Two runs on 2026-11-03: a quiet run (load 6.9) and a same-day run under heavier load (load 9+).

### 5.2 Suite totals

| Run | jholvm | bun | node | QuickJS | JSC | jholvm vs node |
|---|---:|---:|---:|---:|---:|---:|
| Quiet (load 6.9) | **182.7 ms** | 191.1 ms | 1,393.6 ms | 2,176.3 ms | 620.9 ms | **7.63×** |
| Loaded (load 9+) | **209.7 ms** | 256.8 ms | 1,505.0 ms | — | — | **7.18×** |

Against bun, jholvm is 4.6% faster on the quiet run and 18% faster on the loaded run. Both runs favour jholvm in total. Per-row results do not favour it uniformly, which is why the per-row table below matters more than the totals.

### 5.3 Per-benchmark medians, quiet run (ms, lower is better)

| Benchmark | jholvm | bun | node | QuickJS | JSC | vs bun |
|---|---:|---:|---:|---:|---:|---|
| array_ops | **13.8** | 14.5 | 107.1 | 27.5 | 34.4 | win |
| closure_calls | 11.1 | 11.1 | 92.7 | 35.5 | 28.9 | tie |
| fib_recursive | 13.8 | **12.4** | 99.4 | 8.0 | 10.0 | loss (see 6.2) |
| gc_stress | **10.0** | 13.7 | 98.2 | 694.3 | 90.8 | win |
| iter_sum | **38.5** | 43.9 | 382.8 | 1,090.2 | 312.2 | win |
| json_roundtrip | 13.7 | **10.3** | 97.3 | 28.6 | 13.8 | loss |
| method_calls | 13.9 | **10.5** | 98.6 | 62.5 | 40.9 | loss |
| object_props | 19.0 | **13.5** | 102.0 | 90.8 | 47.8 | loss |
| polymorphic_props | **10.1** | 16.4 | 92.0 | 63.6 | 22.7 | win |
| sort_bench | **22.8** | 29.4 | 122.9 | 62.8 | 39.4 | win |
| string_build | 16.0 | **15.4** | 106.9 | 52.5 | 33.5 | loss |

Bold marks the fastest engine in the row. The tie band is ±4%.

### 5.4 Head-to-head summary

| Opponent | Wins | Ties | Losses | Reading |
|---|---:|---:|---:|---|
| bun | 5 | 1 | 5 | parity in total; jholvm faster on the heavy loops and allocation |
| node | 11 | 0 | 0 | faster on every row; 7.63× on the total |
| QuickJS | 10 | 0 | 1 | loses fib_recursive only (8.0 vs 13.8 ms) |
| JSC | 9 | 1 | 1 | loses fib_recursive (10.0 vs 13.8) and ties json_roundtrip |

Against QuickJS and JSC the table counts a fixture as a loss only when jholvm is slower outside the ±4% band. The fib row is discussed in 6.2.

---

## 6. Known behaviour and limits

### 6.1 Four losses to bun on property and string workloads

json_roundtrip, method_calls, object_props and string_build are 20–40% slower than bun. These are workloads where bun's engine specialises heavily. This is the most useful optimisation target for the next release.

### 6.2 `fib_recursive` measures two different things

The shipped fixture calls `fib(30)` with a literal argument. The VM constant-folds a pure call whose argument is known at compile time, so that row measures process startup, not recursion. Measured on the same family of programs at `fib(38)` (126,491,971 calls, count matched against node):

| Shape | Time | Per call | Regime |
|---|---:|---:|---|
| `const n = 30 + 8; fib(n)` | < 0.005 s | — | constant-folded |
| `let n = 38; fib(n)` | **0.14 s** | ~1.1 ns | JIT-compiled (faster than bun's 0.17 s) |
| captured counter (impure) | 4.46 s | ~35 ns | interpreter fallback (~26× bun) |
| ternary form (pure) | 2.30 s | ~18 ns | not folded, not JIT-fast |

The JIT path is strong. The interpreter fallback is the single largest open performance gap. The benchmark README now states that the fixture does not measure recursion.

### 6.3 Other limits

- **Platforms.** Verified on macOS arm64 only. Linux, Windows and `--all-features` were not run in this session.
- **CI.** `.github/workflows/**` is untracked by owner decision, so no hosted CI has run `ci.yml` or `benchmark.yml`. The workflows were checked for runnability locally.
- **Corpus.** The 1,483-file corpus and the 44 CLI examples are project record figures (section 4), not re-run here.
- **Changelog structure.** The changelog keeps several `[Unreleased]` sections in its house style. They are not merged.
- **Host constraints.** The development machine has 16 GB RAM and a shared disk that repeatedly reached 2–4 GB free during this work. Build caches were pruned as needed and no repository content was deleted.
- **External tool.** The requested `repo-analyze` command could not run (`npm error code E404`). Local repository statistics were used instead: 168 commits, 16 crates, 153,109 lines of Rust.

---

## 7. Release checklist

| Item | Status |
|---|---|
| Version 2.0.0 in `Cargo.toml` and `CHANGELOG.md` | done |
| Workspace check passes | done (exit 0) |
| Workspace tests pass | done (1008 / 0 / 16) |
| Format gate passes | done (0 diffs) |
| Engine in default build | done |
| Release binary built and installed | done |
| Benchmark re-run and report corrected | done |
| Documentation and dated fix ledger | done |
| Commit the release | **owner** |
| Tag `v2.0.0` | **owner** |
| Decide whether CI workflows are tracked | **owner** |
| Run Linux and Windows | open |
| Interpreter-fallback performance project | open, next release |

---

## 8. Reproduce

```sh
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2
cargo check --workspace --all-targets
cargo test --workspace --no-fail-fast
cargo fmt --all --check
bash scripts/build_release_safe.sh          # default features, includes jholvm
bash crates/jhol-vm/bench/run_bench.sh --all --runs 3 --json bench.json
./target/release/jhol run crates/jhol-vm/bench/fib_recursive.js   # prints 832040
```

---

## 9. Artifacts

- `docs/reports/RELEASE_REPORT_2.0.0_DAMASCUS.md` — this document
- `docs/reports/JHOL_VS_COMPETITORS_2026-11-03.pdf` and `.html` — two-page benchmark report
- `docs/reports/bench-data-2026-11-03.json` — raw measurements
- `docs/engineering/PRODUCTION_READY_2026-11-03.md` — dated fix ledger
- `CHANGELOG.md` — 2.0.0 section and the `[Unreleased] — the engine ships in the box` section
- `HOW-WE-WON.md` — corrected benchmark record

---

jhol 2.0.0 "Damascus" ships a real engine, in the box, with a green suite and an honest benchmark record. It is at parity with bun and about 7.6× faster than node on this suite. The next release has a clear target: the interpreter fallback and the four property and string workloads where bun still leads.
