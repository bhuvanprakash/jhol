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
