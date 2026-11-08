# JholVM — the JavaScript engine

A from-scratch engine living in `crates/jhol-vm` (~32k lines): lexer, parser, AST,
compiler, register VM, garbage collector, builtins, module loader, formatter and a
narrow ARM64 JIT. It is selectable with `JHOL_RUNTIME_ENGINE=jholvm`; the package
manager uses it to run lifecycle scripts, `jhol run`, `eval` and `exec`.

No third-party interpreter is embedded for JholVM itself. The design is a
deliberate compromise: an interpreter plus a *narrow* JIT, with correctness
guaranteed by guards and deopt rather than by approximation.

---

## Pipeline

```
source ─► lexer ─► parser ─► AST ─► compiler ─► bytecode ─► VM
                                                   │
                                        ┌──────────┴──────────┐
                                    interpreter            JIT (counted loops)
```

* `lexer.rs` — tokens with positions; BOM, comments, template literals, regex
  disambiguation; UTF-8 safe.
* `parser.rs` / `ast.rs` — Pratt expression parser, statement grammar, hoisting.
* `compiler.rs` — register allocation, constant folding of literal shapes,
  inline-cache sites, fused operations, string accumulation, dead-value
  elimination, loop cell creation for closured `let`/`const`.
* `bytecode.rs` — the instruction set; each op is small and specialised
  (`Add`, `Idx`, `GetProp`, `NewObjectLit`, `FlushStr`, …).
* `vm.rs` — the dispatch loop, frames, ICs, native fast-path callbacks,
  call-window reuse, panic-free walker.
* `value.rs` — NaN boxing: every value fits in 64 bits, with tagged pointers into
  the arena.
* `arena.rs` / `objects.rs` — a flat arena and a **non-moving mark-sweep** collector
  (44-bit offsets). Non-moving is a requirement: the interpreter holds raw
  references, so compaction would require rewriting every one.

---

## Performance decisions (each measured against node as the oracle)

| decision | why |
|---|---|
| fused bytecode ops | one dispatch for common pair sequences |
| inline caches (store/number/array/string) | property access without a map walk |
| engine-side proto values | avoid rebuilding builtin objects per access |
| `INLINE_PROPS = 6` | the 5-property object (4-literal + one write) would otherwise heap-spill every instance |
| dense index fast path guarded by `len` | an arbitrary `i < 4096` guard made 98 % of 200k-element accesses take the slow path |
| string accumulator op | avoid intermediate strings in loops |
| narrow JIT | counted loops with whitelisted ops; anything else is rejected and interpreted |

The JIT (`jit.rs`) compiles counted loops when every operation is whitelisted,
emitting region code with a full slow path (`region_arith_full`,
`region_property`, `region_binary`) that deopts instead of guessing. Escape
analysis rejects a loop if a temporary can escape — a nested-literal bug once
summed 241 080 instead of 343 200 while the corpus stayed green, which is why the
JIT is bisected against the interpreter on every benchmark run.

---

## Modules

`modules.rs` implements CommonJS for JholVM: node-subset resolution
(`./x`, `x.js`, `x/index.js`, `package.json#main`, `node:` builtins), wrapper
execution with `(module, exports, require, __filename, __dirname)`, a
pre-execution cache so cycles behave like node's, JSON modules, and a cache stored
on the globals object under a `#`-prefixed key.

Two engine defects were found by wiring this up, both fixed: `compile()` never
bound entry-function parameters (so CJS wrappers saw `undefined`), and
`enter_frame` sized the frame from `nslots` alone while the parameter copy wrote
past it.

---

## Builtins

Full `Date`, `WeakMap`/`WeakSet`, `Symbol` (`for`, `keyFor`, well-knowns, via a
static atom registry), `atob`/`btoa`, `crypto.getRandomValues`/`randomUUID`,
`Function.prototype.bind`, `EvalError`/`URIError`, `JSON` with a byte scanner
faithful to node (strict tokens, lone surrogates preserved, `JSON.stringify(undefined)`
→ undefined), `Object.prototype.toString` primitive tags, `Number(undefined)` →
NaN, `toFixed` tie rounding, `Object.is`.

---

## Formatter

`jhol fmt` reformats with comment preservation: 0 width violations, 0
non-idempotent files, and 1 483/1 483 real-world files clean. 42 of 52 reference
files are byte-identical to the reference formatter; the rest differ deliberately,
because matching byte-for-byte would mean reproducing another tool's layout choices.

---

## Testing

* `crates/jhol-vm/tests/js/` — 44 input/expected pairs including large-array
  indices, JSON and builtins.
* `crates/jhol-vm/tests/adv/` — 6 hostile fixtures (one expected-failure).
* `sh crates/jhol-vm/verify.sh` — corpus, unit tests, jit/interpreter agreement.
* `crates/jhol-vm/bench/run_bench.sh --jholvm --bun --runs N` — interleaved A/B,
  min-of-N; absolute numbers move 2–3× with machine load, so both are measured in
  the same seconds.

---

## Limits, stated

* **No Promise/microtask loop** — `await`-heavy code does not run correctly.
* **`BigInt` absent.**
* **`Symbol` is string-backed** — `typeof` reports `string`.
* **No TDZ enforcement.**
* **Per-property enumerable/writable flags are not modelled** (one adversarial
  fixture is an expected failure for this).
* **Undeclared global assignment does not raise ReferenceError.**
* **Regex is a subset** — no lookahead, backreferences or named groups.
* **`arguments` is a real array**, not the mapped object.
* **Interpreter-fallback path is ~26× slower** than the JIT for impure loops with
  captured counters; the JIT rejects some real loops (`GetProp` on a non-virtual
  slot).

These are compatibility limits, not bugs: each is a known, documented gap in
`CHANGELOG.md`.
