# Engine gaps — what is missing, why, and what fixing it requires

JholVM is a real engine (~32k lines: lexer, parser, compiler, register VM, non-moving
mark-sweep GC, builtins, module loader, formatter, narrow ARM64 JIT). These are its
honest limits, each with the mechanism a fix needs. Nothing here is "TODO"; every entry
says what breaks today and what the change costs.

Measured on the current build (`JHOL_RUNTIME_ENGINE=jholvm`), not assumed.

---

## 1. `eval` is absent — and it is not a ten-line builtin

**Today:** `eval("1+1")` throws `TypeError: undefined is not a function`.

**Why it is hard here:** `Vm::run_module` does

```rust
self.stack.clear(); self.frames.clear(); self.handlers.clear();
```

so the VM is **not reentrant**. A native function that called `eval_source` from inside
the interpreter loop would destroy the caller's stack, frames and exception handlers.

**What a correct `eval` needs:**

1. a reentrant entry: save `stack`, `frames`, `handlers`, and the `protos` length, run the
   nested module, restore all four, and root the result value before restoring (the
   nested run can trigger a collection);
2. a decision on scope: direct `eval` sees the caller's bindings, indirect `eval` runs in
   global scope. Direct eval additionally requires disabling the compiler's slot
   optimisations for the enclosing function.

**Interim behaviour:** `jhol eval -p` wraps the source in parentheses instead of calling
`eval`, so `node -p`-style expression printing works on this engine. Statements with
`-p` produce a syntax error naming the wrapper, which is honest about the gap.

**Acceptance for the fix:** `eval("1+1") === 2`, `eval("{ let x = 1; x }") === 1`, a
throwing eval leaves the caller's `try/catch` working, and the corpus stays green.

---

## 2. No `Promise` / microtask loop

**Today:** `Promise` is undefined; `await`-heavy code cannot run.

**What it needs:** a job queue on the VM, `Promise` with thenable assimilation, and
`queueMicrotask` ordering rules (microtasks drain after each macrotask, before timers).
The timer wheel already exists, so the missing piece is the microtask queue and its drain
points — plus the compiler's `await` lowering if it is not already emitting a suspension
point (it is not).

**Acceptance:** `await` in an async function resolves in order with `queueMicrotask`,
unhandled rejections surface on exit, and the existing timer tests are unaffected.

---

## 3. `BigInt` is absent

**Today:** `10n` is a syntax error.

**Why it is hard here:** values are NaN-boxed into 64 bits. A BigInt needs a heap object
with a sign and a limb vector, plus a new tag in the boxing scheme, plus arithmetic that
promotes and refuses to mix with Number (as the spec requires).

**Acceptance:** `typeof 10n === "bigint"`, `10n + 1n === 11n`, `1n + 1` throws
`TypeError`, and `JSON.stringify` refuses a BigInt exactly as V8 does.

---

## 4. `Symbol` is string-backed

**Today:** `typeof Symbol("x")` reports `string`, and two `Symbol("x")` compare equal.

**What it needs:** a distinct symbol primitive in the boxing scheme (a tag with an interned
description), identity comparison, and `Symbol.for`/`keyFor` over the existing registry.
The registry already exists; the type does not.

**Acceptance:** `typeof Symbol() === "symbol"`, `Symbol() !== Symbol()`,
`Symbol.for("a") === Symbol.for("a")`.

---

## 5. No temporal dead zone

**Today:** `let a = a;` yields `undefined` instead of throwing
`ReferenceError: Cannot access 'a' before initialization`.

**What it needs:** a per-frame "initialised" bitmap for block-scoped slots, a
`CheckInit(slot)` instruction emitted before reads of `let`/`const` slots, and the
compiler marking which slots are block-scoped. It touches the hot dispatch loop, so it
needs the corpus and the JIT/interpreter agreement check to stay green.

**Acceptance:** the canonical cases (`let a = a`, `f(); let x = 1; function f(){ x }`,
`class C { static x = C.y; static y = 1 }`) throw, and hoisted `var`/function bindings do
not.

---

## 6. `arguments` is a real array

**Today:** `arguments instanceof Array` is true; the spec's mapped object is absent.

**What it needs:** an arguments object with `length`, indexed access into the caller's
register window for sloppy-mode parameter aliasing, and `callee` handling. Sloppy-mode
aliasing is the reason this is not just "return an object".

---

## 7. Regex is a subset

**Today:** no lookahead/lookbehind, no backreferences, no named groups, no `s` flag
semantics, no unicode property escapes.

**What it needs:** a real backtracking VM with capture slots (the current matcher is
pattern-directed), or a translation to one. This is the largest single item here.

---

## 8. Assorted

| gap | note |
|---|---|
| undeclared global assignment does not raise `ReferenceError` | needs strict-mode tracking through the compiler |
| per-property enumerable/writable flags are not modelled | one adversarial fixture is an expected failure for this |
| interpreter fallback is ~26× slower on impure loops with captured counters | JIT region rejects some real loops (`GetProp` on a non-virtual slot) |
| `jhol eval -p` with statements | see §1; expressions work on every engine |

---

## The rule these follow

An engine that documents its gaps is usable; one that quietly returns `undefined` is not.
Every entry above has an acceptance test written down **before** the implementation, which
is how the six hardware/wheel rules in `axes.rs` were kept honest.
