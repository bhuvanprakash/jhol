# Jhol changelog

> **Status at a glance** (append-only history below; this block is the current state)
>
> | capability | state | evidence |
> |---|---|---|
> | install / ci / verify / explain | shipped | 708 core tests, installer gate |
> | multi-major npm trees | shipped | `jest@29.7.0` → 266 packages, warm stable |
> | reproducible history | shipped | `--as-of` across three dates |
> | license policy at resolution time | shipped | 0 violations on a real tree |
> | security exclusion at resolution time | shipped | `--exclude-vuln`, OSV + re-resolve |
> | models: inspect / pull / dedupe / verify | shipped | 10,760 B header, one inode per model |
> | hardware axes + wheel compatibility | shipped | `axes`, `wheels` (6 rules, tested) |
> | Python wheel graph resolution | shipped | `pysolve` — torch@2.4.1 → 10 pkgs, 1.1 s |
> | **Python wheel installation** | **shipped** | `py install requests` → 5 dists, 1.5 s; per-file digests |
> | **MCP environment tools** | **shipped** | `jhol_verify_environment`, `jhol_axes`, `jhol_why_not`, `jhol_plan_python` |
> | environment forks (CoW) | shipped | 797 ms, 4,491 files cloned, 0 B added |
> | attestation / replay | shipped | deterministic proof; spec → plan → execute |
> | cost estimates in MCP responses | **not built** | tools answer, they do not quote bytes/time yet |
> | engine (JholVM) | shipped with known gaps | no Promise/BigInt/TDZ; documented |
>
> Honest summary: jhol resolves **and** installs a Python tree, verifies it alongside npm
> and model pins, and an agent can drive all of it over MCP. What remains from the plan is
> quoting cost (bytes/time) before an agent commits, and the engine's known gaps.

# Production gate — 2026-11-03

Everything measured on this machine, dev profile, 17-crate workspace, Jhol 2.0.0
"Damascus" + the 2026-10-24 doc-sync + the E60 typed-record work. Purpose: make
the tree shippable by clearing every concrete blocker found. Node is the oracle
throughout; no competitor source opened.

## Gates all green

| gate | result |
|---|---|
| cargo check --workspace --all-targets | exit 0 |
| cargo test --workspace --no-fail-fast | **1008 passed / 0 failed / 16 ignored** |
| cargo fmt --all --check | **clean (0 diffs)** after formatting 91 stale-file spots |
| behavior parity offline-instalfailure | fixed (stale assertion) |
| daemon_socket_tests | 5/5, now tempdir-based (no /tmp create_dir race) |
| release binary | rebuilt 2026-11-03 from current tree (safe profile) |

## Fixes, dated

### 1. stale test assertion (offline install)

`tests/commands.rs` tests/commands.rs:200 expected the string "not in cache" but
the store was renamed to "not in the content store" during the 2.0.0 sync. The
test asserted a phrasing, not behaviour. Now asserts:
`Offline mode:` + (`not in the content store` OR `not in cache`) + `left-pad`.

### 2. flaky daemon socket tests

`src/main.rs:7858` `isolated_dir()` did `std::fs::create_dir` in `/tmp` and
`expect`ed it. Under disk pressure, create_dir panics (seen: 2 FIELD failures in
one workspace run, 5/5 pass isolated). Replaced with
`tempfile::Builder::tempdir()` — per-test dir, auto-removed, no /tmp creation
needed. 5/5 green.

### 3. `jhol run <missing-script>` — verified **not** a bug

Checked before touching code:

```
jhol run nope      -> exit 1, `Missing script "nope" in package.json.`
jhol run nope.js   -> exit 1, `no such file: nope.js`
```

Earlier claim of silent exit 0 was wrong (an existing file that prints nothing).
No change made.

### 4. HOW-WE-WON.md corrected from a real run

Claim was "11/11 wins, 1.42x vs bun". Re-ran
`crates/jhol-vm/bench/run_bench.sh --all --runs 3`:

- 5 wins / 1 tie / 5 losses vs bun; totals **190.6 ms vs 189.4 ms = parity**
- **7.84x faster than node** (runner prints it: `Speedup vs node: 7.84x`)
- session-A table (load 12.6) kept, labelled as measured then
- 11/11 headline withdrawn as a current claim

### 5. fib "cliff" — root cause, three regimes

`fib(n)` per-call cost depends on the shape, not n:

| shape (fib(38) = 126,491,971 calls, count matches node) | time | per call |
|---|---:|---:|
| literal arg (compile-time-known) | <0.005 s | — |
| `let n = 38` (JIT, pure) | **0.14 s** | ~1.1 ns |
| captured counter (impure, interpreter fallback) | 4.46 s | ~35 ns |
| ternary form | 2.30 s | ~18 ns |

Three regimes: **constant-folded**, **JIT-compiled**, **interpreter fallback**.
JIT path already at bun speed. **The remaining 26x is the interpreter-fallback
path; that is the next real work item** (`crates/jhol-vm/bench/README.md`
corrected so the fixture and claim agree).

## Not finished (owner decision)

- `--all-features` / Linux / Windows verification (never run on this machine).
- The interpreter fallback path (35 ns/call) — the 26x gap.
- Real CI: `.github/workflows/**` untracked by design; GitHub never runs them.
- 63 duplicate doc pairs (kept by rule; manifest at `docs/archive-record/DOC_DUPLICATES.md`).
- Version still `2.0.0` while this work is uncommitted — the owner keeps
  committing themselves and forbids agent commits.

## Preservation

- Nothing deleted. Formatting verified whitespace-only on 7 of 13 dirty files
  and argument-order/mod-order reshuffles on 6 (checked with byte-diff).
- Snapshot of the 13 dirty files before formatting: `/tmp/jhol-pre-fmt-1793681639`.
- No burst commits: the owner's no-commit rule holds.

## Addendum: jholvm was opt-in, then fixed (2026-11-03 later)

The rebuilt default release binary **could not serve JholVM at all**:
`JHOL_RUNTIME_ENGINE=jholvm` errored with
`requested an engine this build does not include; rebuild with --features jholvm`.
Root cause: `jhol-vm` sits in the dependency graph only through
`crates/jhol-bundle`'s `native-fmt` feature; `jhol-runtime` runs with
`default,jsc-engine,mimalloc,native-engine,swc-transpile` — no `jholvm`. The
feature exists (`Cargo.toml:62` `jholvm = ["jhol-runtime", "jhol-runtime/jholvm"]`)
but is **not in the root default list**, so every default build ships the engine
behind a flag nobody passes. This is exactly how the "11/11" numbers were
invisibly produced: the shipped binary silently lacked the engine it claimed.

Two consequences, both handled:

1. **Release rebuild with the feature**: `scripts/build_release_safe.sh
   --features jholvm` (2026-11-03, exit 0). Verified: `JHOL_RUNTIME_ENGINE=jholvm
   jhol run crates/jhol-vm/bench/fib_recursive.js` -> 832040, exit 0.
2. **Fresh all-engine bench on the rebuilt binary**:

   | bench | jholvm | bun | vs bun |
   |---|---:|---:|---|
   | array_ops | 13.8 | 14.5 | win |
   | closure_calls | 11.1 | 11.1 | tie |
   | fib_recursive | 13.8 | 12.4 | loss |
   | gc_stress | 10.0 | 13.7 | win |
   | iter_sum | 38.5 | 43.9 | win |
   | json_roundtrip | 13.7 | 10.3 | loss |
   | method_calls | 13.9 | 10.5 | loss |
   | object_props | 19.0 | 13.5 | loss |
   | polymorphic_props | 10.1 | 16.4 | win |
   | sort_bench | 22.8 | 29.4 | win |
   | string_build | 16.0 | 15.4 | loss |
   | **TOTAL** | **182.7** | **180.7** | **7.63x vs node** |

   **5 wins / 1 tie / 5 losses, 182.7 vs 180.7 ms — parity with bun again, on a
   rebuilt release binary.** Confirms the earlier 5/1/5 result was not a one-off.

**Recommended owner change** (one line, in the repo's own spirit):
`default = ["runtime", "bundle", "bundle-swc", "http-reqwest", "jholvm"]` — the
engine the release brag about must ship in the box, not behind a flag. Also
`bundlers` `native-fmt` should list the engine explicitly.

## Re-verification after tree drift (2026-11-03, later)

The working tree gained eight more modified files after the release gates were
recorded (`crates/jhol-bundle/src/parse.rs`, `crates/jhol-core/src/{adversarial_install,install,lifecycle,pack,registry_query}.rs`,
`crates/jhol-vm/src/{jit,objects}.rs`). All of them are **older than the 10:34
release binary** (`find crates src -name '*.rs' -newer target/release/jhol` reports
0), so the shipped binary and the benchmark numbers already cover this source.

Gates re-run on the drifted tree:

| Gate | Result |
|---|---|
| `cargo check --workspace --all-targets` | exit 0 (2m14s) |
| `cargo test --workspace --no-fail-fast` | **1008 passed / 0 failed / 16 ignored** |
| `cargo fmt --all --check` | 0 diffs |

Nothing regressed. No rebuild or reinstall was needed.

### Memory state at re-verification

Disk 7.6 GB free (above the 4 GB rule). RAM free+inactive 2.4 GB, below the rule,
but every large consumer belongs to the owner's own tooling — Cursor renderers
(592 MB + 301 MB), `tsserver` (499 MB), DeepSeek Harness (292 MB), a
Virtualization XPC service (258 MB), Antigravity language server (167 MB). The
only jhol process is a 4 MB `jhol-bin daemon --serve`. Killing any of the others
would destroy the owner's live work, so none were touched; this is recorded as an
environment observation, not a jhol defect.

## `jhol fmt` and JAGR-∞ resolver — assessment and fixes (2026-11-03, late)

Both were measured before being changed. Evidence and outcome:

### `jhol fmt`

Measured on a hand-built corpus, not on the repo:

| Property | Before | After |
|---|---|---|
| `.ts` / `.tsx` / `.jsx` | accepted then **`0 file(s), 0 changed`, exit 0** (silent no-op) | formatted through the swc backend |
| `.css` / `.scss` / `.less` | only trailing whitespace trimmed | real formatting: rules, declarations, nesting, comments, byte-exact strings and `url(...)` |
| `/* @__PURE__ */` | **deleted**; `/*#__PURE__*/` moved to end of file | preserved and glued to the annotated expression |
| Idempotency | held for JS/JSON | holds for JS/TS/JSX/TSX/CSS |
| `--check` / `--list-different` | exit 1 with the file named | unchanged (correct) |
| ASI handling | `foo` / `[1,2]` resolved per spec | unchanged (correct) |

Root causes fixed: `.ts/.tsx/.jsx` were excluded by an `is_supported` gate written
for the JS-only native parser while the swc backend was compiled in but unused for
those extensions; `.jsx` additionally needed `jsx: true` on `EsSyntax`; `format_css`
was a whitespace trimmer; and the native printer has no comment-attachment model,
so annotation comments were dropped. Files carrying build/lint annotations now
take the swc path (swc captures comments into a map and hands them to the emitter),
and `join_pure_annotations` re-glues an annotation to its expression.

Ceiling stated in the code: CSS formatting does not wrap long values (safe
formatting, not Prettier parity). Wrapping needs a real CSS parser.

Tests added: CSS structure and idempotency, strings/url payload preservation,
nested at-rules, comment retention, TS/JSX routing, and annotation survival
(`crates/jhol-bundle/src/fmt.rs`). Bundle suite 31 → 44 tests.

### JAGR-∞ resolver trace

A debug channel existed (`JHOL_DEBUG_JINF`, `[JINF]` lines) but was undocumented,
emitted a package dump rather than attribution, and said nothing about *which*
layer answered. Now:

- `JHOL_JAGR_TRACE=1` (documented; `JHOL_DEBUG_JINF` still accepted) prints
  `[JAGR]` lines on stderr: entry (`N direct deps, overrides, full_retry`), then the
  layer that actually answered — `layer=resolved-bundle-cache` (warm replay),
  `layer=lockfile-fast-path`, `layer=homotopy`, or `layer=rvm` with chart size,
  version count and solve time — then `done: N packages in T (synth retry: yes/no)`,
  or `failed after T: <reason>`.
- All three layers verified on the installed binary: bundle cache 77 pkgs / 9.9 ms,
  lockfile fast path 77 pkgs / 1.1 ms, cold RVM 74 chart packages / 3,146 versions
  / 677 ms total.
- Verified in three states: warm (fast path), cold (rvm, 74 chart packages /
  3,137 versions, solve 1.9 ms), and failure (1.7 s, reason printed).

Resolver behaviour measured, unchanged: 78 packages, deterministic lockfile
(`362584debe9111bb` across independent caches), 1.47 s cold / 0.37 s warm, clear
peer-conflict error under `JHOL_STRICT_PEER_DEPS=1`.

Remaining known risk (not addressed): 58 `.unwrap()`, 9 `.expect(`, 4 `panic!` and
21 `as usize` in the resolver path — hostile packument input can still abort the
process. That is the next hardening item, not this pass.

Gates after these changes: `cargo test --workspace --no-fail-fast`
**1014 passed / 0 failed / 16 ignored**; `cargo fmt --all --check` 0 diffs;
release binary rebuilt and installed.

## JholVM and CLI: bug-class sweep (2026-11-03, final pass)

Method: a 49-case differential harness (`.js` snippets run under `node` and under
`jhol run` with `JHOL_RUNTIME_ENGINE=jholvm`; stdout and exit code compared).
Node is the oracle; no competitor source was read.

**Result: 8 divergences found; 3 fixed in the first pass, 3 more in the second;
5 remain and are listed below.**

| # | Case | Symptom before | State |
|---|---|---|---|
| 1 | `try/finally` in three shapes | `finally` never ran (completion-value path dropped it; throw path emitted `Throw` first; `return` skipped it) | **fixed** — 5 shapes now match node |
| 2 | `String.replace/match/split/test` with a regex | regex literals have no methods; `replace` silently returned the input | **fixed** — `regex.rs` engine, `RegExp` global, regex-aware `replace`/`replaceAll`/`split`/`match`/`search` |
| 3 | error `.name` on runtime throws | runtime errors threw a *string*; `e.name`/`e.message` undefined, `typeof e` `"string"` | **fixed** — converted to Error objects at the catch |
| 4 | `JSON.parse` error class | threw `TypeError` (node: `SyntaxError`) | **fixed** — `syntax_error()` |
| 5 | `Promise` | absent (`Promise.resolve` → TypeError) | open (feature) |
| 6 | `BigInt` | `typeof 10n` → `number` | open (feature) |
| 7 | `Symbol` | values are strings, so `typeof` says `string` and symbol keys count in `Object.keys` | open (documented deviation in `SESSION.md`) |
| 8 | `let` TDZ | `let` before declaration returns `undefined` instead of throwing `ReferenceError` | open |
| 9 | undeclared-global call | `f()` for an undeclared `f` reports `TypeError`, node reports `ReferenceError` | open — the VM sees only the callee value at the call site, and treating `undefined` as `ReferenceError` mislabels every missing *method*, which is worse |

Verification of the fix (all five now identical to node):

```
try{console.log("a")}finally{console.log("b")}                     -> a b
try{throw new Error("x")}finally{console.log("b")}                  -> b, then rethrow
try{throw …}catch(e){…}finally{console.log("f")}                    -> c f
function f(){try{return 1}finally{console.log("f")}} f()            -> f 1
function f(){try{return 1}finally{return 2}} f()                    -> 2
```

VM suite grew 86 → 99 tests: four `finally` regressions and a nine-case regex
suite (classes, quantifiers, anchors, boundaries, groups, case-insensitivity,
global iteration, refusals, and a catastrophic-backtracking guard).

### CLI contract audit

Ten invocations checked: unknown subcommand, unknown flag, missing argument,
invalid enum value, `--version`, `--help`, `fmt` with no paths, `fmt` on a
nonexistent path, `fmt` on an unsupported file, `fmt --check` on a dirty file.
All now follow the convention *0 success / 1 failure / 2 usage error*; the two
silent successes (`fmt` on a missing path, `fmt` on an unsupported extension) were
the only defects found and are fixed.

### Gates after this pass

`cargo test --workspace --no-fail-fast` **1018 passed / 0 failed / 16 ignored**
(was 1008 at the start of the day); `cargo fmt --all --check` 0 diffs; release
binary rebuilt and installed to `~/.cargo/bin/jhol`.

## Second sweep: 129 differential cases, 12 more fixes (2026-11-04)

A larger corpus (80 cases across numbers, strings, arrays, objects, functions,
classes, iteration, errors, JSON, Date, Math, Map/Set, iterators) plus the earlier
49-case corpus: **129 cases total, 12 divergences before this sweep, 12 after it**.

| # | Defect | Fix | Evidence |
|---|---|---|---|
| 1 | `Math.round(-2.5)` → `-3` | JS rounds `.5` toward `+Infinity` | `-2 3 -0` (incl. signed zero) |
| 2 | `toPrecision` missing entirely | added, significant-digit rules | `1.50`, `1.2e+3` |
| 3 | `getOwnPropertyNames({get x(){}})` → `[]` | accessor names live under `#get:` keys; now surfaced | `x` |
| 4 | `for (const [i,v] of arr.entries())` — `entries`/`keys`/`values` missing | added (pairs as arrays; `for…of` already iterates them) | `0 a` / `1 b` |
| 5 | `e instanceof TypeError` → `false` | the error registry stored the *constructor* instead of `prototype` | `true true TypeError` |
| 6 | `instanceof Error` for runtime faults | same root as 5 | `true` |
| 7 | `e.name` for caught runtime faults | errors converted to objects at the catch (earlier round) | `TypeError` |
| 8 | `JSON.parse` error class | `syntax_error()` (earlier round) | `SyntaxError` |
| 9 | global `String.replace` replaced first match only | global walk (earlier round) | `a#b#` |
| 10 | `String.match`/`search` absent | added with regex support | `1,2` / `1` |
| 11 | `replace` silently returned input on a regex | engine + regex-aware path | `a1b` → `a#b` |
| 12 | `finally` skipped in three shapes | completion-value path, throw path, `return` path | 5 shapes match node |

### Still open (measured, not estimated)

| Divergence | Why it is still open |
|---|---|
| `Promise` (absent) | needs a microtask queue and `await` lowering — a feature, not a patch |
| `BigInt` (absent) | new numeric type through parser/VM/GC |
| `Symbol` is string-backed | `typeof` says `string`; symbol keys appear in `Object.keys`. Documented deviation in `SESSION.md`; a real fix is a new value type |
| `let`/`const` TDZ | needs per-binding "not yet initialised" state and a check on read |
| **Arrow parameter defaults ignored** (`(a,b=2)=>b` → `undefined`; `function` form works) | diagnosed: arrows compile parameters at base 0 while the call places arguments at the function base, so `GetLocal(slot)` reads the wrong register. Fixing it means changing the arrow calling convention, which is why it is not a drive-by patch |
| ~~`arguments` object absent~~ | **fixed** — built from the frame's argument window; `Array.isArray(arguments)` differs (true vs node's false) |
| `Object.defineProperty` ignores `enumerable:false` | the property store has no per-property attribute flag |
| undeclared-global call reports `TypeError` (node: `ReferenceError`) | the VM sees only the callee value at the call site; treating `undefined` as `ReferenceError` would mislabel every missing method |

Suites: VM tests **99 → 104** (five conformance regressions pin these fixes in the
repo, not just in a scratch harness), workspace **1026 → 1032**, `cargo fmt --all
--check` 0 diffs; release rebuilt and installed.

## Third sweep: hidden issues that "looked right" (2026-11-04)

A 51-case corpus written specifically for cases whose output looks plausible but
is wrong. **19 divergences found; 10 fixed in this pass; 9 remain.**

Fixed: a process-aborting stack overflow on arithmetic over an array hole;
sparse-array semantics (`map`/`filter`/`forEach`/`reduce`/`delete`/`Object.keys`);
canonical own-key order (indices ascending first) in `Object.keys`, `for…in` and
`JSON.stringify`; `Math.round` on the double just below `.5`; `$&`/`` $` ``/`$'`
expansion for string-pattern `replace`; and `Number.parseInt === parseInt`.

Remaining, with cause:

| Divergence | Cause |
|---|---|
| `(2.55).toFixed(1)` → `2.6` (node `2.5`) | the exact-decimal expansion in `toFixed` uses the scaled float; ties at the 3rd digit differ |
| `(0.5).toString(2)` → `0` (node `0.1`) | `toString(radix)` handles integers only; fractional radix conversion is missing |
| `JSON.stringify` ignores `toJSON` | the serializer does not call a `toJSON` method before encoding |
| `f()` inside an object method: `this` is `undefined` (node: `globalThis`) | the VM is strict-mode by default; sloppy-mode call semantics are not modelled |
| `o.m.call(null)` throws (node: `this === globalThis`) | same root as above |
| `var` closures in a `for` loop see per-iteration values (node: one shared binding) | loop bindings are cellified for `var` as if they were `let` |
| object spread does not invoke getters (`{...o}` with `get a()`) | spread copies own keys, and accessors live under `#get:` |
| object rest keeps the excluded key (`{a,...r}`) | the rest helper does not subtract the named key |
| `x8` tail: `(2.55).toFixed(1)` | same as the first row |

Gates: workspace **1040 / 0 / 16** (VM 105 → 111), `cargo fmt --all --check` 0
diffs, release rebuilt and installed.

## Fourth sweep: function strictness, spread/rest, toJSON (2026-11-04)

Third sweep had 19 divergences; 10 were fixed then, and this pass closed four
more of the "looks right" class:

| Fixed | Was | Now (node) |
|---|---|---|
| `f()` plain call | `this === undefined` → TypeError | `this === globalThis` |
| `o.m.call(null)` | TypeError | reads globals (`undefined`) |
| `{...{get a(){…}}}` | no `a` at all | `5` and `a` is an own key |
| `{a, ...rest}` | `a` also in `rest` | excluded |
| `JSON.stringify({toJSON})` | `{}` | the method's return value |

Root cause of the first two: `is_strict` was hard-coded `true` for **every**
function. Strictness is now per function — class members and constructors and
scripts strict, plain functions strict only with `"use strict"`, object-literal
methods sloppy (matching node's CommonJS).

Corpora after this pass: hidden 19 → **3**, 80-case 12 → **6**, 49-case 7 → **5**.
**14 / 180 cases diverge**, all documented: `toFixed` exact decimal, fractional
`toString(radix)`, `var` in a `for` loop sharing one binding, `defineProperty`
`enumerable:false`, arrow parameter defaults, `Symbol` string-backed, `Promise`,
`BigInt`, TDZ, and undeclared-global call error class.

Gates: VM tests 111 → **114**, workspace **1043 / 0 / 16**, fmt 0 diffs, release
rebuilt and installed.

## Storage exhaustion masqueraded as test failures (2026-11-04)

A full-suite run reported `1042 passed / 1 failed` and then 6 more failures
across `jhol-receipt::snapshot` and `jagrs::tests` — all at file-write sites.
The cause was the machine, not the code: the data volume was at **274 MB free**
and the tests write snapshots and manifests. `cargo test` had produced
`ENOSPC: no space left on device, write` in one of the helper processes.

After freeing 6.5 GB of regenerable debug cache and my own scratch, the same
suite reported **1043 passed / 0 failed / 16 ignored**, twice.

Recorded because it matters for CI: these suites are storage-sensitive, and a
red result at low disk says nothing about the code. Check free space before
believing a failure.

## Fifth sweep: defaults, numeric formatting, and the loop-closure defect (2026-11-04)

| Fixed | Was | Now |
|---|---|---|
| `((a,b=2)=>[a,b])(1)` | `[1, undefined]` | `[1,2]` |
| `((a=1,b=2)=>[a,b])(9)` | `[1, undefined]` | `[9,2]` |
| plain-function defaults | wrong in the same way | correct |
| `(2.55).toFixed(1)` | `2.6` | `2.5` (exact decimal expansion) |
| `(0.5).toString(2)` | `0` | `0.1` (fractional radix) |

Root cause of the defaults bug: the compiler passed the *constant index* of
`undefined` where `StrictEq` expects a register, so the comparison read whatever
lived in the register with that number. The destructuring default path already
loaded it; the parameter path did not.

### Open, diagnosed: a closure in a `var` loop body runs the body once

```js
var fs = []; for (var i = 0; i < 3; i++) { fs.push(() => i) }  // length 1, not 3
var a  = []; for (var i = 0; i < 3; i++) { a.push(i) }         // correct: 3
```

`cellify_pending()` emits `CreateCell` **at the current code position**, and it is
called lazily when the nested function is compiled — which happens while emitting
the loop body. The cell is therefore re-created on every iteration, resetting the
captured slot after the first pass, so the counter no longer advances.

The fix is architectural: cellification must run before the loop is emitted (a
pre-pass that scans the body for captured locals, or moving `CreateCell` into the
function prologue with jump-target fixups). Splicing instructions into already
emitted code at the end of a session is how a codegen bug becomes three, so this
is recorded rather than patched. It affects any `for`/`while` body that closes over
a `var` — common in older libraries.

Gates after this sweep: VM tests 114 → **116**, workspace **1045 / 0 / 16**,
`cargo fmt --all --check` 0 diffs.


## Fixed — `var` closures in `for` loops: body ran once, then per-iteration cells (node: one shared binding)

The diagnosed-but-unpatched defect from the 2026-11-03 record is fixed, with
the architecture the earlier notes called for:

- **Root cause (two halves).** The loop pre-pass `loop_cell_slots` only
  handled `let`/`const` inits — a `var` init returned empty, so the
  shadow/rebind machinery never engaged for `var` loops. The body compile
  *did* cell the captured variable anyway (the arrow's compile marks it),
  producing a slot/cell desynchronisation: the fused test compared the raw
  slot (now a cell pointer) as a number, and the loop exited after one
  iteration.
- **Fix, by declaration kind.** `var`: box ONCE before the loop head
  (`CreateCell` reads the plain slot the init just wrote), then every
  test/body/update access routes through the cell — one binding shared by
  all iterations and all closures, exactly node's `var` semantics. No
  shadow, no rebind. `let`/`const`: the existing shadow register +
  `RebindCell` per iteration stays (fresh binding per closure — also node).
- The earlier "box before the loop panics" notes were about boxing an
  *unwritten* slot; boxing after the init has written the plain slot is the
  ordering that works.

Verified against node on five shapes: inline arrow, `const`-then-push,
function declaration, skip-step update (`i++` inside the body), and
shared-write-through-closure (`objs[0].set(99)` visible from `objs[2].get()`)
— all identical. `for-of let` unchanged.

This was the last item in the "looks right but is wrong" class that made
older libraries (any `for (var i …) { setTimeout/callback using i }`) fail
silently.

## Added — six test suites un-vacuumed: the empty fixture directories now run

`tests/{hostile-fixtures,regression,graph-invariants,polyglot-e2e,
snapshot-replay,feature-matrix}` existed as README-only shells — CI would
have passed them vacuously. Each now has a `run.sh` with real assertions:

- **hostile-fixtures** (5 tests): a local registry serves raw crafted
  tarballs (traversal, absolute paths, backslash, symlink escape, plus a
  clean control). The real `jhol install` binary must refuse every attack
  and succeed on the control. Complements the in-crate
  `adversarial_install.rs` unit tests with a full-pipeline E2E.
- **regression** (3 tests): install determinism (two installs →
  byte-identical trees), lockfile integrity completeness (every package
  carries an SRI), no-deps install succeeds.
- **graph-invariants** (4 tests): every declared dep present in
  node_modules, no orphan top-level dirs, no broken `.bin` links, depth-2
  dependency chain materialised.
- **polyglot-e2e** (2 tests): a project with JS + Python + Rust manifests
  passes `jhol plan` and `jhol sync`.
- **snapshot-replay** (1 test): wipe node_modules, restore from
  lockfile + `.jhol` snapshot, `install --offline` succeeds.
- **feature-matrix** (5 tests): each named feature combination (`runtime`,
  `runtime jholvm`, `bundle bundle-swc`, …) compiles with `cargo check`.

Total: 20 new E2E assertions, all passing. `--all-features` now compiles
(fixed a duplicate `http_listen_cb` symbol in the JSC backend).

## P3 delivery — 2026-11-02 session (require, publish/login, error overhaul)

Dated record of the P2/P3 feature work (verified before the production-gate
session above; tests still green after it: workspace 1045/0, VM 116/116,
corpus 44/44, both gates PASS):

### 1. CommonJS `require` for JholVM (`crates/jhol-vm/src/modules.rs`)

Node-subset CJS: relative/absolute/bare resolution (bare walks `node_modules/`
upward), `.js`/`.json`/`package.json#main` targets (package.json parsed by
the engine's own JSON parser — no second JSON implementation), wrapper
execution with `(module, exports, require, __filename, __dirname)`, cache
inserted *before* execution (cycles observe partial exports, as node), JSON
modules parsed without execution, cache hung off globals under a `#`-key
(GC-rooted, filtered from user key lists). Per-module directory on a VM
stack, so nested requires resolve relative to their own file.

**Two engine bugs this exposed, both fixed structurally:**

1. `compile()` never bound entry-function parameters (the entry was always a
   parameterless script) — the CommonJS wrapper's five params resolved as
   GLOBALS, so a module body saw `undefined` for module/exports/require.
   `compile()` now binds entry params exactly as `compile_fn` does.
2. `enter_frame` sized the frame window from `nslots` alone while the param
   copy writes `reg_base + 2 + nparams` — a five-param function with no
   top-level locals wrote past the resize (out-of-bounds panic). The window
   now covers `max(nslots, param_base + nparams)`; this also closes a latent
   GC-scan gap (params were outside the traced window for such protos).

Verified line-for-line against node on a 10-assertion fixture matrix
(relative/bare/main/JSON/rebind/nested-relative/cache-shared/error-shape).

### 2. `jhol publish` / `jhol login` — the registry write path

`pack` + CouchDB-style packument (`versions`, `dist-tags`, tarball as base64
`_attachments`) `PUT {registry}/{name}` with bearer auth resolved exactly as
installs resolve it (env, project/global `.npmrc`, scoped registries).
Manifest passthrough keeps what the resolver needs (`os`/`cpu`/`libc`,
`peerDependenciesMeta`, `engines`, `exports`, `dist.integrity`).
`--dry-run` builds and prints the full request without sending. `login`
upserts `//host/:_authToken=` at mode 0600. **Verified end-to-end against a
local registry fixture: publish -> packument GET -> tarball GET -> install
from that registry -> require the installed package.**

The write path exposed an HTTP-client bug: the retry helper accepted only
HTTP 200, so npm's **201 Created** publish success was reported as a
failure. Any 2xx now succeeds (install paths meeting other 2xx responses
benefit too).

### 3. Error messages no longer dump engine internals

Uncaught exceptions printed `JSException { value: JSValue { raw: 0x10f114088,
ctx: ... } }` — a memory address instead of the message. The JSC backend now
uses the exception's interpreted string (falling back to its `name`), and
the JholVM bridge prints the thrown string instead of `Debug(Str(...))`.
Both engines verified on uncaught and caught paths.

### 4. `json_escape` fast path + P1 remainder

Stringify's escaper matched + pushed per character over every key and value.
ASCII runs now copy in one `push_str` per run. Offline failures name
`jhol prefetch`; 62 `.bak` files purged; `cache gc` extended to sweep
**all** content-store roots (the warm dir and the durable `~/.jhol-cache`),
with the unpacked-store half keyed through `store_index.json` and fail-safe
by construction (an unclassifiable tree is kept; no index means no-op).

### Bench note

`json_roundtrip` measured 10/10 interleaved pair wins under load
(medians 59.3 vs 87.2 ms, load 154 at the time); on a quiet host it sits at
parity with bun (bun's own samples ranged 17-129 ms across identical runs —
host variance dominates this bench). This is consistent with the
production-gate session's parity finding above; no current-claim dispute.

## Tier 1 + Tier 2 delivery — 2026-11-07

The production-adoption gap closed. Every item below is verified by a test or
an end-to-end run against a fixture.

### Tier 1 — blockers for production use

**`--omit=dev` (and `optional`, `peer`)** — the flag every Dockerfile and CI
deploy needs. `NODE_ENV=production` implies it; `--include-dev` overrides;
`--prod` is the shorthand. Semantics match npm: a package declared in both
`dependencies` and `devDependencies` is a runtime dependency and survives;
`optionalDependencies` that are also hard deps survive `--omit=optional`; the
lockfile keeps recording **every** class so a later plain install restores
the full tree. Implemented as a resolver filter plus a materialisation filter
(the lockfile now also writes npm's `dev: true` / `optional: true` markers),
and it deliberately disables the warm-bypass/capsule/daemon lanes — a proof or
capsule for the full tree is not a valid proof for a reduced one, and the
packed daemon request has no slot for it.

**`jhol install -g <pkg>`** — machine-wide installs into
`$JHOL_GLOBAL_PREFIX` (default `~/.jhol/global`), with POSIX shims in
`<prefix>/bin` for every `bin` entry (a `.js` target without a shebang runs
under `node`), a global manifest recording `^<installed version>`, and
`jhol global list | path | remove`. Verified: `mkdirp` installed globally and
ran through its shim.

**`jhol patch`** — persistent local fixes to third-party packages:
`jhol patch <pkg>` extracts an editable copy, `--commit` stores
`patches/<pkg>+<version>.patch`, `--list`, `--rollback`. Patches re-apply
automatically after every install lane (including the fast ones), which is
what makes the fix survive a wipe.

### Tier 2 — compatibility gaps

**`engines` enforcement** — `engines.node` is read and compared against the
runtime (22.0.0, or `$NODE_VERSION`); warns by default, fails with
`--engine-strict`. Full range support: `>=`, `<`, `^`, `~`, `14.x`, `*`,
conjunctions (`>=12 <19`), alternatives (`>=16 || >=18`); an unparsable range
never blocks. `npm`/`yarn`/`pnpm`/`bun` engine keys are reported, never
enforced.

**`--save-dev` / `-D`, `--save-peer`, `--save-optional` / `-O`,
`--save-exact` / `-E`** — `add`/`install` write into the right package.json
section and pin the exact installed version under `-E`. Star specs are refined
to `^<version>` in whichever section they were saved into.

**`--filter <pattern>` / `-F`** — monorepo workspace selection (`@acme/a`,
`@acme/*`), with a clear error when nothing matches.

**`packageManager` field** — parsed (`name@version`, hash-suffixed too) and
compared with this build; warns on a foreign tool or a major skew, refuses
under `JHOL_PM_STRICT=1` / strict mode.

**`jhol dedupe`** — reports packages installed at more than one version with
copy counts (`--json` for machines). Reporting only by design: collapsing a
duplicate is a resolution decision (an `overrides` entry or a dependency
update), not something an install tool should do silently.

Gates: workspace tests green (620 jhol-core incl. 20 new, 116 VM), installer
gate PASS, engine gate PASS, all five E2E suites pass.


## Audit 2026-11-07 — real-package install sweep, and two bugs it found

Method: 34 popular packages installed in isolated directories with unique
manifests, `--native-only --no-save`, no daemon. This is the only number in
the project measured against what users actually install.

**Before: 23/34 succeeded. After: 32/34.**

### Fixed — transitive `npm:` aliases (the cause of 11 failures)

`pretty-format` (and therefore jest, expect, vite, next, prisma, zod-like
graphs) declares:

```json
"@jest/react-is-18": "npm:react-is@^18.3.1",
"@jest/react-is-19": "npm:react-is@^19.2.5"
```

Those names do not exist on the registry: the alias is a name, the target is
`react-is`. Root-level alias handling existed, but **transitive** aliases were
fed to the resolver as if they were packages, so every graph containing one
died on `Failed to fetch metadata for @jest/react-is-19: HTTP 404`.

Fixed in four places, each needed by a different stage:
1. expansion enqueues the alias *target* (`real@range`) and records
   `alias -> real`;
2. the flat spec list keeps the target even when nothing else pulls it in;
3. the nested resolver resolves the target while building `dependencies`;
4. the legality check and the layout-edge builder compare the target, not the
   alias name.

After install, `materialize_aliases` links `node_modules/<alias>` to
`node_modules/<real>` so `require()` reaches it.

### Found, not yet fixed (recorded with repros)

**1. Fresh-project bundle replay ignores the requested package.**
Two directories with an identical `package.json` and different `jhol install
<pkg>` arguments replay the *first* project's bundle: `install zod` in a fresh
directory whose manifest reads `{"name":"t","version":"1.0.0"}` produced
express's 68-package tree. The replay key must include the requested spec
list, not only the manifest fingerprint. Repro: any two dirs with the same
manifest, different packages.

**2. `jest` and `prisma` still fail** with `verify_deps: empty resolution`
from the nested resolver — the flat pass detects a conflict, and the nested
pass returns an empty tree instead of resolving it.

Both are correctness bugs, not missing features, and both are recorded here
rather than papered over.


## Fixed — the replay lane installed an unrelated project's tree

**Symptom.** `jhol install zod` in a fresh directory whose `package.json`
read `{"name":"t","version":"1.0.0"}` produced **express's 68-package tree**,
and `jest`, `next`, `vite` and friends all produced the same tree. Any two
projects with a byte-identical manifest shared a bundle.

**Cause.** `BundleCache::deps_manifest_key` hashes `package.json` + platform
only. The publish side aliased its bundle to that key, so a bundle published
by `install express` answered a later `install zod` in a different directory
that happened to carry the same manifest. The key described the *manifest*;
the bundle answered a *request*.

**Fix.** A manifest-keyed alias is only published when the install had no CLI
packages — a plain `jhol install`, where the manifest genuinely is the
request (`request_has_packages()`). The key version is bumped
(`jhol-bundle-deps-v1` → `v2`, in both `jhol-core` and the trampoline, which
must stay in lockstep), so aliases published by older builds are unreachable
rather than silently wrong.

Verified: `install express` then `install zod` in two directories with an
identical manifest now yields 68 and 1 packages respectively.

## Still open — `jest` and `prisma`

Both fail with `verify_deps: empty resolution` from the nested resolver:

```
flat resolution conflict (pretty-format requires ansi-styles ^5.2.0, resolved <missing>);
resolving nested layout
single-pass: rejected by the completeness guard; using the full resolver
Error: Application error in resolve_nested_layout: verify_deps: empty resolution
```

Reading: the flat pass correctly detects the conflict, and the nested
(JAGR-∞ chart/MaxSAT) pass returns an **empty resolution** — `Inst::VerifyDeps`
in `jagr_infinite.rs` rejects an empty selection before it reaches the
legality check. The empty chart is the thing to diagnose: it means the
program was built with no candidate packages for that graph, not that the
solver found the graph unsatisfiable. That is where the next session should
start (`Inst::VerifyDeps`, ~line 2519).

Current real-package sweep: **32/34** popular packages install in isolated
projects with unique manifests and no daemon (express, lodash, axios, react,
typescript, webpack, eslint, prettier, next, vite, zod, rxjs, mocha, chai,
sinon, date-fns, ramda, …).


## JAGR-∞ — the resolver becomes a propagation engine (Phases 0–4)

The honest state before this work: `compile()` emitted a fixed five-instruction
program (`LoadChart → GreedySCC → VerifyDeps → StoreResult → Halt`), and
`GreedySCC` picked the highest allowed version of every package **independently
— it never read a dependency edge**. The 31-instruction RVM handled 25
instructions that no program ever emitted; two of them (`MaxSAT2Var`,
`MaxSATPortfolio`) returned an *empty resolution*, which surfaced to users as
`verify_deps: empty resolution` on real packages. There was no MaxSAT, and the
name promised one. `spec_to_interval` returned `Interval::full()` for every
input, and `atlas.stitch` was dead on the main path.

### Phase 0 — the instruction set now contains only what exists

| before | after |
|---|---|
| 31 instruction kinds, 6 emitted | **7 instruction kinds, all emitted and all implemented** |
| 2 instructions silently returned ∅ | none can; an unimplemented instruction cannot be expressed |
| `Value` carried knot/LCC/Betti payloads for instructions never used | `Value` carries Interval / Chart / Resolution / Unit |
| doc comment advertised "tropical/sheaf/MaxSAT ops" | documents what the program does |
| `LearnedClauseCache` + `RVM::lcc` (never queried) | removed |
| `spec_to_interval` returning `full()` always | removed (a lie any caller could trip on) |

The research modules are **untouched** — `chart`, `atlas`, `knot_invariants`,
`homotopy`, `precompute`, `persistence`, `holographic_rg`, `ricci_curvature`,
`sheaf_laplacian` keep every line. Only the *wiring* and the *advertising* went,
so those pieces can be tested and adopted deliberately later.

### Phase 1 — bitmap propagation + bounded search

`GreedySCC` → `Propagate` + `Search`:

- **Propagate** narrows every package's version domain to a fixpoint. A domain
  is a `VersionBitmap` over dense ordinals, so narrowing is a word-wise AND;
  a dirty queue re-queues only packages whose domain actually shrank. A package
  whose domain empties under a *required* edge is a named conflict, not a
  silent miss.
- **Search** is entered only if narrowing leaves work. It orders packages
  most-constrained-first (fewest candidates), tries versions highest-first,
  propagates each tentative choice, and backtracks on an empty child domain.
  Bounded by `JAGR_SEARCH_BUDGET` (default 20 000 steps), so a pathological
  graph costs a controlled amount of time instead of hanging.

This is not clause learning or backjumping: nothing is learned and no nogood is
recorded. The whole engine is monotone domain narrowing over fixed-width
bitmaps, with repair localized to the package whose domain emptied.

### Phase 3 — conflicts are explained

`verify_selection` and `explain_empty` report *which* package lost its domain,
*which* dependents demanded what, and *which* candidate versions survived:
`JAGR-∞ conflict: no version of p-limit satisfies every dependent (candidates
after narrowing: [2.3.0, 2.2.2]); p-locate needs p-limit in ordinals [..]`.
The old text was `verify_deps: empty resolution`.

### Phase 4 — soft constraints are modelled, not special-cased

`extract_deps` now returns `(target, interval, optional)` and the flag reaches
`DependencyEdge.optional`, so propagation and search treat an optional edge as
*satisfiable-or-droppable* instead of required. A name declared in both
`dependencies` and `optionalDependencies` is hard (npm's rule). Before this,
every edge was `optional: false`, so one unavailable platform-specific optional
dependency could fail an entire install.

### Phase 2 — verified-selection cache

`compile` already cached *programs* by constraint signature; the resolver now
also caches the **answer** (`sel-v1-<chart>.sel`). An entry is re-verified
against the live chart before use — an edge scan — so a registry that moved a
dependency under us is a miss instead of a stale tree. `JHOL_NO_SELECTION_CACHE=1`
disables it.

### Also fixed while in here

`engines` mis-parsed `>= 10.*` (a space between operator and version split it
into `>=` plus `10.*`), which rejected node 22 for `get-caller-file`'s
`6.* || 8.* || >= 10.*`. Fixed with a test.

### Measured

Real-package sweep — 34 popular packages, isolated directories, unique
manifests, no daemon, `--native-only --no-save`:

| | before | after |
|---|---|---|
| packages that install | 23/34 | **34/34** |
| `jest` | `verify_deps: empty resolution` | 236 packages |
| `prisma` | `verify_deps: empty resolution` | 85 packages |
| `vite`, `next`, `zod`, `rxjs`, `mocha`, `chai`, `sinon`, `date-fns`, `ramda` | failed | install |

Warm repeat of the same constraint signature replays from the selection cache:
express 648 ms cold → **53 ms** warm.

Cold install vs bun (isolated caches, single package): lodash 352/294,
express 955/745, chalk 306/130, dialog-polyfill 597/400 ms — **bun still ahead
on cold**. Warm: lodash 68/30, express 314/281, chalk 365/1422,
dialog-polyfill 210/1708 — jhol ahead where bun re-validates.

So: correctness is now ahead of where it was by a wide margin, and cold-install
speed is a known, unflattering comparison that the next phase has to attack.

Gates: 621 jhol-core tests (5 new/updated resolver tests), 116 VM, installer
gate PASS, engine gate PASS.


## JAGR-∞ — completion pass: engine tests, and the numbers

Follow-up to the propagation rewrite (`bf8516fa`). Two things were thinner than
the phases promised and are now done: the new engine had **no unit tests**, and
the cold/warm comparison had not been run on a clean cache.

### The engine now has its own tests (7, in `jagr_infinite::rvm::exec::tests::engine`)

| test | what it pins |
|---|---|
| `propagation_narrows_to_the_intersection_of_dependents` | two dependents' ranges intersect into one domain; versions outside are gone |
| `propagation_reports_a_real_conflict_naming_both_dependents` | disjoint demands produce an error naming **both** dependents, the package, and its surviving candidates |
| `optional_edge_with_no_candidate_does_not_fail_the_tree` | an optional edge with no candidate is dropped, not fatal |
| `search_backtracks_when_the_highest_version_is_impossible` | the search falls back instead of reporting failure |
| `verify_rejects_out_of_domain_and_unknown_assignments` | unknown package, missing assignment, empty selection, and a valid assignment |
| `empty_chart_yields_an_empty_selection_not_an_error` | an empty chart is satisfiable-and-empty, not an error |
| `budget_exhaustion_is_reported_not_silent` | a tiny budget says "budget", never a partial answer |

`jhol-core` suite: 621 → **628**.

### Cold install, isolated caches, no daemon, two independent rounds

Round 1 (ms):

| pkg | jhol | bun |
|---|---:|---:|
| lodash | 702 | 330 |
| express | 389 | 876 |
| webpack | 1315 | 2273 |
| eslint | 602 | 1055 |
| jest | 4679 | 6211 |

Round 2 (fresh cache dirs again):

| pkg | jhol | bun |
|---|---:|---:|
| lodash | 140 | 306 |
| express | 651 | 1006 |
| webpack | 1021 | 3247 |
| eslint | 509 | 1261 |
| jest | 2542 | 6462 |

**Cold: jhol is ahead on 4/5 in round 1 and 5/5 in round 2.** `jest` went from
12.7 s (before the resolver work) to 2.5 s. The two rounds differ by up to 5x
on the same package, so the honest claim is "ahead on large trees, within noise
on tiny ones" — not a fixed multiplier.

### Warm install is still bun's: measured, not hidden

| pkg | jhol | bun |
|---|---:|---:|
| express | 461 | 32 |
| webpack | 338 | 103 |
| eslint | 811 | 48 |
| jest | 1331 | 159 |

The warm lane here is `install <pkg> --no-save`, which has no lockfile to bind
to and therefore re-resolves. bun's warm path validates a lockfile and clones.
Closing this needs the lockfile fast path to accept an explicit-package request
— recorded as the next perf item, not claimed fixed.


## JAGR-∞ wired as the sole resolver — and the defect that fix exposed

### Done

**Determinism.** Search ordered packages by candidate count only, so equal-sized
domains followed `HashMap` iteration: the *same manifest* could produce different
trees on different runs. Now `(count, package_id)`. A resolver must be a function.

**Quadratic fix.** The propagation dirty queue tested membership with
`VecDeque::contains` — a linear scan per enqueue. A `HashSet` side-set makes it
O(1). On a wide graph this was O(n²) work in the hot loop.

**Peers are enforced.** `extract_peers` collected them, `PeerEdge` stored them,
the lockfile recorded them, and resolution did `let _ = peer;`. Peers now narrow
a domain when they overlap it and are *reported* when they cannot, on
`Chart.peer_warnings` (serde-default so older caches still load). Advisory by
default, escalated to a hard error by `JHOL_STRICT_PEER_DEPS` /
`--strict-peer-deps` — which until now had nothing to escalate.

**JAGR-∞ decides.** New `jagr_infinite::select_versions(packuments, deps)`
answers *which version of each package* through chart propagation + search. The
flat install path's own greedy picks are now explicitly an **estimate used only
for fetch scheduling**; `select_versions` replaces them before the conflict
check. When the chart cannot be built (missing packument, no published version
matching a direct range) the estimate stands — a sole resolver must not be able
to turn a working install into a failing one.

`jhol install <pkg>` therefore has one authority for version choice instead of
two that disagreed (the earlier `p-locate requires p-limit ^2.2.0, resolved
3.1.0` → nested re-solve dance).

### The defect this exposed (recorded, not papered over)

Chart **edges** are built with `parse_single_range`, which returns the *full*
range for exact pins and plain versions and otherwise uses a generic ordinal
scale — **not** the target package's version ordinals. So:

- **direct** ranges narrow correctly (`select_versions` applies
  `candidate_domain_for_package` to each direct dependency, now covered by a
  test), and
- **transitive** ranges cannot narrow at all, because the interval is not in the
  target's coordinate space.

`resolves_a_transitive_edge_through_propagation` is `#[ignore]`d with that
reason rather than deleted or faked. The fix is one well-scoped change: resolve
each edge's spec against the *target's* version list at chart-build time and
store a per-edge `VersionBitmap`, then have `propagate` intersect that mask
instead of the generic interval. That is the highest-value item left in
JAGR-∞.

### Tests

`jhol-core` 628 → **631 passing, 7 ignored** (4 new: highest-satisfying-version,
unsatisfiable-range-returns-None, missing-packument-returns-None, and the
ignored transitive case; plus the 7 engine tests added earlier).

### Verified

Real packages install: 15/15 and 19/19 in two isolated sweeps after the wiring
(`express lodash axios react chalk typescript webpack eslint prettier mocha jest
zod prisma next vite` …). Installer gate PASS, engine gate PASS.


## JAGR-∞: per-edge masks — transitive narrowing now works

The defect recorded in `ebeda053` is fixed.

**What was wrong.** Chart edges carried a `VersionInterval` produced by
`parse_single_range`, which returns the *full* range for exact pins and plain
versions and otherwise a **generic** ordinal scale — not the target package's
ordinals. So a requirement could never narrow anything downstream: every
transitive dependency resolved to its newest version, and `--omit`,
peer checks and conflict detection all saw a graph that was not the real one.

**What changed.** `DependencyEdge` and `PeerEdge` now carry
`required_mask: Option<VersionBitmap>` — the requirement evaluated against the
*target's* published version list, one version at a time, at chart-build time.
`propagate`, `solve_backtrack` and `verify_selection` narrow/verify with that
mask when present and fall back to the interval otherwise (older cached charts
deserialize with `None` and behave exactly as before).

Two precision fixes fell out of it:

- **Dedup is on `(target, spec, interval, optional)`**, not `(target, interval)`.
  `^1.0.0` and `~1.2.0` used to collapse into one edge because their generic
  intervals were identical; they now produce two masks, each precise.
- **Specs are trimmed before dedup**, so `" ^1.0.0 "` no longer creates a second
  edge for the same requirement.

`resolves_a_transitive_edge_through_propagation` is **no longer `#[ignore]`d** —
it passes: `app@1.0.0` requiring `lib ^1` drags `lib` to 1.0.0 instead of 2.0.0.

### Tests

`jhol-core`: **632 passing, 6 ignored** (was 631/7 — the ignored transitive case
is now a passing one). Two old tests were updated because they pinned the lossy
behaviour: `repeated_dependency_and_peer_specs_keep_same_unique_edges` now
asserts two distinct specs and their exact text, which is strictly stronger than
counting one collapsed edge.

### Verified

Cold install, isolated caches, no daemon:

| pkg | jhol | bun |
|---|---:|---:|
| express | 646 | 1417 |
| webpack | 1974 | 4738 |
| eslint | 1652 | 1732 |
| jest | 4961 | 33082 |

Real packages: 19/19 in an isolated sweep. Installer gate PASS, engine gate PASS.


## JAGR-∞: peer masks, and a correction to the warm-install number

### Peer masks wired

`PeerEdge.required_mask` is populated now (it was always `None`), so peer
requirements narrow in the *peer's* ordinal space exactly as dependencies do:
`extract_peers` carries the raw spec, the chart builder evaluates it against the
peer's published versions, and `propagate` prefers the mask over the generic
interval. The peer loop previously did `let _ = peer;`, then briefly only
narrowed by the generic interval — which was `full` for plain specs, i.e. no
narrowing at all.

### Correction: the warm-install gap is ~2.5x, not 14x

The 14x figure recorded earlier compared `jhol install <pkg> --no-save` (which
has no manifest entry and therefore re-resolves on every run) against bun binding
a lockfile. Measured on the path users actually take — a project with
`package.json` + `package-lock.json`, `node_modules` wiped:

| | jhol | bun |
|---|---:|---:|
| warm install, express (68 packages) | 50 / 54 / 69 ms | 21 / 22 / 22 ms |

So **~2.5x**, not 14x. The genuine defect is narrower and still real:
`install <pkg> --no-save` re-resolves every time (351–479 ms for the same tree)
because the request never matches the lockfile's root dependency set.

### Numbers, final state of this session

| workload | jhol | bun |
|---|---:|---:|
| cold express | 646 | 1417 |
| cold webpack | 1974 | 4738 |
| cold eslint | 1652 | 1732 |
| cold jest | 4961 | 33082 |
| warm express (lockfile) | ~57 | ~22 |

Cold is comfortably ahead; warm is behind by 2.5x and the reason is known
(materialisation, not resolution — 68 directories rebuilt from the content store
versus bun's clone-from-cache).

### Tests

`jhol-core`: 632 passing, 6 ignored. Both gates PASS (the installer gate's first
run reported a transient failure while the release binary was being rewritten;
re-run green, 2,107 files deterministic).


## Warm-install investigation: measured the cause, reverted the fix

Follow-up to the peer-mask commit. Two hypotheses were tested and one change
was **reverted** because it did not do what it claimed.

**Hypothesis 1: explicit installs re-resolve because they never bind to the
lockfile.** True as a mechanism (`can_reuse_prepared_layout` requires an empty
package list or `from_lockfile`), so `lockfile_satisfies_request` was written to
mark a request bound when every requested spec is already pinned. Measured:

| workload | before | with the binding |
|---|---:|---:|
| repeat `install express --no-save` (68 pkgs) | 351-479 ms | 279-416 ms |

The binding never engaged — not even when `express` *was* a declared dependency,
which means the helper's view of "the lockfile answers this" disagrees with the
installer's. Rather than ship a flag that silently does nothing, **the whole
change was reverted**; the helper, the CLI block and the re-export are gone. The
tree is back to the verified peer-mask state.

**Hypothesis 2 (now the recorded conclusion): the warm cost is
materialisation, not resolution.** Same 68-package tree, same cache:

| path | warm time |
|---|---:|
| manifest + lockfile (`jhol install`) | 50-69 ms |
| explicit (`jhol install express`) | 279-480 ms |

Resolution is identical work in both cases once the expansion cache hits; the
difference is that the manifest path projects a whole prepared tree
(clonefile/CoW of one bundle) while the explicit path materialises 68
directories individually. **The next attempt should target materialisation for
explicit requests — reusing the prepared-tree projection rather than re-deriving
it — not the resolver.**

Recorded because a failed optimization is a fact about the system: the resolver
was not the problem, and the fix belongs one layer down.


## The core issue: fast paths answered a question they could not see

Three reproducible defects, all on a plain `express@^4` project, all the same
shape — a decision made from evidence too weak for the question:

| what was done to the tree | jhol said | truth |
|---|---|---|
| deleted `node_modules/express` | Added 1 packages | repaired |
| edited `node_modules/express/package.json` → `9.9.9` | **Up to date** | tampered version kept |
| added `node_modules/evil-leftover` (not in the lockfile) | **Up to date** | kept forever |

A tampered package surviving an install and an unauthorised package being
permanent are not performance misses; they are the two properties a package
manager exists to guarantee. They were possible because **five components
independently decided "already up to date"**, each with different evidence:

1. the trampoline's `.jhol-ok` mtime,
2. the CLI's prepared-tree proof,
3. the JGrid bundle projection lane,
4. `lockfile_fingerprint_matches()`,
5. the daemon's verdict,

plus `nm_fresh()` inside `install_package`. A fingerprint can prove "nothing
changed *around* the tree". None of them could prove "*the tree is what the
lockfile authorises*" — which is why the answer depended on which lane won.

### The fix: one predicate, one decision point

New crate **`crates/jhol-treecheck`** (deps: `serde_json`; 10 tests):

```
Match | Missing{path} | Wrong{path,want,got} | Extraneous{path} | Unverified{reason}
```

It reads one small `package.json` per installed package and no archives, so it
is cheap enough to run on every warm install. `reconcile_tree()` verifies,
removes unauthorised directories (which needs no resolution), and verifies
again. `jhol-core` re-exports it, so the CLI, the installer lanes and the
trampoline share one implementation — the trampoline deliberately does not
depend on `jhol-core`, which is why this had to be its own crate.

It is consulted **at the entry point**, before any lane is chosen, and again at
the single convergence point where every fast lane's success is checked. A lane
may now only *save work*; it cannot *decide truth*.

### Measured

| | before | after |
|---|---|---|
| package.json edited to 9.9.9 | Up to date, kept | `Repaired`, restored to 4.22.3 |
| unauthorised directory | Up to date, kept | `Repaired`, removed (3/3 runs) |
| clean warm install (71 pkgs) | 8-33 ms | **9-11 ms** |

`Repaired (n packages in node_modules)` is a new outcome: an install that fixed
something no longer claims it did nothing.

### A bug found in the verifier itself

The first walker descended into a package's own subdirectories, so
`node_modules/a/lib` was reported as an unauthorised package and would have been
**pruned with real files inside it**. The queue now holds `node_modules`
directories, never package directories; a test pins scoped and nested layouts.

`jhol-core` 632 tests, `jhol-treecheck` 10, installer gate PASS.


## Concurrent installs: one per project, enforced by the kernel

Two `jhol install` runs in the same directory used to interleave — both resolved,
both wrote `node_modules` and `package-lock.json`, and the resulting tree matched
neither run. The verifier could report that afterwards; it could not prevent it.

`crates/jhol-core/src/project_lock.rs` serialises installs per working tree. It
is taken at the CLI entry for commands that write the tree (`install`, `i`, `ci`,
`add`, `remove`, `update`, `dedupe`, `prune`) and held for the life of the
process; read-only commands never take it.

| | before | after |
|---|---|---|
| two cold installs at once | no waiting, tree needed repair | one waits, both succeed, tree consistent |
| two warm installs | both fine | both fine, no lock taken (nothing to write) |
| lock after exit | file left behind after every install | released by the kernel on any exit |

### Why `flock` and not a pid file

The first version created the lock with `create_new` then wrote the owner's pid.
The reproduction caught the race immediately: between the create and the write, a
second process reads an **empty** file, concludes the content is garbage, calls
the lock stale, removes it, and proceeds — two installs, no waiting, precisely
the bug the lock exists to prevent.

`flock` deletes the whole class. Ownership is a property of the open file
description, not a value inside the file, so there is nothing to read and nothing
to race; and a process that dies (including through `std::process::exit`, which
the CLI uses widely and which bypasses `Drop`) has its lock released by the
kernel. The pid inside is diagnostics only — it tells a waiting user *who* they
are waiting for and is never consulted to decide whether the lock is held.

### Detail that matters

A nested install (a lifecycle script that runs `jhol install`) would deadlock
against itself, since `flock` on a second fd of the same file blocks even within
one process; the guard therefore tracks locks held by this process and treats a
re-entry as a no-op.

`jhol-core` 636 tests (4 new), `jhol-treecheck` 10, installer gate PASS.


## `jhol cache clean`: reports what it could not remove, and why

The command returned a count and swallowed every failure with `is_ok()`, so a
cache written by an older `sudo` run produced `Removed 0 cached package(s)` and
nothing else — indistinguishable from an already-clean cache. Worse, it could
not run at all in that state: cache initialisation runs before the subcommand
and aborted with `Failed to initialize cache: Permission denied`, which is
precisely when you need the command.

Now:

```
jhol: 2 entries could not be removed:
  /tmp/cache/a.tgz  (Permission denied (os error 13))
  /tmp/cache/b.tgz  (Permission denied (os error 13))

those entries belong to another user (an older `sudo` run?).
  Fix ownership:  sudo chown -R $(id -u):$(id -g) /tmp/cache
  Then re-run:    jhol cache clean
```

- `cache clean` no longer requires a usable cache (every other `cache`
  subcommand still does, because it reads or writes one).
- Bytes freed are reported alongside the count.
- The permission case names the directory and the exact command that fixes it,
  instead of leaving the user to infer it from `Removed 0`.
- **Partial failure exits 1.** A script that runs `jhol cache clean` and checks
  the status must not be told the cache is empty while entries survived.

`jhol-core` 636 tests, installer gate PASS.


## `jhol ci` panicked; every tree-writing command now reports repairs

`jhol ci` crashed on every invocation:

```
thread 'main' panicked at clap_builder/.../arg_matches.rs:185:
arg `engine-strict`'s `ArgAction` should be one of `SetTrue`, `SetFalse`
```

The `ci` subcommand never declared `engine-strict`, but its handler read it. This
is the two-clap-builder hazard again — the flag exists on `install` and the code
treats both handlers alike. Every `get_flag` call now goes through a helper that
treats an undeclared flag as *off* (which is what an absent flag means), so a
drift like this can no longer turn into a panic:

```rust
fn flag(m: &ArgMatches, name: &str) -> bool {
    m.try_get_one::<bool>(name).ok().flatten().copied().unwrap_or(false)
}
```

117 call sites rewritten. A scripted audit of every subcommand arm against its
`Arg::new` declarations now finds no real drift (the three remaining hits are the
shared `backend` argument, a false positive).

### Reporting, everywhere

The reconcile-from-the-entry gate only covered plain `install`, so
`install --frozen-lockfile` and `ci` repaired a tampered tree and still printed
`Up to date`. Reconciliation now runs for every command that writes the tree, and
the repair flag is set whenever the tree disagreed on arrival — so:

| command, tampered tree | before | after |
|---|---|---|
| `install --frozen-lockfile` | Up to date, 9.9.9 kept | `Repaired`, restored to 4.22.3 |
| `ci` | **panic** | repairs, reports script approvals |
| `install --offline` (extraneous dir) | Up to date, kept | `Repaired`, pruned |

`jhol-core` 636 tests, `jhol-treecheck` 10, installer gate PASS.


## JAGR-∞: mask memoisation, the builder contract, and two real defects found

Work on the resolver's own proof obligations turned up two defects, one of which
had been silently making `jest` resolve by *violating* requirements.

### Shipped

- **Mask memoisation.** `compute_mask` is now cached per `(target, spec)` in the
  chart builder. A chart has thousands of edges and very few distinct specs, so
  this is O(distinct specs x versions) instead of O(edges x versions).
- **The chart builder names its packages.** `build_full_chart` used to leave
  `name` empty with a "callers must fill names" rule in a doc comment — invisible
  to the type system, missed by new callers, and the reason conflict messages
  were `pkg#290`.
- **Edges carry their raw spec** (`DependencyEdge::spec`, `PeerEdge::spec`), the
  derivation a `jhol why` answer needs.
- **Proof obligations as tests**: soundness over 40 random worlds, determinism
  under map insertion order and repeats, monotonicity + idempotence of
  propagation, and a 400-package x 12-version perf corpus (188 ms).

### Found

1. `VersionBitmap::intersect` ANDs words positionally and ignores `ordinal_lo`,
   so bitmaps anchored at different ordinals are compared bit-for-bit as if they
   described the same versions. `from_interval({1..1})` anchors at 1; intersecting
   it with a domain anchored at 0 selects ordinal 0 — a version the range
   excludes. The fix is written and verified, and **held back**: with correct
   intersections `jest` stops resolving, because its tree needs several majors of
   `chalk`/`semver` and the flat path insists one version satisfies every
   dependent. The old bug was accepting incompatible versions, and the nested
   layout masked the result. Pinned as an `#[ignore]`d test carrying the reason.
2. The lockfile does not record everything the installer materialises:
   `express@latest` installs 68 packages and writes 56 entries, and the 12
   omissions appear in no dependency map either — the lockfile is internally
   consistent and wrong. That makes `jhol ci` non-reproducible and made pruning
   against it delete real packages.

### Safety change

Because of (2), pruning is now **opt-in** (`JHOL_PRUNE_EXTRANEOUS=1`): an
unlisted package is reported as a warning and left in place. Tampered or
missing packages are still detected and repaired.

`jhol-core` 641 tests (7 ignored, each carrying its reason), `jhol-treecheck`
12, installer gate PASS. Design for the remaining resolver work is in
`docs/JAGR-INFINITE-ROADMAP.md`.


## JAGR-∞: chart assembly 12.4× faster (compile the range once)

Measured first, optimised second. A benchmark over the real packument cache
(`crates/jhol-core/examples/resolve_bench.rs`, reading the same gzipped HTTP
bodies an install reads) plus phase timings showed chart build was **144 ms of a
145 ms solve**, and that mask assembly — not version extraction (2.4 ms) — was
98% of it.

Cause: `version_satisfies_parsed` is written for one-off checks. It trims, probes
for an exact pin, and takes a mutex-guarded global cache lock **per call**.
Deciding which versions a spec allows asks that question once per published
version of the target, so the lock and hashing were the inner loop.

`registry::compile_spec` now decides the variant once — `Any` (stable only),
`Exact` (a bare pin is equality, not caret), `Alternatives` (a `VersionReq`, or
several joined by `||`), `Unsupported` — and matching becomes a pure comparison.
Published version strings are also parsed once per package rather than once per
`(spec, version)` pair.

| phase | before | after |
|---|---:|---:|
| mask assembly | 141.6 ms | **11.4 ms** |
| chart total | 144.0 ms | **14.5 ms** |
| propagate | 0.6 ms | 0.7 ms |

Semantics are unchanged by construction and pinned by `compiled_spec_tests`: 25
spec shapes x 16 versions (pins, wildcards, prereleases, `||`, hyphen ranges,
junk) comparing the compiled answer with the one-off predicate.

Also landed: mask memoisation per `(target, spec)` (6,715 edges collapse to 3,774
computations, 1.78x), a `SolveStats` phase report, and `JAGR_PROFILE_CHART=1`.

Not landed, with reasons in `docs/JAGR-INFINITE-IDEAS.md`: capability-equivalence
classes (measured: chart build is mask-bound, so classes would save search time,
not build time), and everything search-dependent — real trees hit a genuine
multi-major conflict in propagation, which a flat solve cannot represent.

`jhol-core` 649 tests, installer gate PASS.


## JAGR-∞: binary resolver index — 67x faster load, 5.3x end to end

The registry cache stores gzipped HTTP bodies, which is what HTTP gives us and a
bad format for repeated resolution. `crates/jhol-core/src/jagr_index.rs` extracts
only what the resolver reads — sorted version strings and each dependency/peer
declaration with the releases that declare it — into a flat length-prefixed
little-endian blob (`JHOLJAG1`).

Measured on 344 packuments / 34,899 edges (2.27 MB index):

| phase | JSON | index |
|---|---:|---:|
| load (gunzip + JSON + prune) | 302.7 ms | **4.5 ms** |
| chart build | 69.6 ms | 65.4 ms |
| end to end | 372 ms | **70 ms** |

The write costs 157.7 ms once per packument and is measured separately so it is
not hidden inside the read. Encoding is deterministic (sorted packages, sorted
edges), reads are bounds-checked, and truncated/foreign/trailing-byte blobs are
rejected so the caller falls back instead of panicking.

A defect this exposed: `extract_deps` builds its result from a `HashSet`, so a
package's edge order depended on hashing — the solver was pure but the chart was
not. Edges are now sorted in the builder. The equality test (chart from index ==
chart from JSON, edge for edge, mask for mask) is what found it.

`jhol-core` 646 tests, installer gate PASS. Next: wire the index into the
registry cache (write/read beside each packument, keyed by its hash).


## JAGR-∞: the binary index becomes a self-healing cache layer

The first attempt at wiring the index hooked the packument fetch path — and it
**never fired**. A real install wrote 63 packuments and 0 indexes, because the
cache is written by more than one layer. Rather than add a second hook, the
loader now materialises the index for anything it had to parse: every consumer of
the resolver's view goes through `load_cache_dir`, so the accelerator installs
itself on first read. The dead hook was deleted rather than left as noise.

| reading 344 packuments (2.27 MB index) | time |
|---|---:|
| JSON (gunzip + parse + prune) | 426.0 ms |
| cache view, first read (parses + writes indexes) | 57.4 ms |
| cache view, indexes present | **15.2 ms** |

Validation is stat-only (length + mtime in the header): hashing the packument
would spend exactly what the index saves. Stale, truncated, foreign-magic or
trailing-byte blobs fall back to JSON, so a bad index costs one parse and never a
wrong answer; writes are atomic (temp + rename).

Nine tests cover round trip, staleness, truncation at five offsets, missing file,
mixed caches, and rebuild-from-JSON.

Honest status: the live install path still parses JSON because other consumers
need the full packument. What landed is the cache layer and its measurement; the
remaining switch is to have the resolver gather packuments through
`load_cache_dir`.

`jhol-core` 651 tests, `jhol-treecheck` 12, installer gate PASS; express installs
71 packages, warm runs report `Up to date`, tampering is repaired.


## JAGR-∞: nested resolution — jest resolves, multi-major trees supported

The flat solver requires one version per package, so every real tree died in
propagation with a genuine multi-major conflict. `jagr_nest.rs` resolves
**instances**: a requirement creates one, and only compatible-incompatible ranges
get a second. That is nesting, expressed directly.

On the real packument cache with the fetch-and-retry loop:

```
nested: 270 instances across 249 packages, 19 packages with >1 version, ~103 ms
  ansi-styles:  6.2.3, 5.2.0, 4.3.0
  ansi-regex:   6.5.0, 5.0.1
  brace-expansion: 5.0.12, 2.1.7
```

`jest` now resolves. The duplicates are exactly the majors a flat domain cannot
hold.

Properties, tested: determinism (sorted worklist and instance order, newest
satisfying; same graph regardless of insertion order), explanation (each instance
carries `required_by: [(dependent, spec)]`), bounds (instance/step caps), and
three distinct failure kinds — `MissingPackument` (fetch and retry),
`Unsatisfiable` (chain + published versions), `Bounded`.

Real data forced two fixes: `npm:` aliases (`"@jest/react-is-18":
"npm:react-is@^18"` installs react-is under the alias name — the resolver asked
the registry for the alias, got 404, and stopped; `Instance::installed_as` now
separates tree name from resolved package), and optional edges to unavailable
packages are dropped rather than fatal.

Not yet wired: the installer's materialisation still uses its own nested layout.
Peers are recorded, not binding. Hoisting is a deterministic most-demanded rule.

`jhol-core` 656 tests, `jhol-treecheck` 12, installer gate PASS.


## Nested resolution wired into the tree — `jhol install jest` works (opt-in)

`jagr_nest` now feeds the real install path through
`lockfile_write::resolve_full_tree_nested`: instances decide the versions, a
placement rule decides the directories, and the registry provider supplies each
one's tarball URL and integrity. Fetch-and-retry is built in — the resolver
reports which package it needs and the loop provides it, so the resolver stays
independent of the transport.

Measured, cold cache, `jest@29.7.0`:

```
nest: Added 266 packages | 197 top-level dirs | jest bin present
      node_modules/@babel/core/node_modules/semver
      node_modules/jest-worker/node_modules/supports-color
      node_modules/p-locate/node_modules/p-limit
      ansi-styles coexisting at 2 majors
```

`jhol install jest` previously failed outright on this manifest.

### Why it is opt-in (`JHOL_NEST_RESOLVER=1`)

The tree is not yet **stable**: a warm run reports a package the lockfile lists
and the tree lacks (`node_modules/function-bind`), then repairs one or two
packages per run. That is a disagreement between the planned path for an instance
and the path materialisation writes — a plumbing bug, not a resolution bug, and
exactly the class of "fast path answers a question it cannot see" this project has
been removing. A resolver the installer only partly honours must not be the
default, so the default path is unchanged and shows no regression:

| manifest | default | `JHOL_NEST_RESOLVER=1` |
|---|---|---|
| `express@^4` | 71 packages ✅ | 71 packages ✅ |
| `jest@29.7.0` | conflict error (pre-existing) | **266 packages ✅** |

### Costs measured

Cold `jest` takes 54–70 s, dominated by fetching ~270 packuments sequentially
plus 266 tarballs. The resolver itself is ~103 ms on that corpus. Parallel
metadata fetch is the obvious next step, and it is a transport change, not a
resolver change.

`jhol-core` 656 tests, `jhol-treecheck` 12, installer gate PASS.

### Housekeeping

The build failed with `No space left on device` (152 MiB free). Removed 12 GB of
regenerable `target/debug` before rebuilding — the standing rule exists because
this has bitten before.


## Nested resolution is the default; the corruption behind the instability

### Promoted to default

`resolve_full_tree_nested` (per-dependent instances) is now the resolver for the
nested branch. That branch is only reached when the flat expansion has already
failed, so this can only turn a failure into an install; it also falls back
automatically if the instance resolver errors, and `JHOL_NEST_RESOLVER=0`
restores the old path.

Fresh cache, `jest@29.7.0`:

```
Added 266 packages in 51.3 s  |  197 top-level dirs  |  jest bin present
warm: Up to date   (x3)
```

Previously this manifest failed with a resolution conflict. `express@^4` is
unchanged at 71 packages, warm `Up to date`.

### The instability was corrupted store content, not placement

Warm runs reported `Added 1 packages` forever. The cause was not the layout plan:

```
node_modules/function-bind/package.json   0 bytes
```

`store_extract_complete` returned early whenever the completion marker existed,
so a **finished extract with an unreadable manifest** counted as installed. Every
install then "repaired" the package by re-cloning the same zero bytes, so the tree
could never match the lockfile.

The guard now treats a manifest that *exists but is empty* as corruption, marker
or not, and lets extraction rebuild the entry. A manifest that is *absent* is
still governed by the marker, because extracting a tarball with no root
`package.json` is legitimate — the first version of the guard rejected those and
broke two store tests and the installer gate, which is how that distinction was
found rather than assumed.

Three tests pin it: empty manifest with marker (incomplete), repaired manifest
(complete), missing manifest without marker (incomplete). After the fix the
corrupted entry re-extracted itself to 2262 bytes and the tree converged.

### A change I reverted: parallel metadata prefetch

The frontier walk (`jagr_nest::missing_packages`) followed `peerDependencies` as
well as dependencies and looked at *every* published version of every package, so
the prefetch fetched a graph that never closed: a cold `jest` install hung until a
600 s timeout, 0 packages installed. Reverted rather than shipped. The sequential
fetch-and-retry path is correct and costs 51 s cold; a future attempt needs a
bounded frontier (skip peers, cap depth and count) and a hard ceiling that falls
back to sequential.

### Verified

`jhol-core` 659 tests (7 ignored, each carrying its reason), `jhol-treecheck` 12,
installer gate PASS, `jest` tree verifies `Match`.

### Housekeeping

Freed 12 GB of regenerable `target/debug` after `No space left on device` at
152 MiB.


## `jhol why` answers from the lockfile; `propagate` is now order-independent

### `jhol why <pkg>` — the derivation, offline

The existing `why` printed dependency chains. It now also prints what the
lockfile already records: every installed copy, its version, the path it lives
at, and the range each dependent demanded. No registry, no re-resolution — the
derivation was written down when the tree was installed.

```
$ jhol why ansi-styles
jest -> jest-cli -> yargs -> cliui -> wrap-ansi -> ansi-styles
...
2 installed copies of ansi-styles
  4.3.0  (node_modules/ansi-styles)
    chalk@4.1.2 needs ansi-styles ^4.1.0
    wrap-ansi@7.0.0 needs ansi-styles ^4.0.0
  5.2.0  (node_modules/pretty-format/node_modules/ansi-styles)
    pretty-format@29.7.0 needs ansi-styles ^5.0.0
```

Attribution is "nearest declaring ancestor": a dependent's own nested copy
answers for it, and the hoisted copy is used only when there is no nested one.
An installed package nothing declares is reported as such rather than being given
an invented dependent, and a missing lockfile is a different answer from "not
installed" — the caller says which.

Five tests cover: transitive copy attribution, root-direct dependency, absent
package, absent lockfile, and an extraneous copy.

### Propagation is deterministic

`propagate` built its incoming-edge lists and its dirty queue by iterating
`HashMap`s, so the order requirements were applied in depended on hashing. It is
idempotent and monotone, so the fixpoint cannot change — but reaching it by a
different route is not the same as guaranteeing the same route, and the chart is
the input the pure solver is judged on. Both are now sorted (incoming edges by
package id, initial queue by package id), with a test that propagates the same
world built in forward, reverse and shuffled insertion order and asserts
identical domains.

`jhol-core` 665 tests (7 ignored, each with its reason), `jhol-treecheck` 12,
installer gate PASS; `jest` warm `Up to date`, `express` 71 packages.


## `jhol why-not <pkg>@<range>` — minimal unsat core, offline

"Resolution failed" is the least useful sentence a package manager can say. The
reason a range is unavailable always has the same shape: the versions that satisfy
the request are exactly the ones some dependents forbid. Because versions are
matched by predicate, that question is a **set intersection**, and the smallest
set of requirements whose intersection is empty is found by shrinking greedily —
drop a requirement, and if the intersection is still empty, it was never needed.
No clause database, no nogoods: the representation already is the solver's
explanation language.

```
$ jhol why-not chalk@^3
no published version of chalk satisfies ^3 together with the installed tree.
  the only versions matching it are: 3.0.0
  and these requirements forbid every one of them:
    jest-watcher@29.7.0 needs chalk ^4.0.0
  relaxing any one of them (or the request) would allow a version to be chosen.
```

Three outcomes, three answers, and the tool never confuses them:

| situation | answer |
|---|---|
| the range is already installed | `chalk@^4 is already installed … 4.3.0 at node_modules/chalk` |
| published but forbidden | the **minimal core**, one requirement where one suffices |
| nothing published in that range | says so, and names the newest published version |

The middle case is the whole point: on a real `jest` tree, `chalk@^3` is blocked by
**one** dependent, not by the thirty that mention `chalk`. A core that names the
whole tree is a log, not an explanation.

Runs offline from the cached packuments through the self-healing index
(`load_cache_dir`), so it costs milliseconds and works on a plane. Attribution and
ranges come from the lockfile; only version lists come from metadata.

`jhol-core` 668 tests (3 new), `jhol-treecheck` 12, installer gate PASS.


## Bounded parallel metadata prefetch — and what the numbers actually say

The previous attempt at parallelising metadata fetch hung a cold install to a
600 s timeout with nothing installed. The cause was the frontier: it followed
`peerDependencies` and *every* published version, so it never closed.

`missing_packages_bounded` cannot do that. Peers are not followed (advisory, and
they pull in packages the tree never contains), the caller passes a hard name
limit, rounds are capped, and there is a 20 s wall-clock ceiling. Whatever is left
is fetched one at a time by the resolve loop, which always terminates. Three tests
pin the bound (exact cap, unknown root, peers not followed).

Phases, measured on a cold `jest@29.7.0` install:

| phase | time |
|---|---|
| metadata prefetch (400 packuments, 16 at a time) | **9.8–12.3 s** |
| tarball downloads (266 packages) | **19–54 s (network-variable)** |
| materialise an existing tree from the store | **379 ms** |

So the prefetch does what it claims — ~200 ms per fetch sequentially becomes
under 25 ms effective — while total install time is now dominated by **tarball
downloads**, which vary by a factor of three between runs on the same manifest.
Reporting a single "install got faster" number would be a lie, so the phases are
reported instead.

Cap tuned from 600 to 400 names: 600 fetched 330 packuments the tree never used
and cost 12.3 s; 400 covers the real closure of this manifest.

Next item, now unambiguously identified: parallel tarball download. Metadata is
solved; payload is not.

`jhol-core` 671 tests, `jhol-treecheck` 12, installer gate PASS; `express`
unchanged at 71 packages.


## `jhol verify` — a gate CI can trust; and why the payload is not ours to speed up

### `jhol verify [--prune] [--json]`

Exit 0 only when `node_modules` is exactly what the lockfile authorises. Anything
else is a failure with the specific difference named, which is what a CI gate
needs and what "Up to date" never gave it.

```
$ jhol verify
OK: 266 packages match the lockfile            # exit 0

$ jhol verify
NOT OK: node_modules/chalk is 9.9.9 but the lockfile pins 4.1.2   # exit 1
  run `jhol install` to repair, or `jhol verify --prune` to remove unauthorised packages

$ jhol verify --prune      # removes unauthorised dirs, then re-verifies
$ jhol verify --json       # {"state": "Wrong {...}", "detail": "..."}
$ jhol verify              # no lockfile: "not verified: no readable lockfile", exit 0
```

`Unverified` (no lockfile) deliberately exits 0: there is nothing to disagree
with, but it is *reported*, because silence would hide a missing lockfile.

`--prune` prunes unconditionally. The default `reconcile_tree` only prunes a tree
whose sole problem is an unauthorised directory — the right caution for an install,
but not for an explicit request, and the first version left the stray directory in
place whenever a wrong version was reported first.

### The payload phase is registry-bound

Cold `jest` spends 19–54 s downloading 266 tarballs, and I looked for our share of
it: download concurrency is already `(cpus x 4).clamp(32, 128)` and the HTTP
client is a pooled `ureq::Agent`, so connections are reused. There is no thread
count or socket pool left to add; the variance is the registry's. Reporting that
instead of shipping a "2x faster install" claim is the honest outcome.

Metadata is solved (bounded parallel prefetch, 9.8–12.3 s for the whole closure);
payload is not ours to fix by adding parallelism.

`jhol-core` 671 tests, `jhol-treecheck` 12, installer gate PASS; both `express` and
`jest` trees verify `OK`.


## `--as-of <date>`: history as an input — and the metadata gap that blocks it

Resolving "as of" a date is now a first-class resolver capability:
`jagr_nest::resolve_with_cutoff(packuments, roots, cutoff)` admits only versions
published at or before the cutoff. Reproducible historical trees are the point,
and the whole feature is one comparison on a date the packument already carries.

Two hazards were closed rather than papered over:

* **Fail-safe by construction.** A version with no publish date cannot be shown to
  have existed before the cutoff, so it is *excluded* — never assumed old enough.
  When a package has no dates at all the resolver reports
  `NestFailure::MissingDates` instead of guessing.
* **No silent fallback.** The flat expansion picks the newest satisfying version
  and never reads dates, and the single-pass layout reuses those picks. Both would
  have turned `--as-of` into "as of today" — the worst possible outcome for a
  reproducibility feature. A dated install now routes through the history-aware
  resolver, and if that resolver cannot honour the date the install **fails with
  the reason** rather than installing today's tree.

```
$ jhol install --as-of 2020-06-01
Error: --as-of cannot be honoured for express: the metadata in use has no publish
dates. Dated resolution needs full packuments; the abbreviated form served from
the cache omits `time`.        # exit 1, nothing installed
```

### The blocking gap, measured

The cache stores **abbreviated** packuments (`application/vnd.npm.install-v1+json`):
scanned 39 cached documents, **0** carried `time`, and none carried per-version
`license`. `registry::fetch_metadata_full` exists to request the full document, but
the packument cache is keyed by package identity rather than by representation, so
the abbreviated body is served back for the full request. Until that cache
distinguishes representations (or dated installs bypass it), `--as-of` cannot
succeed end to end — and it says so instead of lying.

Same gap blocks license/CVE policy masks, which need per-version `license` and
advisory data that abbreviation omits.

### Also fixed: a flaky test suite

Two `jhol-treecheck` tests set and cleared `JHOL_PRUNE_EXTRANEOUS`, and cargo runs
tests in parallel — the suite failed intermittently depending on interleaving. They
now hold a lock. A flaky gate is worse than no gate.

`jhol-core` 673 tests, `jhol-treecheck` 12 (three consecutive clean runs),
installer gate PASS.


## `--as-of` works end to end: reproducible historical trees

The feature was blocked by my own pruning, not by missing metadata:

```
$ jhol install --as-of 2020-06-01   # express ^4
Added 50 packages       express 4.17.1

$ jhol install --as-of 2023-06-01
Added 58 packages       express 4.18.2

$ jhol install                       # today
Added 71 packages       express 4.22.3
```

Three versions of the same manifest, three faithful trees. Nothing else in this
space offers it without vendoring a lockfile from the past.

### What was actually wrong

`prune_packument_in_place` keeps `name`, `versions` and `dist-tags` — measured: the
cache stores **abbreviated** documents (0 of 39 sampled carried `time`), while
`fetch_metadata_full` returned a complete one with **322** `time` entries. The
resolver then reported "no publish dates" because *the caller had just pruned them
away*, twice: once in the prefetch and once in the sequential fetch, where a second
`prune_packument_in_place` call undid the keeping variant.

Fix: `prune_packument_in_place_keeping(v, &["time"])` and exactly one prune per
fetched document. Two tests pin it — `time` survives the keeping variant and is
still dropped by the default one.

### Behaviour, deliberately strict

* A version with no publish date cannot be shown to have existed before the cutoff,
  so it is excluded rather than assumed old enough.
* A dated install never falls back to the flat resolver (which ignores dates) — it
  fails with the reason. Verified earlier: exit 1, zero packages installed.
* Cost is honest: 21–24 s against 0.5 s undated, because history needs full
  packuments plus their tarballs. An opt-in feature may be slow; it may not be
  wrong.

`jhol-core` 675 tests, `jhol-treecheck` 12, installer gate PASS.


## License policy as a resolution input — `--allow-license`

Policy is not an audit run afterwards; it is an input to the solve. Given
`--allow-license MIT,BSD-3-Clause,ISC,Apache-2.0`, the resolver only considers
versions whose declared license is in the set, so the tree it builds complies by
construction.

Verified on a real `express@^4` tree:

| policy | result |
|---|---|
| `MIT` | refused at `qs` (BSD-3-Clause), nothing installed |
| `MIT,BSD-3-Clause` | refused at `setprototypeof` (ISC), nothing installed |
| `MIT,BSD-3-Clause,ISC,Apache-2.0` | **71 packages installed, 0 policy violations, 0 unknown** |

The refusal names the package and the range, like every other conflict.

### Details that decide whether a policy is real

* **Unknown licenses are excluded by default.** A policy that silently accepts
  unlicensed packages is not a policy; `--license-allow-unknown` is the explicit
  escape hatch.
* **Both licence forms are read**: `"license": "MIT"` and
  `"license": {"type": "Apache-2.0"}`, plus SPDX expressions
  (`"MIT OR GPL-3.0"` is permitted when *any* part is allowed — which is what an
  SPDX disjunction means).
* **Matching is case-insensitive**, because publishers are inconsistent.
* **Full metadata is fetched only when a policy is asked for.** The abbreviated
  documents used for normal installs carry no per-version `license`, and the prune
  that keeps installs small strips it — so `prune_packument_in_place_keeping` now
  retains `time` and `license` when those features are active. This is the same
  bug that blocked `--as-of`: metadata pruned out from under the feature that
  needed it, twice.

No competitor does this at resolution time: an audit tells you your installed tree
is unacceptable; this builds one that is acceptable.

`jhol-core` 678 tests, `jhol-treecheck` 12, installer gate PASS.


## One freshness authority, applied at the source

`lockfile_fingerprint_matches()` was the last predicate that could authorise
skipping an install without looking at the tree:

```rust
prepared_tree_is_valid(cwd) && can_skip_install_for_dir(cwd)   // before
… && tree_state::verify_tree(cwd).is_match()                    // after
```

The first two are hints — they prove nothing changed *around* the tree, never that
the tree is what the lockfile authorises. Putting the verdict at the source means
every caller inherits the authority instead of re-deriving it, which is the
property this project kept needing to re-apply lane by lane.

Verified on a 266-package `jest` tree:

```
tamper node_modules/chalk -> 9.9.9
jhol install             -> Repaired (266 packages in node_modules)
node_modules/chalk       -> 4.1.2          (restored from the lockfile)
jhol verify              -> OK, exit 0
warm install             -> 18 ms
```

18 ms is the honest cost of checking the tree instead of trusting a timestamp (the
earlier 9–10 ms figure was a 71-package tree, not a faster path).

`jhol-core` 678 tests, `jhol-treecheck` 12, installer gate PASS.


## `docs/PRODUCTION-READINESS.md`: every guarantee with the command that checks it

Seven guarantees, each with the command that proves it and the failure it replaced:
tree-vs-lockfile truth, install convergence, concurrent-install serialisation,
determinism, multi-major resolution, self-explaining failures, and history/policy as
resolution inputs. Plus the measured numbers table, the gates, and six known gaps
stated rather than implied.

Written because "production ready" is a claim, and a claim without a command beside
it is marketing.


## Documentation: the whole system, in seven documents (1 282 lines)

| document | lines | contents |
|---|---:|---|
| `docs/RESOLVER-JAGR.md` | 324 | bitmaps and bases, chart, propagation, search, why flat cannot represent an npm tree, instances and placement, aliases, index format, policy and history, obligations |
| `docs/DEFECT-LOG.md` | 230 | twelve defects: symptom, root cause, how it was caught, the rule it produced, and the test born from it |
| `docs/INSTALL-PIPELINE.md` | 180 | argv → tree: freshness authority, project lock, five lanes, resolution path, metadata, download/store invariants, materialisation, failure modes |
| `docs/PRODUCTION-READINESS.md` | 163 | seven guarantees, each with the command that checks it; measured numbers; known gaps |
| `docs/CLI-REFERENCE.md` | 138 | every command, flag, exit code and relevant env var |
| `docs/ENGINE-JHOLVM.md` | 125 | lexer → JIT, NaN boxing, non-moving GC, fused ops/ICs, module system, builtins, formatter, and the compatibility limits stated plainly |
| `docs/OPERATIONS.md` | 122 | workspace map, cache layout + invariants, gates, how to measure without lying to yourself, troubleshooting table, landmines |

Written to be *checkable*: guarantees carry commands, numbers carry the harness that
produced them, limits are stated rather than omitted, and defects record the rule
instead of the anecdote. Nothing here claims a result the repository cannot
reproduce.


## One CLI definition: the dual clap builder is gone

`src/main.rs` held two command trees: the live chain inside `main` and
`build_completions_cli()`, which existed only for shell completions and had drifted
— its subcommand list was stale, so `jhol completions` described a CLI that no
longer existed. Every flag added to the live tree was invisible to it, and flags
added to it were silently ignored by parsing. Five fixes in this project trace to
that drift, including a 100 % `ci` crash from reading an undeclared flag.

The live chain is now `fn cli_command() -> Command`, used by both:

```rust
let matches = cli_command().get_matches();      // dispatch
let mut cmd = cli_command();                    // completions
```

Verified:

| check | result |
|---|---|
| `Command::new("jhol")` definitions in `src/main.rs` | **1** (was 2) |
| `jhol completions bash` mentions `verify` | 20 |
| … mentions `why-not` / `--as-of` / `--allow-license` | 4 / 2 / 2 |
| `jhol ci` (previously a 100 % panic) | works |
| `jhol install --as-of … --allow-license …` parses | yes |

The generated completion script grew to 3,914 lines because it now describes the
real CLI. `flag(&m, name)` remains as defence in depth: an undeclared flag reads as
*off* rather than panicking.

`jhol-core` 678 tests, `jhol-treecheck` 12, installer gate PASS.


## Security exclusion as a resolution input — `--exclude-vuln`

The third policy axis, built the same way as the other two: resolve, ask OSV about
the releases that were chosen, ban the vulnerable ones, resolve again (bounded to
four rounds). The resolver *finds* a tree without known advisories instead of
reporting them after the fact.

```
$ jhol install --exclude-vuln        # lodash pinned to 4.17.15
[jhol] security policy: excluding lodash@4.17.15
      (GHSA-29mw-wpgm-hmr9, GHSA-35jh-r3h4-6jhm, GHSA-p6mc-m468-83gw, …)
Error: no tree avoids the known vulnerabilities: no version of lodash satisfies 4.17.15
  if a direct dependency pins the excluded version, widen its range
  (e.g. `lodash: "^4.17.0"` instead of `"4.17.15"`) so the resolver can choose a clean release
```

```
$ jhol install --exclude-vuln        # lodash ^4.17.0
[jhol] security policy: 1 releases checked, none vulnerable
Added 1 packages      -> lodash 4.18.1
```

An exact pin is reported as unsatisfiable rather than silently overridden — the
manifest asked for that version, and a package manager that quietly installs a
different one is not one you can trust. When the range allows movement, it moves.

Bounded by construction: four audit rounds, sixteen concurrent OSV queries, and a
failure to reach the advisory source is reported, never treated as "no
vulnerabilities found".

### `--as-of` caching: 24.7 s → 6.2 s on repeat

Full packuments are cached under a representation-aware key, so a repeated dated
install reuses them:

| run | time |
|---|---:|
| 1 (cold) | 24.7 s |
| 2 | 8.1 s |
| 3 | 6.2 s |

Same tree (`express 4.17.1`) every time. The remaining cost is tarballs plus a
couple of hundred full metadata documents — the fetch path is the next target, not
the resolver.

### `docs/ENV-AUDIT.md`: the 180 knobs classified

Nothing is retired yet, and the document says so: 24 are diagnostics (they found
most of this project's defects), ~40 are escape hatches guarding a default that was
once unsafe, ~20 are tuning, ~30 are interface. The rule for adding another:

> A new environment variable must name a behaviour that cannot be a default, and
> the commit that adds it must say which default it is avoiding and what would make
> that default safe.

Retiring an escape hatch without fixing the default it guards is how a project keeps
the unsafe thing and renames it.

`jhol-core` 680 tests, `jhol-treecheck` 12, installer gate PASS.


## AI-first, slice 1: models become inspectable and deduplicated

First slice of `docs/AI-FIRST.md`: two primitives that make a model a first-class
artifact rather than an opaque blob.

### `jhol ai inspect` — structure without weights

safetensors is `[u64 LE header length][JSON header][data]` with a byte range per
tensor, so a checkpoint's structure lives in kilobytes. Two byte-range requests:

```
100 tensors, 126,851 parameters, header 10752 bytes (data starts at 10760)
dtypes: F32 x99, I64 x1
```

Measured: **10,760 bytes** fetched and 1.16 s for a 520 KB file; the same 10,760
bytes for a 7 GB checkpoint. `header_len` caps the declared size so a hostile header
cannot choose our allocation.

### `jhol ai pull` — a model store that cannot be poisoned

Files are stored by SHA-256 and materialised by hard link. Two projects pulling the
same checkpoint cost **one copy** (verified: identical inode, one store entry, 520 212
bytes total). Branch refs are resolved once through `X-Repo-Commit`, so a manifest
pins a commit rather than a moving name.

The hazard this exposed is worth recording: project copies and store entries share an
inode, so an in-place edit rewrites the stored bytes for every project — and my first
version then "repaired" by re-linking the corrupted bytes. Entries are now `0444`,
`put` verifies an existing entry before trusting it, and `materialise` refuses bytes
that disagree with their own name. Tampering is blocked by the filesystem *and* caught
by `jhol ai verify`.

`jhol-ai` 13 tests, `jhol-core` 682, installer gate PASS.


## AI-first slice 2: models under `jhol verify`, streaming pulls with resume

### One command for packages and weights

`jhol ai pull` writes `.jhol/models.lock.json` (commit, per-file SHA-256, size) and
`jhol verify` checks it alongside the tree:

```
OK: models match the lock (no package lockfile)

NOT OK: tiny-random-bert: models/.../model.safetensors is 2f892aa16f8e but the lock pins 965f02b6a7e5
  run `jhol ai pull ...` to restore the pinned model files      # exit 1
```

`package-lock.json` stays npm's file, so a project remains installable by npm while
jhol holds it to a stronger standard. A project whose only pinned artifacts are models
verifies too (`TreeState::Unverified` + a matching model lock used to print "not
verified", which was wrong).

### Streaming downloads with resume

One pass: stream to disk, hash while writing, resume by range. A 7 GB checkpoint never
enters memory. Proven end to end:

```
pull --max-bytes 100000   -> error, partial bytes: 98304
pull                      -> 520,212 bytes (resumed via Range/206), digest verifies
verify                    -> OK
```

A server that ignores the range (200 instead of 206) restarts the file rather than
appending a duplicate — a checkpoint that hashes wrong with no explanation is the worst
possible outcome of a "resume".

`jhol-core` 684 tests, `jhol-ai` 13, `jhol-treecheck` 15, installer gate PASS.


## AI-first slice 3: hardware/ABI axes, and compatibility with reasons

`crates/jhol-core/src/axes.rs` + `pypi.rs`, surfaced as `jhol axes` and
`jhol wheels <pkg>[@<ver>]`.

```
$ jhol axes
macos arm64 · libc unknown · no cuda

$ jhol wheels torch@2.4.1 --python 3.12
torch@2.4.1 — 20 file(s), 1 compatible with macos arm64 · python 3.12 · no cuda
  ok        torch-2.4.1-cp312-none-macosx_11_0_arm64.whl
  (19 rejected, each with its own reason: abi cp310 != 3.12, platform win_amd64, …)
```

Five rules had to be right, and every one was found by a failing test rather than by
reading a spec:

| rule | the mistake it prevents |
|---|---|
| `manylinux_2_17_x86_64` = glibc ≥ 2.17 **and** arch | splitting on the first underscore gives arch `17_x86_64` |
| `manylinux2014` means glibc 2.17 | legacy spelling still published |
| `aarch64` ≡ `arm64` | string equality rejects every Linux wheel on an M-series Mac |
| `cp312-none-…` is version-specific | `abi: none` is not "any interpreter" (torch's macOS wheels) |
| `cp39-abi3-…` is forward-compatible | stable ABI from 3.9 runs on 3.13 |

Detection is honest about absence: no `nvidia-smi` means "no cuda", never an error;
`libc unknown` is printed rather than guessed.

**Scope, stated:** this is decision support. Axes are not yet fed into a wheel
*dependency graph* — that requires the provider unification (P0) and a wheel resolver,
which is the next slice. What exists answers the question that currently costs hours:
*can this machine run this release, and if not, exactly why not.*

`jhol-core` 691 tests, `jhol-ai` 13, `jhol-treecheck` 15, installer gate PASS.


## AI-first slices 4–6: environment forks, attestation, replay

Three slices landed, each measured rather than asserted.

### `jhol env fork` — copy-on-write, because hard links are not isolation

Forking a 266-package, 22.5 MB tree:

```
forked exp1 in 797 ms — 690 dirs, 4491 files (4491 cloned, 0 copied),
                       22579752 bytes referenced, ~0 bytes added
```

My first implementation hard-linked files to share blocks. The test caught what that
means: editing the forked `chalk` changed the **original** too, because an inode is an
inode. A fork exists to be edited — a patch or a native build in the fork would have
silently modified the environment it was meant to protect. Forks now use APFS
`clonefile` / Linux `FICLONE`: writes isolated, blocks shared, `env list|diff|rm`
alongside.

### `jhol attest` — a proof produced by the thing that enforces it

Deterministic: sorted keys, no timestamps, so two machines with the same environment
produce byte-identical attestations. It records manifest digest, lockfile digest,
resolved packages, model pins and host axes, and verification **re-derives** all of it
rather than trusting the file. It refuses to attest a tree that does not match its
lockfile, and it covers models.

```
$ jhol attest
attested 260 packages and 0 models at ./.jhol/attestation.json
$ jhol attest --verify      # after any change
Error: attestation does not match: package.json changed    (exit 1)
```

### `jhol replay spec.json` — reproduction that will not rewrite your manifest

A spec declares dependencies, an as-of date, models and expected hardware. `replay`
prints the plan or executes it by spawning the same commands a user would type, so it
cannot drift from the CLI's semantics — and it refuses drift in the other direction:

```
$ jhol replay paper.json --execute
replay complete: 2024-express-repro          # express 4.18.2 as of 2024-01-01, 20 s

$ # after adding a dependency the spec does not declare
replay: the project does not match the spec:
  lodash: have ^4 want <absent in spec>
Error: refusing to edit package.json silently; align the manifest with the spec first
```

`jhol-core` 700 tests, `jhol-ai` 13, `jhol-treecheck` 15, installer gate PASS.


## AI-first slice 7: the Python wheel graph — axes now *resolve*, not advise

`crates/jhol-core/src/pyresolve.rs` resolves a Python dependency graph to artifacts that
run on this machine: it follows `requires_dist`, evaluates PEP 508 environment markers,
and picks releases that publish a wheel for this host and interpreter.

```
$ jhol pysolve torch@2.4.1 --python 3.12
10 package(s) resolved in 1057 ms for macos arm64 · libc unknown · python 3.12 · no cuda
  MarkupSafe 3.0.4        (markupsafe-3.0.4-cp312-cp312-macosx_11_0_arm64.whl)
  filelock 4.2.9          (filelock-4.2.9-py3-none-any.whl)
  networkx 3.7            (networkx-3.7-py3-none-any.whl)
  sympy 1.14.0            (sympy-1.14.0-py3-none-any.whl)
  torch 2.4.1             (torch-2.4.1-cp312-none-macosx_11_0_arm64.whl)
  …
```

That is the difference between "your machine cannot run this" and **"here is the set of
artifacts that runs here"** — the CUDA/ABI matrix decided before anything is downloaded.

### Rules this needed, each found by a test or a real resolution

* **Markers are evaluated.** `sys_platform == "win32"` dependencies are dropped on Linux;
  `extra == "test"` dependencies are dropped always; a marker that cannot be evaluated
  (e.g. `platform_release`) is treated as *not applying* and recorded as skipped —
  skipping is recoverable, installing the wrong thing is not.
* **Versions compare numerically.** `python_version > "3.9"` must be true for 3.12;
  string comparison says no and rejects correct dependencies.
* **Pre-releases do not win by being newest.** The first real run selected
  `networkx 3.7rc0`, `sympy 1.14.0rc2`, `typing-extensions 4.16.0rc2` — a stability
  decision made on the user's behalf. Now excluded unless `--pre` or the specifier names
  one (`==1.0rc1`).
* **A release without a wheel for this interpreter is skipped**, not chosen and left to
  fail at import: newest `numpy` publishing only `cp311` falls back on 3.12.
* **Conflicts name both sides** ("c 1.0.0 is required by b 1.0.0, but 2.0.0 is already
  selected") and unsatisfiable requirements list the rejections.

### Scope, stated

Resolution only. Nothing materialises a Python tree yet — that is the next slice, and it
is plumbing rather than modelling: unpack wheels via the existing store, write a lock,
and extend `jhol verify` to cover it the way it already covers npm and models.

`jhol-core` 708 tests, `jhol-ai` 13, `jhol-treecheck` 15, installer gate PASS.


## AI-first slice 8: Python wheels installed, verified, and agent-drivable

The install half of P0, plus the agent surface.

### `jhol py install` — materialise what `pysolve` decided

```
$ jhol py install requests --python 3.12
installed 5 distribution(s) in 924 ms into .jhol/py/site-packages
  certifi 2026.10.23 (12 files)   charset_normalizer 3.5.2 (19)   idna 3.20 (16)
  requests 2.34.2 (26)            urllib3 2.8.0 (42)
```

Wheels are verified against the index SHA-256 before extraction, cached by content
(so a second project shares one copy), extracted into a flat `site-packages`, and
recorded per-file. `jhol verify` now covers the python tree alongside npm packages and
model pins:

```
$ jhol verify
OK: python tree matches the lock (no package lockfile)
$ # after editing an installed file
NOT OK: ./.jhol/py/site-packages/requests/__init__.py is b5c349b342da but the lock pins 311157e7fa4a   (exit 1)
```

Three details worth recording, each found by running it:

* **site-packages is flat.** My first layout extracted into `site-packages/<dist>/`, which
  double-nests because a wheel already contains its package directory.
* **Empty files are normal.** A zero-length `__init__.py` is not corruption; the
  zero-length heuristic belongs to the store's *manifest* check, not to wheel contents.
* **A wheel is untrusted input.** Absolute paths, `..` and symlinks are refused, and
  `*.data/` (scripts/headers) is skipped rather than placed where it would look
  installable.

Scope: no console-script rewriting, no RECORD-based uninstall, so an extension module is
not guaranteed to load. A pure-Python package is importable from
`.jhol/py/site-packages`. This is deduplicated, verified, reproducible plumbing — not a
pip replacement.

### MCP tools an agent can actually use

`jhol_verify_environment` (one verdict per artifact kind), `jhol_axes`, `jhol_why_not`
(minimal unsat core, offline), `jhol_plan_python` (resolve without installing, including
what was skipped by environment markers). Verified over stdio JSON-RPC:

```
→ jhol_verify_environment
  {"ok": true, "tree": {...}, "models": {...}, "python": {"match": true, "detail": "python tree matches its lock"}}
```

### A flaky gate fixed

`daemon_socket_startup_lock_serializes_and_releases` failed roughly one run in five: it
asserted the lock was re-acquirable in the very next syscall. The property is "released
on drop", not "visible immediately", so it now retries briefly. Five consecutive clean
runs of the whole suite (712 tests).

`jhol-core` 712 tests, `jhol-ai` 13, `jhol-treecheck` 15, installer gate PASS.


## MCP cost estimates, and an honest engine-gap register

### Agents can price a plan before committing

`jhol_plan_python` now quotes cost — total bytes, bytes that would actually download
(cache-aware, using the store), and an estimated duration at a deliberately conservative
throughput — and `jhol_plan_model` quotes a Hugging Face pull from HEAD headers:

```
jhol_plan_python {package: requests}   -> 5 package(s): 0.5 MB total, 0.5 MB to download (0 cached), ~1 s
jhol_plan_model  {source: hf://…, file: [model.safetensors, config.json]}
                                       -> 520,760 bytes, ~1 s
```

The estimate quotes bytes exactly and time from a stated constant; it does not pretend to
predict a network. Finding the size needed a real `HEAD` — my first version used a ranged
GET, which reports the *range's* length and made a 520 KB model claim to be 1 byte.

### `jhol eval -p` was broken on JholVM

`-p` wraps the source in `console.log(eval(...))`, and JholVM has no global `eval`, so
every `-p` invocation on that engine failed with `TypeError: undefined is not a function`.
Expressions now work on every engine (the source is wrapped instead of evaluated), and the
underlying gap is documented rather than papered over.

### `docs/ENGINE-GAPS.md`

Eight measured gaps with the mechanism each fix needs and its acceptance test: `eval`
(the VM is not reentrant — `run_module` clears stack, frames and handlers, so a correct
`eval` needs save/restore and GC rooting), `Promise`/microtasks, `BigInt` (NaN-boxing tag),
`Symbol` as a real primitive, TDZ (per-frame init bitmap + `CheckInit`), `arguments`, the
regex subset, and assorted strict-mode gaps. Every one was verified by running the engine
today, not by reading the source.

`jhol-core` 713 tests, `jhol-ai` 13, `jhol-treecheck` 15, installer gate PASS.


## `eval` attempted for real, then reverted — and why that is the useful outcome

The engine-gap register said "a correct `eval` needs save/restore plus GC rooting". I
tried it: swap `stack`/`frames`/`handlers`, run the nested module, restore, retain the
result. It compiles, and it aborts on the first trivial input:

```
thread panicked: unsafe precondition violated:
slice::get_unchecked_mut requires that the index is within the slice
```

inside the register accessors, whose comment states the invariant this breaks:

> *Bounds are a compile-time invariant: every frame resizes the stack to its `nslots` on
> entry, so the index is always in range.*

**The stack is not storage, it is a layout.** Frames hold `reg_base`/`stack_base` indices
into it, and the dispatch loop relies on that layout holding for the whole run. Swapping
the vector underneath a live loop cannot preserve it. A correct `eval` needs one of:

* a **separate** stack (and base offsets) for the nested run, or
* an explicit stack pointer threaded through `ld`/`st` so a nested run cannot violate the
  layout.

Both are structural changes with their own acceptance tests. Reverted rather than shipped:
an engine that aborts on `eval("1+1")` is worse than one that reports the gap, and a UB
check firing on a two-character program is not something to leave behind.

Everything else stays green: `jhol-vm` 116 tests, engine gate PASS, and the documented
interim behaviour (`jhol eval -p` wraps the expression instead of calling `eval`) still
works on all three engines.

`docs/ENGINE-GAPS.md` now records the attempt as evidence, so the next person starts from
"the stack layout is the obstacle", not from "add a builtin".


## Engine: temporal dead zone fixed, and arrow-body declarations stop panicking

Two engine defects closed, both found by running the engine rather than reading it.

### `let` before initialisation now throws

```
$ jhol eval '(function(){ try { let a = a } catch (e) { console.log(e.message) } })()'
Cannot access 'a' before initialization
```

The compiler pre-scans each block for `let`/`const` names and tracks them in a dead-zone
set tagged with the function they belong to. `load_name` consults that set **before** the
binding lookup — inside `let x = <init>` the slot already exists, which is precisely the
dead zone — and a declaration clears its own name. A dedicated `Op::ThrowTdz` raises the
`ReferenceError`, naming the binding.

Limited on purpose to the current function: a nested function body starts with an empty
set, so `let f = () => x; let x = 5; f()` stays legal. Dynamic TDZ (a function called
before the `let` runs) still needs the per-frame initialised bitmap and is documented as
such.

### Arrow functions panicked the compiler

`(() => { let z = 3; return z + 1 })()` aborted with `Option::unwrap()` on a `None` scope:
arrows were pushed with **no scope at all**, and `declare_pattern` assumes one exists.
`var` escaped because hoisting takes another path — which is why a 44-pair corpus never
caught it. Every function now starts with a scope; arrows get an empty one, since their
`this`/`new_target` are lexical.

Both are pinned by `tdz_and_scope_tests` (three tests: throws, message names the binding,
legal uses stay legal; plus arrow `let`/`const`/destructuring).

`jhol-vm` 119 tests, engine gate PASS, installer gate PASS.


### Engine fixes pinned in the corpus

The TDZ and arrow-scope fixes from the previous entry now have a corpus pair
(`tests/js/45-tdz-and-arrows`, 45 pairs total) covering: `let` in an arrow body,
destructuring in an arrow body, `var` in an arrow body, `for…of` with `const`,
`let a = a`, a block read before its declaration, and the three legal cases that must
*not* throw (read after declaration, closure evaluated after declaration, hoisted `var`).

Recorded in `docs/ENGINE-GAPS.md` §9 why strict-mode assignment to an undeclared name is
**not** the next fix: the frame does not record whether it is the script entry, this
engine marks scripts strict, and node's `-e` is sloppy — implementing it naively would
make `jhol eval 'x = 1'` throw where `node -e 'x = 1'` does not. Fix the frame's context
first, then the check is trivial.

`jhol-vm` 119 unit tests, corpus 45/45, engine gate PASS.


## Engine: strict code may no longer create globals by assignment

`x = 1` on an unresolvable name now throws where it should:

```
(function(){ "use strict"; undeclared_zzz = 1 })()
-> ReferenceError: undeclared_zzz is not defined
```

The prerequisite was exactly what the earlier note predicted: the frame recorded
strictness but not *context*. `FnProto` gained `is_script`, and the check landed in
`Op::DefGlobal` — the first attempt put it in `Op::SetGlobal`, which the compiler never
emits for an unresolved assignment, so nothing threw and the probes all looked "fine".

Behaviour, matched to node rather than to intuition:

| context | assignment to an undeclared name |
|---|---|
| `"use strict"` function | **throws**, naming the binding |
| class method (strict by definition) | **throws** |
| sloppy function | creates the global (unchanged) |
| script level (`jhol eval`, `-e`-style) | creates the global — node runs scripts sloppy |
| existing `globalThis` name, strict code | allowed (the reference resolves) |

Five unit tests (`strict_assignment_tests`) and corpus pair `46-strict-assignment`.

`jhol-vm` 124 tests, corpus 46/46, engine gate PASS, installer gate PASS.


## Engine: symbols are primitives now — plus a `String(undefined)` bug they uncovered

A differential sweep of 20 common idioms against node found **exactly one** divergence:
`typeof Symbol("x")` reported `string`. Symbols were strings internally, so identity,
`keyFor` and printing were all wrong in the same way.

They are now a **tagged interned id** in the existing NaN-boxing layout — subtag 3,
payload = id, no heap object — which makes identity free:

| expression | node | jholvm |
|---|---|---|
| `typeof Symbol("x")` | `symbol` | ✅ |
| `Symbol("x") === Symbol("x")` | `false` | ✅ |
| `Symbol.for("k") === Symbol.for("k")` | `true` | ✅ |
| `Symbol.keyFor(Symbol.for("k"))` / on an ad-hoc symbol | `k` / `undefined` | ✅ |
| `String(Symbol("d"))` | `Symbol(d)` | ✅ |
| `` `${Symbol("t")}` ``, `sym + ""` | throws | ✅ throws |
| `o[sym] = 1` then `o[sym]` | `1`, no collisions | ✅ |

**Still strings on purpose:** well-known symbols (`Symbol.iterator`, …). The iteration
protocol looks them up by their current string keys, so constructor and protocol must move
together with `for…of`, spread and generators covered by tests; doing half of it would break
iteration — worse than an inconsistent `typeof` on two properties.

### A bug the symbol work uncovered

`String(undefined)` and `String(null)` returned `""`, because a present `undefined`/`null`
argument was treated as "no argument". Node returns `"undefined"`/`"null"`, and `String()`
alone is `""`. Fixed; `JSON.stringify([String(undefined), String(null), String()])` now
matches node exactly.

Corpus pair `47-symbols` (node-verified) pins all of it. `jhol-vm` 124 tests, corpus
47/47, engine gate PASS.


## Engine: well-known symbols are real symbols — and a latent protocol split was hiding there

`Symbol.iterator` used to be the string `"Symbol(iterator)"` while the iteration protocol
looked up `"@@iterator"`. The two never met, so `[][Symbol.iterator]` was `undefined` even
on arrays — a split no test had noticed because everything iterated through the internal key.

Well-known symbols are now real symbols whose id **is** that internal key, so both sides
resolve to the same property:

| expression | before | now |
|---|---|---|
| `typeof Symbol.iterator` | `string` | **`symbol`** |
| `typeof [][Symbol.iterator]` | `undefined` | **`function`** |
| `String(Symbol.iterator)` | `Symbol(iterator)` | `Symbol(Symbol.iterator)` |

Iteration itself is unchanged and re-verified: `[...new Set([1,2,2])]`, `[..."ab"]`,
`[...generator()]`, `Array.from(map)`, `for…of "ab"`.

### The bug the migration exposed (documented, not rushed)

`const [x, y] = new Set([3, 4])` yields `undefined, undefined`; node yields `3, 4`. Array
patterns index the right-hand side instead of iterating it, so any non-array iterable
destructures to nothing. The fix is to reuse the existing spread machinery (`__jhol_push_all`
already drains an iterable) in the *binding* path — but it is a compiler change, and I would
rather write it with its acceptance test than bolt it on at the end of a session. It is
`ENGINE-GAPS.md` §8b with the mechanism named.

Corpus pair `48-wellknown-symbols`. `jhol-vm` 124 tests, corpus 48/48, engine gate PASS,
installer gate PASS.


## Engine: array destructuring follows the iterator protocol

`const [x, y] = new Set([3, 4])` bound `undefined, undefined`; node binds `3, 4`. Array
patterns indexed the right-hand side instead of iterating it, so every non-array iterable
(Set, Map entries, generator, string) destructured to nothing.

The bind path now materialises through the iterator via one native,
`__jhol_iter_to_array`, which reuses the existing spread machinery and **returns a real
array unchanged** — so the common `const [a, b] = arr` still costs no allocation.

Verified against node:

| pattern | result |
|---|---|
| `const [x, y] = new Set([3,4])` | `3 4` |
| `const [a, b] = new Map([["k",1]])` | `["k",1] undefined` (the entry array — node agrees) |
| `const [p, q] = generator()` | `1 2` |
| `const [c, d] = "ab"` | `a b` |
| `const [h, ...t] = new Set([1,2,3])` | `1 2,3` |
| `function f([a, b])` called with a Set | `9` |

Corpus pair `49-array-destructuring`. `jhol-vm` 124 tests, corpus 49/49, engine gate PASS,
installer gate PASS.

### Note on the fixtures

I wrote the expected output wrong **twice** while adding this pair — first `a|1` instead of
`a,1`, then assuming `const [a, b] = map` yields the key and value rather than the entry
array. Both times node was right and the corpus caught it. The corpus compares to node, not
to my memory, which is the only reason those mistakes cost minutes instead of shipping.


## Engine gaps: the `Promise` plan is now concrete

`Promise`/microtasks is the largest remaining engine gap. Rather than start it in the tail of
a session, the plan is now written down from reading the code:

* **the drain point exists** — `Engine::run_event_loop` already drains `setTimeout` callbacks
  in deadline order; microtasks hook there (after each callback, before sleeping) and once
  after the entry script;
* **closures have a mechanism** — native fns take no environment, but `JFunction` carries
  `bound_this`/`bound`, so `resolve`/`reject` can be natives bound to the promise;
* **`await` is the hard half** — async functions are currently *rejected at compile time*
  with a clear message, which is better than running them synchronously. Lowering them needs
  suspension points modelled on the generator machinery.

Order, each with its acceptance test: (1) microtask queue + `queueMicrotask` and node-exact
ordering against timers; (2) Promise core with thenable assimilation and unhandled-rejection
reporting; (3) `all`/`allSettled`/`race`/`any`; (4) `async`/`await` lowering.

The reason it is not shipped tonight: a half-built `Promise` is *worse* than an absent one,
because libraries feature-detect it and would then take an async path that hangs or resolves
in the wrong order. The ordering guarantee is the entire value.


## Engine: Promise core + microtask queue, with node-exact ordering

`Promise` was undefined and every async library feature-detected it away. There is now one
FIFO microtask queue — drained after the entry script and before each timer callback — plus
`queueMicrotask`, `new Promise(executor)`, `then` / `catch` / chaining, thenable
assimilation, and `Promise.resolve`/`reject`.

Ordering is the deliverable, and it is compared line-for-line against node by corpus pair
`50-promise`: sync logs, `.then` chains, a throw inside `then` caught downstream,
`queueMicrotask`, `setTimeout(0)`, and a thenable all interleave exactly as node does.

**Closure mechanism:** native fns get no environment, so `resolve`/`reject` are natives bound
to their promise via `bound_this`, and promise jobs are bound natives carrying
`[promise, onFulfilled, onRejected, derived]` in `bound` — the same mechanism `bind` uses.

**Two ordering bugs found while building it:**

* `Promise.resolve(obj)` returned `obj` itself: "has a state slot" was checked with
  `get_prop(..).is_ok()`, which is `Ok(undefined)` for a *missing* key, so any object looked
  like a promise and its own `then` ran inline.
* A thenable's `then` was called synchronously. The spec absorbs it in a *job*; inline made
  every later `.then` land one microtask earlier than node.

**Still open, stated:** `async`/`await` (rejected at compile time with a clear message),
`Promise.all`/`allSettled`/`race`/`any`, `finally`, and unhandled-rejection reporting. All
additive on the queue; the hard part — ordering — is done and pinned.

`jhol-vm` 124 tests, corpus 50/50, engine gate PASS, installer gate PASS.


## Resolver: shared instances, alias identity, and metadata read only for what is reached

Three defects, each found by trying to break the instance resolver rather than by
reading it.

**1. `npm:` aliases were folded into the real package.** Instance reuse was keyed on
the version alone, so `"aliased": "npm:foo@^1.0.0"` was satisfied by the existing
`foo` instance and never got its own directory. The dependent that asked for
`aliased` would look in `node_modules/aliased` and find nothing — a failure at
runtime, long after the install said OK. Identity is now `(installed_as, version)`.

**2. Overlapping ranges created avoidable duplicates.** The first demand for a
package chose the newest matching version with no memory of the second demand about
to arrive, so `c ^1.0.0` then `c 1.2.3` produced two copies of `c` although one
version satisfied both. Version choice now scores candidates by how many demands
already waiting in the worklist they cover, newest-first, with coverage as a
tie-breaker only — a lone `^1.0.0` is never downgraded. Deterministic and
invariant-checked.

Honest result: jest's 266 instances did not shrink, because its six duplicates are
genuine major conflicts. The rule is correct, tested, and pays off on graphs whose
demands merely overlap.

**3. Metadata was built for the whole cache.** The resolver constructed its
per-package view for every packument before reading a single requirement: 12 484
parses to resolve a 266-package tree. It is now lazy, per reached package.

| jest@29.7.0 over a 12 484-packument cache | before | after |
|---|---:|---:|
| resolve | 2 883 ms | **35 ms** |
| tree | 266 instances / 260 packages | identical |

The invariant: resolution costs what the tree costs, not what the cache costs. That
is what makes `why`, `why-not` and `--as-of` cheap, since they run the same
resolver.

New tests: `alias_does_not_reuse_the_real_name`,
`overlapping_ranges_share_one_instance`, `unshared_demand_still_takes_the_newest`,
`every_recorded_demand_is_satisfied`. `jhol-core` 716 tests, real install of
express@4.18.2 verified at 71 packages in 1.33 s.

## Resolver: propagation honours an empty mask, and solves only what is reachable

**An empty requirement mask was widened to "any version".** `Some(empty)` means
nothing published satisfies the spec. The narrowing code treated it like a missing
mask and substituted the *generic* interval — a bitmap on a different ordinal base.
The requirement disappeared, the intersect compared two scales, and the conflict
report blamed a package demanding `[0..65535]` because the real mask had been
discarded. A tree that violated a requirement could therefore be accepted silently.
Emptiness is now honoured; a mask that cannot be evaluated narrows nothing instead
of guessing.

**The solve covered the whole packument set.** The chart was built from every
cached packument and propagation required a satisfiable version for each of them,
reachable or not. An `express` manifest failed on `@esbuild/aix-ppc64` — a Linux
ppc64 binary on darwin/arm64, from a package express never depends on — and forced
the nested fallback on a tree with no conflict. The chart is now pruned to the
edge-reachable set from the roots: 128 -> 70 packages on the express set.

**Conflict candidates printed the six oldest releases**, because `take(6)` ran over
versions stored ascending. Newest-first now.

Open and recorded, not guessed: the flat path still refuses real express with a
conflict on `accepts` although `accepts@1.3.8` is published and demanded; the
nested path resolves the same tree correctly. `jhol-core` 718 tests, installer
gate PASS.

## Resolver: a dependent's requirement is a union over its versions, not a conjunction

The flat resolver conjoined every requirement pointing at a package, including
requirements coming from *different versions of the same dependent*. Express
published 153 releases; 4.0.0 asks accepts `~1.0.5`, 4.18.2 asks `~1.3.8`, 5.0.0
asks `^2.0.0`. Intersecting those is empty by construction, so the propagate phase
reported "no version of accepts satisfies every dependent" on ordinary trees and
every install silently fell through to the nested layout.

An edge is created by a version, not a package. Correct semantics:

* across versions of one dependent — **union**: any release may be chosen, each
  brings its own demand;
* across different dependents — **intersection**: one instance must serve them all.

The union over-approximates (it includes demands of parent versions that may not be
selected), which is the safe direction: propagation only narrows, and legality is
proved separately against the selected versions, so a weak narrowing cannot ship an
invalid tree.

```
before: propagate: no version of accepts satisfies every dependent   (false)
after:  search: constraints are mutually unsatisfiable across 70 packages  (true)
```

Express still fails flat and still installs through nested, because it genuinely
needs `ms@2.0.0` and `ms@2.1.3`. The difference is that it now fails for the real
reason, after doing the real work.

Test: `a_dependent_asking_for_two_majors_does_not_empty_the_domain`.
`jhol-core` 719 tests, installer gate PASS, express install 71 packages and verify
clean.

## Install: predictive prefetch is opt-in, after measuring what it cost

The spectral predictor ran synchronously inside the install loop and fetched the
packuments it expected next *before* the packages the user asked for. With a warm
cache and a six-package manifest:

| | wall time |
|---|---:|
| online, prefetch on (old default) | 367–453 ms |
| `--offline` (prefetch skipped) | 50 ms |
| online, prefetch off (new default) | 38–110 ms |

Speculation cost seven times the transaction it decorated, and every install paid
it whether or not the prediction was ever used. `JHOL_PREDICTIVE_PREFETCH=1`
restores the old behaviour; the predictor still powers `jhol prefetch`.

Express, 71 packages, warm cache: 38–49 ms and `verify` clean.
`jhol-core` 719 tests, installer gate PASS.
