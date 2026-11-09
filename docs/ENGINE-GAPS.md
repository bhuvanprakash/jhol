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

## 2. `Promise` core + microtask queue — **shipped**; `async`/`await` still rejected

**Now works:** one FIFO microtask queue drained after the entry script and before every
timer callback; `queueMicrotask`; `new Promise(executor)`; `then` / `catch` / chaining;
thenable assimilation (as a *job*, as the spec requires — see below); `Promise.resolve` /
`Promise.reject`.

```
$ jhol eval 'Promise.resolve(1).then(v => console.log(v))'
1
```

**Ordering is node-exact**, which is the whole point: corpus pair `50-promise` compares a
mixed script — sync logs, `.then` chains, a throw inside `then`, `queueMicrotask`, a
`setTimeout(0)`, and a thenable — line for line against node.

**Closure mechanism:** native functions receive no environment, so `resolve`/`reject` are
natives **bound** to their promise (`bound_this`) and promise jobs are bound natives whose
`bound` array carries `[promise, onFulfilled, onRejected, derived]`. That is the same
mechanism `Function.prototype.bind` uses; no new object kind was needed.

**Two bugs found while building it, both ordering-visible:**

1. `Promise.resolve(obj)` returned the object itself, because "is there a state slot" was
   checked with `get_prop(..).is_ok()` — which is `Ok(undefined)` for a *missing* key. The
   object's own `then` then ran inline.
2. A thenable's `then` was invoked synchronously; the spec absorbs it in a job, and doing it
   inline moved every later `.then` one microtask earlier than node. Fixed by queueing the
   assimilation.

**Combinators shipped** (`all`, `allSettled`, `race`, `any`) plus `finally` and a global
`AggregateError`, on the same queue:
one machine with different settle rules, one pair of handlers, mode selects the rule.
Corpus pair `51-promise-combinators` compares ordering and results line for line with node,
including a `race` against a timer and `finally` on both paths. `52-promise-any` pins
`any` including the empty-input case, which rejects rather than fulfils.

**Unhandled rejections are reported.** A rejected promise with no rejection handler
used to leave the program exiting 0 with no output — a failed async operation looked
like a clean run. The engine now records rejections at settle time, clears the record
when a handler attaches (`catch`, or `then` with a second argument), and fails the run
at the end with `UnhandledPromiseRejection: <reason>`, exit 1 — node's behaviour.

`VmError::Exception` also renders a thrown *string* as its message instead of the
generic `uncaught exception`, which is what made the report readable.

**Still open:** `async`/`await` (rejected at compile time with a clear message),
`finally` waiting for a promise returned by its callback, and property enumerability —
`Error.message` is an own enumerable property here, so `JSON.stringify(new Error("x"))`
prints `{"message":"x"}` where node prints `{}`. The enumerability debt is the same one
noted in `builtins.rs`: there are no per-property enumerable flags.

### `async`/`await` — the plan, and why it was not started today

The pieces that exist: `Op::Yield`, `FnProto::is_generator`, corpus pair 31 exercising
generators, and a correct microtask queue with the whole Promise surface.

The lowering that fits this engine: an `async` function compiles exactly like a
generator (body → `is_generator`), `await e` compiles to `yield e`, and the call site
drives the generator — each yielded value is subscribed with `then(resume, reject)`,
the final return resolves the promise, a throw rejects it.

Two things are missing before that is honest work rather than a demo:

1. **`Generator.prototype.throw`.** Needed for `try { await x } catch (e)` and for
   rejecting a resumed await. The generator object exposes `next` only.
2. **`try`/`catch` across a suspension point.** Our handler table is per-frame and
   unwound on throw; resuming a generator has to restore it at the yield point.

Until both land, `async` stays a compile-time `SyntaxError` naming the gap, which is
better than an engine that silently drops an exception inside an async body.

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

## 8b. ~~Array destructuring does not use the iterator protocol~~ — **fixed**

**Was:** `const [x, y] = new Set([3, 4])` bound `undefined, undefined`; node binds `3, 4`.
Array patterns indexed the right-hand side instead of iterating it, so every non-array
iterable destructured to nothing.

**Now:** the bind path materialises through the iterator protocol first, reusing the spread
machinery (`__jhol_push_all` already drained iterables) via one new native,
`__jhol_iter_to_array`. That helper **returns a real array unchanged**, so the common
`const [a, b] = arr` path still allocates nothing.

Verified against node for: Set, Map entry list, generator, string, rest-from-Set, defaults,
empty array, plain array, and parameter destructuring. Corpus pair
`49-array-destructuring` (the fixture itself was corrected twice — node yields the *entry
array* for a Map, which is not what I assumed).

Found while migrating well-known symbols: the same sweep that proved `[...set]` correct
showed `[a, b] = set` wrong, because the two used different paths. They now share one.

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

### Formatter: a chained call is not broken at 80 columns

`52-promise-any` exposed it: `Promise.any([a, b]).catch((e) => {` is emitted as one
92-column line because the formatter breaks arguments but not a call chain. It was
found by the gate, not by reading the formatter — the fixture had to be reshaped to a
form the formatter respects, and the defect is recorded here rather than hidden.
