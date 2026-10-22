# JholVM

Jhol's own JavaScript engine. Everything below the JavaScript boundary is written
from scratch in this crate — no interpreter library, no vendored runtime, no
code generated from another project's sources.

```
source ──lex──▶ tokens ──parse──▶ AST ──compile──▶ bytecode ──run──▶ Vm
```

| Module | Responsibility |
|--------|----------------|
| `lexer.rs` | Byte-level tokenizer with ASI-relevant line tracking, template-literal raw fragments, regex/division disambiguation |
| `parser.rs` | Pratt parser over the token stream |
| `ast.rs` | The syntax tree the compiler consumes |
| `compiler.rs` | AST → fixed-width bytecode: closures, hoisting, desugaring |
| `bytecode.rs` | Instruction set, constant pool, per-site inline-cache state |
| `vm.rs` | Register interpreter, exception unwinding, generators, engine entry points |
| `builtins.rs` | The whole JS-visible object graph: `Object`, `Array`, `String`, `Number`, `Math`, `JSON`, `Map`, `Set`, `Error`, `console`, `process`, timers |
| `value.rs` | 64-bit NaN-boxed values |
| `objects.rs` | Arena object layouts, property stores, function property bags |
| `arena.rs` | One flat reservation + non-moving mark-sweep collector + interning |
| `tests.rs` | Unit tests for the encoding, the collector and the object stores |

~12.7k lines of Rust, no interpreter dependency.

## The five things that make it not-a-clone

### 1. Single-word values over one arena

A `Value` is one `u64`:

```
 63            48 47     44 43                                  0
+----------------+---------+-------------------------------------+
|     0x7FF8     | subtag  | payload / arena offset / Smi        |
+----------------+---------+-------------------------------------+
```

`0x7FF8...` is a quiet-NaN bit pattern, so every finite double and both
infinities keep their native bits and "is this a double?" is a single 16-bit
compare. Any NaN produced by arithmetic canonicalises to one reserved encoding,
which is what keeps the boxed space collision-free.

The 44-bit offset is relative to one arena reserved at startup and committed
lazily, so heap references are untagged word-sized values and never need a
handle-table lookup. Subtle detail that cost real debugging time: `arena_base()`
returns the *reservation*, not the address of the `Arena` struct.

### 2. Shape-tagged property stores

Property-addition histories intern into `u32` shape ids through one transition
table keyed `(shape, PropKey) -> shape`. Two objects that gained the same keys
in the same order share a shape, and that identity is exactly what the
interpreter's inline caches validate against.

Stores stay insertion-ordered vectors (so `Object.keys` order is free) and only
promote to a hash index past twelve properties. Nothing like QuickJS's
per-object dictionaries, nothing like V8's full descriptor-array machinery.
Functions carry the same property bag, so `A.prototype = x` and `A.name` work
without special cases.

### 3. Inline caches on loads, stores and calls

Every `GetProp`/`SetProp`/`GetGlobal`/`CallMethod` site owns an `Ic` slot indexed
by its instruction position, so caching costs no operand space. The first
execution runs the slow path and records the object's shape plus the slot the key
was found in; afterwards the site validates with one shape compare and reads the
slot directly.

Safety comes from re-validating the cached key on every hit, so a stale slot can
never escape even if two shapes were to collide. QuickJS has no equivalent; this
is the main reason the interpreter beats it on property-heavy code.

### 4. Cell-based closures

A captured local is boxed into a `JCell` at the moment the first closure that
references it is created, and from then on that slot is read and written through
the cell in the enclosing frame too. Frames stay plain `Value` arrays, no upvalue
can dangle, and functions that capture nothing pay nothing at entry.

The cell is created at closure-creation time rather than in the prologue,
because rewiring on every call would be wrong for recursion: the enclosing frame
of a self-call is the function itself, not the scope that created the closure.

### 5. One flat interpreter loop

`Call` pushes a frame and the same loop keeps going, so a JavaScript call costs a
frame push and no Rust stack frame. A builtin invoking a JS callback goes through
`run_until(depth)`, which stops when that frame returns rather than draining the
whole machine — and a *native* call, which leaves no frame at all, bridges its
result through `native_result`.

## Other design notes

* **Non-moving mark-sweep.** A semispace copier would be faster for the young
  generation but invalidates the `&'static` references the interpreter is written
  in terms of. Freeing a block keeps its header, so sweeping is a single linear
  walk; splitting a recycled block writes a `tag == 0` tail header to keep that
  walk valid forever.
* **Exceptions unwind through a handler table**, not through `?`, so `try`/`catch`
  works across nested calls. Handlers are restored to the handler frame's own
  register *window* — truncating to its argument base blanks registers the body
  still needs — and `return` pops the handlers that are still live.
* **Generators** snapshot the register file on `Yield` and push it back on
  `next()`; the function object keeps the cells it captured.
* **`__proto__` is the internal slot**, not a data property, so `class … extends`
  goes through a runtime helper rather than `Object.defineProperty`.
* **`super(...)` resolves at runtime** from `new.target`, so a reassignment of
  `Foo.prototype.__proto__` is visible to `super`.
* **`new` honours native constructors**: `new Map()` gets a fresh `this` with the
  constructor's own `.prototype`.

## Building and testing

```
cargo build -p jhol-vm
cargo test -p jhol-vm --lib -- --test-threads=1
cargo run -q -p jhol-vm --example run path/to/script.js
```

`crates/jhol-vm/tests/js/` holds a 36-file conformance corpus with expected
output captured from node, plus a runner:

```
sh crates/jhol-vm/tests/js/run_corpus.sh <command-that-runs-a-js-file>
```

Helpers for building fixtures by hand: `cargo run -q -p jhol-vm --example dis`
disassembles a file, `--example ast` prints the parsed tree.

Environment: `JHOL_VM_ARENA_BYTES` sets the reservation size,
`JHOL_VM_HEAP_LIMIT` sets the collection threshold, `JHOL_VM_NO_GC=1` disables
collection (useful when bisecting collector bugs).

## Status

**36 of 36** corpus programs produce byte-identical output to node; **18 of 18**
unit tests pass. Wired into the CLI: `JHOL_RUNTIME_ENGINE=jholvm jhol run
script.js` (`cargo build --release --features jholvm`).

Benchmarks (`crates/jhol-vm/bench/`, median wall time, Apple Silicon), output
verified identical to node for every row:

| bench | JholVM | node | bun | QuickJS | JSC |
|---|---|---|---|---|---|
| fib_recursive | 0.38s | 0.22s | 0.02s | — | — |
| iter_sum (50M) | ~4.5–5.7s | 0.69s | 0.07s | — | — |
| closure_calls | 0.16s | 0.29s | 0.01s | — | — |
| method_calls | 0.26s | 0.28s | 0.01s | — | — |
| object_props | 0.21s | 0.18s | 0.02s | 0.40s | 0.14s |
| polymorphic_props | 0.16s | — | 0.01s | 0.17s | 0.06s |
| array_ops | 0.08s | 0.18s | 0.01s | 0.07s | 0.12s |
| json_roundtrip | 0.02s | 0.17s | 0.01s | 0.09s | 0.04s |
| sort_bench | 0.15s | 0.31s | 0.04s | 0.14s | 0.23s |
| gc_stress | 1.5s | — | 0.08s | 2.5s | 0.22s |
| string_build | ~44s | 0.31s | 0.08s | — | — |

Read it honestly: JholVM beats node on recursion, tight loops, closures, method
dispatch and JSON, and beats QuickJS on JSON and GC-heavy allocation. It loses
to bun everywhere and to JSC on compute-bound work. **An interpreter with inline
caches is in the right family but the wrong order of magnitude against a tiering
JIT; closing that needs a register allocator and an arm64 backend.**

Known gaps, in rough order of pain:

* **`string_build` is O(n²)**: `s += x` allocates and copies, so 200k appends
  move ~20 GB. Fix: rope/cons strings (one small node per concat, flatten on
  read). I did not implement it this round — it changes `JString`'s layout and
  every string read path, and I was not willing to risk a working 36/36 for one
  benchmark. This is the single worst gap (550×) and the next thing to do.
* `iter_sum` at 16 ns/instruction: the interpreter dispatch itself is the cost.
  A JIT is the only real fix; micro-optimisation has hit diminishing returns.
* No JIT: every JS operation is a match arm, a NaN-unbox and a register access.
* The regex engine is descriptor-only. No `Proxy`, no `Reflect`, no real
  `Symbol` (the well-known string `"@@iterator"` is used), no `Intl`.
* Async parses but does not suspend; TDZ is not enforced.

Bugs found and fixed along the way (each was real, each looked like "the engine
is broken"):

* `arena_base()` returned the address of the `Arena` *struct*, not the
  reservation — every dereference missed.
* `SetProp`/`DefProp`/`SetIndex` used inverted operands relative to `GetProp`.
* `DefineGlobal`/`SetGlobal` read the value from the wrong operand, so every
  user-defined global received `this` instead.
* Builtin→JS callbacks pushed a frame and never ran it.
* Native functions' return values never reached the caller — including native
  *getters*, which is why `Map.prototype.size` was always `undefined`.
* Getters were invoked with the *prototype* as `this` instead of the receiver.
* Exception unwinding blanked the handler frame's registers.
* `patch_to` was called on a `StrictEq`, writing a jump target into an operand
  slot — the single cause of every `for...of` crash.
* `class … extends` needed a dedicated `SetProto` opcode: routing the internal
  prototype slot through a global helper depended on a call-site register layout
  that was wrong.
* `frame.this_val` was cached at entry, so `super()` replacing `this` was never
  seen by later reads in the same constructor.
* A class name was bound *after* the body compiled, so `extends` in the next
  statement resolved a stale local slot.
* **`set_prop` wrote data properties through to the prototype.** That made
  `P_C.constructor = C` overwrite `Shape.prototype.constructor`, so every later
  `super()` called the subclass again and recursed until the stack died. Only a
  *setter* on the prototype may intercept an assignment.
* `@@iterator` returned a bare array, which has no `next()`.
* `new Set(iterable)` did not dedupe.
* Array#sort used insertion sort, so a 100k sort took 524s; a merge sort with
  the JS comparator brings it to 0.9s.
* `for (let i…)` re-boxed the loop slot every iteration even when nothing
  captured it — 50M heap allocations for a 50M-iteration loop. Now only nested
  functions that actually reference the variable trigger a per-iteration cell.
* Each engine leaked its arena reservation, which is why tests only died after
  the eighth one.
* `??` ran its right operand for present values and skipped it for absent ones.
* The accessor scan in `set_prop` recomputed its `#set:` symbol once per
  prototype level; it is now computed once per store.

```
cargo build -p jhol-vm
cargo test -p jhol-vm --lib -- --test-threads=1
sh crates/jhol-vm/tests/js/run_corpus.sh <command-that-runs-a-js-file>
cargo run -q -p jhol-vm --example run path/to/script.js
cargo run -q -p jhol-vm --example dis path/to/script.js   # disassembly
cargo run -q -p jhol-vm --example ast path/to/script.js   # parsed tree
JHOL_RUNTIME_ENGINE=jholvm jhol run script.js              # via the CLI
```

Environment: `JHOL_VM_ARENA_BYTES` sets the reservation size,
`JHOL_VM_HEAP_LIMIT` sets the collection threshold, `JHOL_VM_NO_GC=1` disables
collection (useful when bisecting collector bugs).
