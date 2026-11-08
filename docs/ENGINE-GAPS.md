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

**Attempted and reverted (evidence for the real fix).** Saving `stack`, `frames` and
`handlers`, running the nested module and restoring them aborts on the first trivial
`eval("1 + 1")` with a `slice::get_unchecked_mut` precondition violation inside the
register accessors:

```rust
fn ld(&self, base: usize, r: u32) -> Value {
    // "Bounds are a compile-time invariant: every frame resizes the stack to its
    //  nslots on entry, so the index is always in range."
    unsafe { *self.stack.get_unchecked(base + r as usize) }
}
```

That invariant is the reason a naive swap is unsafe: the stack is not just storage, it is
a layout the dispatch loop relies on, and the frames' `reg_base`/`stack_base` are indices
into it. A correct fix must either (a) run the nested module on a **separate** stack with
its own base offsets, or (b) thread an explicit stack pointer through the accessors so a
nested run cannot violate the layout. Both are structural; neither is a builtin-sized
change. Reverted rather than shipped: an engine that aborts on `eval("1+1")` is worse than
one that says it does not support eval.

**Acceptance for the fix:** `eval("1+1") === 2`, `eval("{ let x = 1; x }") === 1`, a
throwing eval leaves the caller's `try/catch` working, a nested eval inside a running
function leaves its locals intact, and the corpus plus JIT/interpreter agreement stay
green.

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

## 5. ~~No temporal dead zone~~ — **fixed for the statically decidable cases**

**Was:** `let a = a;` yielded `undefined`; `{ q; let q = 1 }` read a global.

**Now:** both throw, naming the binding:

```
Cannot access 'a' before initialization
```

**How:** the compiler pre-scans each block for `let`/`const` names and keeps them in a
dead-zone set tagged with the function they belong to; `load_name` consults that set
*before* the binding lookup (inside `let x = <init>` the slot already exists, which is
exactly the dead zone), and a declaration clears its own name. A dedicated
`Op::ThrowTdz(name-const)` raises the `ReferenceError`.

**Deliberately limited to the current function:** a nested function body starts with its
own set, so `let f = () => x; let x = 5; f()` stays legal — the case a naive textual check
breaks.

**Still not caught (documented, not hidden):** dynamic TDZ — a function called before its
`let` is initialised reading that binding at run time, e.g.

```js
f(); let x = 1; function f(){ return x }   // should throw; returns undefined here
```

That needs the per-frame initialised bitmap + `CheckInit` opcode described in
`ENGINE-GAPS` history, because the information only exists at run time.

**Tests:** `tdz_and_scope_tests` (three: throws, message names the binding, legal uses
stay legal).

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

## 8. Arrow bodies used to panic the compiler — fixed

`(() => { let z = 3 })()` aborted compilation: arrows were pushed with **no scope**, and
`declare_pattern` unwraps `scopes.last()`. `var` escaped it because hoisting took another
path, which is why the bug survived a 44-pair corpus. Every function now starts with a
scope; arrows get an empty one because their `this`/`new_target` are lexical.
Test: `arrow_bodies_can_declare_bindings`.

## 9. Assorted

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
