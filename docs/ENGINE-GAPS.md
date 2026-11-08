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

## 4. ~~`Symbol` is string-backed~~ — **fixed** (one documented exception)

**Was:** `Symbol("x")` returned the string `"Symbol(x)"`, so `typeof` said `string` and two
calls with the same description were equal.

**Now:** a symbol is a **tagged interned id** in the existing NaN-boxing layout — subtag 3,
payload = the id, no heap object. Identity is the id, so:

| expression | result |
|---|---|
| `typeof Symbol("x")` | `"symbol"` — the only divergence in a 20-idiom differential sweep |
| `Symbol("x") === Symbol("x")` | `false` |
| `Symbol.for("k") === Symbol.for("k")` | `true` |
| `Symbol.keyFor(Symbol.for("k"))` | `"k"` (and `undefined` for an unregistered symbol) |
| `String(Symbol("d"))` | `"Symbol(d)"` |
| `` `${Symbol("t")}` `` and `sym + ""` | **throws TypeError**, as the spec requires |
| `o[sym] = 1` | symbol keys keep identity (`PropKey::Sym(id)`), so two same-description symbols do not collide |

**Well-known symbols are now real symbols too.** Each one's id is the *internal* key the
protocols already use (`@@iterator`), so `obj[Symbol.iterator]` and `for…of` finally resolve
to the same property. Before this they were strings spelled `"Symbol(iterator)"` while the
protocol looked for `"@@iterator"`, so `[][Symbol.iterator]` was `undefined` even on arrays —
a latent inconsistency that no test had noticed.

| expression | result |
|---|---|
| `typeof Symbol.iterator` | `"symbol"` |
| `String(Symbol.iterator)` | `"Symbol(Symbol.iterator)"` |
| `typeof [][Symbol.iterator]` | `"function"` (was `undefined`) |
| `[...new Set([1,2,2])]`, `[..."ab"]`, `[...g()]`, `Array.from(map)`, `for…of "ab"` | unchanged and still correct |

Corpus pair `48-wellknown-symbols` pins them.

Corpus pair `47-symbols` (node-verified) pins every row above.

### A bug this uncovered

Writing the symbol tests exposed an older one: `String(undefined)` and `String(null)`
returned `""` because a present `undefined`/`null` argument was treated as "no argument".
Node returns `"undefined"` / `"null"`, and `String()` alone is `""`. Fixed in
`n_string_ctor`.

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

## 9. ~~Strict-mode assignment to an undeclared name~~ — **fixed**

`x = 1` where `x` is unresolvable now throws in strict code:

```
Cannot access ... → ReferenceError: x is not defined
```

The prerequisite turned out to be exactly what the earlier note predicted: the frame had
strictness but not *context*, so `FnProto` gained `is_script`. `Op::DefGlobal` (where an
unresolved assignment actually lands — the first attempt checked `Op::SetGlobal`, which
the compiler does not emit for this) now refuses when the frame is strict **and** not the
script **and** the name is absent from the global object.

Script-level code stays sloppy, because node runs `-e` scripts that way; class bodies and
`"use strict"` functions throw. Assigning to a name that exists on `globalThis` is still
allowed in strict code.

Tests: `strict_assignment_tests` (five: strict function, class method, sloppy function,
script level, existing global) and corpus pair `46-strict-assignment`.

## 8b. Array destructuring does not use the iterator protocol

**Today:** `const [x, y] = new Set([3, 4])` yields `undefined, undefined`; node yields `3, 4`.
Array patterns index the value instead of iterating it, so any non-array iterable (Set, Map,
generator, `arguments`-like) destructures to nothing.

**What it needs:** the bind path for `Target::Array` should materialise the right-hand side
through the iterator when it is not statically an array — the spread machinery already does
this (`__jhol_push_all` drains an iterable into an array), so the fix is to reuse that op in
the *binding* path, not to build a second iteration implementation.

**Acceptance:** destructuring from a Set, a Map entry array, a generator and a string all
match node; destructuring an array stays unchanged and allocation-free (the current path
must still be taken when the right-hand side is a literal array).

Found by the well-known-symbol migration: the same sweep that proved `[...set]` correct
showed `[a, b] = set` wrong, because they use different paths.

## 9b. The note that led here (kept for the record)

`x = 1` where `x` is unresolvable must throw `ReferenceError` in strict code. The VM has
per-function strictness (`proto.is_strict`), so the check itself is a few lines in
`Op::SetGlobal`.

It is not done because the **frame does not record whether it is the script entry**, and
this engine marks `FnCtx::Script` strict. Node's `-e` evaluates scripts as sloppy, so
implementing it naively would make `jhol eval 'x = 1'` throw where `node -e 'x = 1'`
succeeds — a divergence introduced to fix a divergence. The fix needs the frame's context
kind alongside its strictness; then the check is trivial and the acceptance test is
"strict function throws, script does not".

## 10. Assorted

| gap | note |
|---|---|
| ~~undeclared global assignment does not raise `ReferenceError`~~ | **fixed** — §9 |
| per-property enumerable/writable flags are not modelled | one adversarial fixture is an expected failure for this |
| interpreter fallback is ~26× slower on impure loops with captured counters | JIT region rejects some real loops (`GetProp` on a non-virtual slot) |
| `jhol eval -p` with statements | see §1; expressions work on every engine |

---

## The rule these follow

An engine that documents its gaps is usable; one that quietly returns `undefined` is not.
Every entry above has an acceptance test written down **before** the implementation, which
is how the six hardware/wheel rules in `axes.rs` were kept honest.
