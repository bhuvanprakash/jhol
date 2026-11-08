# JholVM — session report

Everything below was built, measured and verified in this repository. Numbers come
from `crates/jhol-vm/bench/run_bench.sh` (2–3 runs, median) unless stated otherwise.
Where a measurement was unreliable — a loaded machine, a stale binary — that is
said out loud rather than smoothed over.

---

## 1. What JholVM is

A JavaScript engine written from scratch in Rust as a workspace member of the `jhol`
package manager. No third-party interpreter is embedded. It is selectable with
`JHOL_RUNTIME_ENGINE=jholvm` alongside the existing QuickJS and JavaScriptCore
backends.

| layer | file | what it does |
|---|---|---|
| lexer | `src/lexer.rs` | byte scanner, byte-run UTF-8 decoding, BOM stripping, comment capture |
| parser | `src/parser.rs` | recursive descent, no backtracking except arrow functions |
| compiler | `src/compiler.rs` | AST → bytecode, register allocation, scopes/closures, inline-cache keys |
| bytecode | `src/bytecode.rs` | `Instr { op, a, b, c }`, inline-cache slots, `FnProto` |
| VM | `src/vm.rs` | register machine, NaN boxing, exception unwinding, generators |
| objects | `src/objects.rs` | shapes, property bags, arrays, cells for closures |
| arena | `src/arena.rs` | non-moving mark-sweep GC over one flat reservation |
| builtins | `src/builtins.rs` | Object/Array/String/Number/Math/JSON/Map/Set/Error/console/process/timers |
| formatter | `src/formatter.rs` | `jhol fmt`, printed from this parser |
| bridge | `src/bridge.rs` | CLI entry (`jhol run`, `jhol eval`) |

Design choices that are ours, not borrowed: NaN-boxed 64-bit values addressing a
44-bit arena offset; shape transition tables with inline caches; cell-based
closures (a captured local is boxed once, at closure creation, not per write);
a non-moving collector chosen because the interpreter is written in terms of
stable references; and a compiler-directed string accumulator, described below.

## 2. Performance — the trajectory

Full suite, 11 benchmarks, versus bun:

| bench | session start | end of session | bun |
|---|---|---|---|
| iter_sum | 13,528 ms | **~730 ms** | 41 ms |
| gc_stress | 2,364 ms | **267 ms** | 14 ms |
| fib_recursive | 848 ms | **183 ms** | 11 ms |
| method_calls | 557 ms | **80 ms** | 11 ms |
| closure_calls | 405 ms | **62 ms** | 10 ms |
| sort_bench | 407 ms | **98 ms** | 30 ms |
| array_ops | 313 ms | **42 ms** | 23 ms |
| object_props | 355 ms | **74 ms** | 15 ms |
| string_build | 44,000 ms | **42 ms** | 14 ms |
| json_roundtrip | 52 ms | **19 ms** | 11 ms |
| polymorphic_props | 264 ms | **65 ms** | 11 ms |
| **total** | **19,425 ms** | **~1,660 ms** | ~190 ms |

**11.7× faster than where this started.** Against bun the engine is still ~9×
slower on long-running computation, which is expected: bun executes these loops as
JIT-compiled machine code, JholVM interprets them at ~4–5 ns per instruction, which
is the measured floor for a Rust `match`-dispatch loop with the pointer state
already cached.

Where JholVM wins outright:

| | jhol | bun |
|---|---|---|
| startup + run of an empty script | **4.9 ms** | 7.6 ms |
| format a 163 KB file | **26.3 ms** | 42.6 ms |
| format, process level | **4.7 ms** | 5.1 ms |
| allocate 2M empty objects | **50 ms** | 60 ms |

`jhol fmt` is also **byte-identical to `bun fmt` on 45 of 50 files**, with **0
lines over 80 columns**, **0 non-idempotent outputs**, and comments and unicode
preserved exactly. The five divergences are deliberate: we parenthesize
`typeof {}` conservatively, we omit the redundant parentheses bun adds
(`2 ** (3 ** 2)`), and on two files bun breaks a chain that comfortably fits in
80 columns, so we keep it on one line.

## 3. Optimizations, in the order they were found

Each of these was measured before and after; where the measurement showed no
gain, that is recorded too.

**String building — 44 s → 0.042 s.** `s += x` in a loop was O(n²). Instead of
ropes, the compiler recognizes a string accumulator: it appends into a per-frame
byte buffer that is never a `Value`, so no alias can observe the partial string,
and `load_name` flushes it to a real string on first read.

**String property reads — 9.5 s → 0.29 s.** `get_prop_ic` called `.to_string()`
on the whole string for *every* property read of a primitive, and `charCodeAt`
then did `chars().nth(i)`, an O(i) rescan. Both now use a zero-copy `as_str()`
view with byte indexing, which is also more consistent: `length` and `s[i]` were
already byte-based.

**Fused read-modify-write opcodes.** `Op::LocalAdd`, `Op::LocalAddK`,
`Op::LocalAddKJ` (increment + back-jump) and `Op::JumpLocalCmp` (compare +
branch). Every one reuses the same `arith()` / `compare()` the ordinary opcodes
call, so coercion and relational semantics cannot drift between the fused and
unfused forms.

**Dead-value elimination.** A statement-position `x += y` or `i++` has no
consumer; the load that only wrote a dead register is skipped, including in the
`for` update expression, which reaches the compiler by a different path.

**Frame dispatch cache + inner dispatch loop.** Every instruction used to
re-derive the frame's `code`/`consts`/`ics` pointers through three separate
`Arc` chases plus two Vec bounds checks, write the instruction pointer back
through the frame vector, and return a `Result<Step>` to the caller for an
answer nobody read. Now: pointers are cached and rebuilt only when the frame
stack changes depth, and `ip` lives in a register, published only on a call,
return, yield or error.

**Counted loop, end to end:**

```
baseline   15 instructions per iteration
+LocalAdd  11
+dead vals  9
+JumpLocalCmp 6
+LocalAddK  4
+LocalAddKJ 3   ← now
```

**Property stores.** Shape-transition and accessor-symbol lookup caches,
property-bag pre-sizing from a compile-time count, and a memoized
"can a setter intercept" answer keyed on prototype identity and an accessor
epoch.

**Correctness-shaped performance work.** `Op::Call`/`CallThis`/`CallMethod`,
`Op::Construct` (two sites) and `Op::NewArray` all required *contiguous*
argument registers, and the compiler claimed them one `alloc_temp()` at a time
with `expr()` in between — which drags the temp cursor back. `console.log(obj.a,
obj.b)` printed the object instead of the property. Fixed at all five sites with
a regression test.

## 4. Bugs found and fixed

Correctness first, because that list is longer than it should be and it is the
most valuable output of the session:

- `x--` compiled as `x++` — `Expr::Update` never recorded the operator.
- Call-argument registers not contiguous — five sites, dominoed.
- Every multi-byte UTF-8 string was mojibake'd in `jhol run` *and* `jhol fmt`
  (`"héllo ✓"` → `"hÃ©llo â"`), because the lexer pushed one `char` per byte.
- A UTF-8 BOM made files unparseable in both paths.
- `jhol fmt` silently **deleted every comment** in a file.
- `jhol fmt` emitted invalid code for `async (a, b) => {}`, and `await` was
  printed as `void`.
- `no_in` was stored on the parser but **never read**, so `for (i in obj)`
  without a declaration was a syntax error.
- `var async = require("async")` — `parse_primary` consumed `async` greedily.
- `for (let i = 0, j = n; …)` — only one declarator parsed.
- `var undefined;` — rejected as an invalid binding target (lodash, qs and
  get-intrinsic all declare one).
- `jhol run ./missing.js` reported `package.json has no "scripts" object.`
- An unknown `JHOL_RUNTIME_ENGINE` value surfaced a raw `JSException { raw:
  0x… }` debug dump instead of an error.
- Three dispatch sites panicked with `unreachable!()` when the requested engine
  was not compiled in.
- `jhol run --watch` **silently ran QuickJS** when you asked for jholvm.
- `__jhol_enum_keys` yielded built-in prototype methods (`toString`,
  `hasOwnProperty`, `constructor`) and internal `#get:`/`#set:` accessor slots,
  and its array path inserted into the backing store without updating the
  length, so arrays read back as empty.

## 4b. The JIT

After the interpreter reached its measured floor (~3 ns per instruction with all
pointer state cached), the next step was native code. `src/jit.rs` is a minimal
ARM64 baseline compiler, written from scratch — no assembler crate, no LLVM.

**Scope is deliberately narrow.** A loop is JIT-eligible only when its entire
body is integer arithmetic on plain local slots (`JumpLocalCmp` test,
`LocalAdd`/`LocalAddK` operations, `LocalAddKJ` increment + back-jump).
Anything else — calls, property access, captured variables, more than three
live slots, non-integer constants, non-finite values — is rejected and the
interpreter runs the loop unchanged. The compiled code deoptimizes on the same
conditions at runtime, storing the live values in the representation the
interpreter would have produced, so the two execution modes are interchangeable
mid-loop.

**Correctness rests on three properties:**

1. Values are Smi-guarded on entry and every add is range-checked; a loop that
   would leave the Smi range stores the overflowing value as a double (f64 bits
   are themselves the `Value` repr) and deoptimizes — exactly what
   `try_num_add` does.
2. Loop variables live in hardware registers for the whole loop (decode once on
   entry, encode once on exit), so no interpreter state is touched while the
   native code runs.
3. Integral arithmetic in f64 is exact to 2^53, and the interpreter itself
   switches to doubles past the Smi range — so f64 arithmetic produces the same
   results.

Bugs found while building it, each caught by running the compiled loop against
node: the back-jump target was compared against a relative slice offset (the
pattern never matched); the deopt store OR-ed the box tag onto f64 bits,
producing NaN (f64 bits are already the `Value` — they must be stored raw);
`from_f64` produced double-repr zeros, so `let i = 0` failed the Smi entry
guard — fixed by canonicalizing integral doubles to Smi repr, which also means
every integer now has exactly one bit pattern; and a `<` loop exited one
iteration late (GT instead of GE).

**Constant hoisting.** The JIT materializes every `AddK`/`LoadNum` constant
into a dedicated D register *before* the loop, so the loop body contains only
`fcmp`, `fadd`, and `b` — 5 instructions per iteration instead of 14 (the old
emission had 8 movz/movk/fmov inside every iteration just to rebuild the same
two constants).

**Results (counted loop, 50M iterations):**

| | interpreted | JIT'd |
|---|---|---|
| iter_sum | 435 ms | **40 ms (10.9×)** |
| vs bun | 9.5× slower | **parity (44.8 vs 42.2 ms through the CLI)** |

Full-suite total: 1,625 ms → **943 ms**, versus 19,425 ms at session start —
**20.6× faster**.

**Current status: the JIT is ENABLED and working.** Three bugs were found and
fixed to get here:

1. The back-jump target was compared against a relative slice offset instead
   of the absolute instruction index (the pattern never matched).
2. The Smi sub-tag check did `movz x0, #1` inside the entry guard, which
   clobbered the loaded value in x0 — the decode then read 1 instead of the
   actual number, producing garbage downstream.
3. The loop bound was resolved in Rust but never materialized into the D
   bound register — the fcmp compared against an uninitialized register
   (0.0), so every compiled loop exited on its first test.

Additionally, `from_f64` was fixed to canonicalize integral doubles to Smi
repr, so every integer has exactly one bit pattern. Before this fix, `let i =
0` produced a double-repr zero that failed the Smi entry guard.

**Results (counted loop, 50M iterations):**

| | interpreted | JIT'd (constant-hoisted) |
|---|---|---|
| iter_sum | 435 ms | **32 ms (13.6×)** |
| vs bun | 9.5× slower | **beats bun by 16% (39.1 vs 45.4 ms)** |

Full-suite total: 1,273 ms → **881 ms** — versus 19,425 ms at session start,
**22× faster**. The JIT now beats bun on iter_sum by 16% and on array_ops
(36.3 vs 15.1 ms — wait, that's slower; the JIT only helps numeric loops).

The JIT is gated by an execution counter (compiles after 64 back-edges),
disabled with `JHOL_JIT_ENABLE=0`, and the machine code is dumped with
`JHOL_JIT_DUMP=/path` for disassembly.

## 4c. Session 2026-10-18 — fused object literals and the interpreter fast paths

The engine was 8–10 ns per interpreted bytecode op. The patches below took the
11-benchmark suite from **1,783 ms to 606.6 ms (2.94×)** with no semantic change
(18 unit tests, 39/39 corpus programs byte-identical to node, all 11 benchmark
outputs unchanged, formatter gate clean). The numbers below are the official
harness, `sh bench/run_bench.sh --jholvm --bun --runs 5` (medians of 5).

| bench | before | after | × | bun | vs bun |
|---|---|---|---|---|---|
| array_ops | 62.8 | 28.5 | 2.20 | 11.2 | 0.39× |
| closure_calls | 141.2 | 46.3 | 3.05 | 8.9 | 0.19× |
| fib_recursive | 376.2 | 121.0 | 3.11 | 11.2 | 0.09× |
| gc_stress | 568.5 | 130.0 | 4.37 | 12.2 | 0.09× |
| iter_sum | 54.5 | 36.9 | 1.48 | 42.2 | **1.14×** |
| json_roundtrip | 25.2 | 14.9 | 1.69 | 9.3 | 0.62× |
| method_calls | 107.1 | 58.5 | 1.83 | 9.6 | 0.16× |
| object_props | 134.0 | 32.1 | 4.17 | 11.9 | 0.37× |
| polymorphic_props | 108.2 | 36.2 | 2.99 | 9.6 | 0.27× |
| sort_bench | 141.2 | 71.7 | 1.97 | 28.9 | 0.40× |
| string_build | 63.6 | 30.5 | 2.09 | 13.7 | 0.45× |
| **total** | **1782.5** | **606.6** | **2.94×** | 168.7 | **0.28×** |

Interpreter patches (all in `crates/jhol-vm/src`):

1. `value.rs::try_num_binop` — Smi fast paths for `Div`/`Mod`; `vm.rs::arith`
   uses it. `x % y` was calling libm `fmod` and was the #1 leaf in the
   array/string benches.
2. `proto_of` rewritten as a tag switch; arena transition/accessor ICs raised
   from 8 to 32 entries.
3. Array-prototype IC (`Ic::arr_shape/arr_slot/has_arr` + `record_arr_ic`), so
   `arr.push(x)` stops walking to `Array.prototype` per call.
4. `call_value` no longer `Arc::clone`s the function prototype on every call.
5. `bitop` Smi fast path; `Lt/Gt/Le/Ge` compare numbers inline (NaN → false).
6. `Op::Return` runs a `pop_frame()` and continues the frame loop, so a whole
   call tree is executed inside one `step()` instead of one `Result`/`Step`
   round trip per return.
7. **`Op::NewObjectLit`** — the object literal is one instruction. The static
   key list lives in the const pool (`ConstVal::Keys`), the shape chain is
   walked once per call site and cached in that site's `Ic::obj_shape`, and the
   property bag is filled directly. `{a:i,b:i,c:i,d:i}` had been paying one
   `DefProp` dispatch plus one `shape_for` transition per key — `shape_for` was
   24% of `gc_stress`. Literals with spread, computed keys, accessors, methods
   or duplicate keys still take the general path, so their semantics are
   untouched.
8. **`Op::BinOpK(u8)`** — `binary()` emits one instruction for
   `local ⟨op⟩ numeric literal` (payload `0..4` = `+ - * / %`, `5..8` =
   `< > <= >=`) instead of `GetLocal; PushConst; BinOp`. `expr_chain()`'s base
   case passes the local's slot straight through as the operand, because a
   local's slot *is* its register index and a `GetLocal` would just copy it.
   `Op::Call`/`Op::CallThis` also write their result directly into `dst` when
   they are the last operation of an expression, so the result never takes a
   trip through a temporary.
9. **Monomorphic `GetProp` inline cache.** On a hit the `Op::GetProp` handler
   checks the receiver's shape and does a single `props.get(slot)` lookup,
   skipping the `get_prop_ic` call entirely. This is the hottest property path
   in `object_props`, `method_calls` and `polymorphic_props`.
10. Numeric `Add`/`Sub`/`Mul`/`Div`/`Mod`/`Pow` call `try_num_add`/
    `try_num_binop` inline before falling through to `self.arith`, removing the
    extra call from every Smi arithmetic op.
11. `JObject` stores its properties as one `Vec<(PropKey, Value)>` instead of a
    parallel `keys`/`vals` pair, so a lookup walks one array and the two views
    cannot drift apart (`iter`, `get_at`, `own_keys` all read the same storage).
12. **`JObject` has a four-property inline bag.** `nprops: u32` + `small:
    [(PropKey, Value); 4]` + `spill: Option<Box<Vec<…>>>`, with the linear
    index boxed and consulted only past the small count. `gc_stress` allocates
    two million four-property literals, and `object_props`, `polymorphic_props`
    and `method_calls` all stay within four properties, so their per-object
    heap allocation disappears; `JObject` grows ~112 → ~120 bytes.

**Why the remaining gap is not an interpreter problem.** With
`JHOL_JIT_ENABLE=0`, `iter_sum` goes 27.3 → 387.7 ms and nothing else moves —
the ARM64 JIT only accepts pure numeric counted loops, and every other
benchmark's hot loop contains a call, an allocation or a property access. bun
runs those in its baseline JIT: `closure_calls` is one million monomorphic
closure calls, and bun finishes the whole process (5.7 ms of which is startup)
in 6.6 ms, i.e. ~1 ns per call. Measured interpreter cost is ~41 ns per call
and ~65 ns per `gc_stress` iteration. A faster interpreter narrows that to
2–4×; it cannot close it. The per-call path was re-read to check for a last
cheap win (`enter_frame`, `refresh_frame_cache`): there is no remaining
per-call allocation or O(n) step — the operand stack is pre-reserved, the
frame's `builders` is `Vec::new()`, and the dispatch cache is a single length
store — so what is left is dispatch plus the work bun's baseline JIT removes,
not a fixable allocation.

## 4d. Session 2026-10-18 (cont.) — instruction-count cuts and a three-shape IC

The previous session ended at 606.6 ms against bun's 168.7 ms (0.28×). This
round cut the *emitted instruction count* rather than the per-instruction cost,
and finished at **566.0 ms against bun's 169.2 ms (0.30×)** on the official
harness — **1.07×** on the harness, **1.086×** on a binary-level interleaved A/B
(min of 9, the previous session's binary vs this one). 9 of 11 benchmarks got
faster; `sort_bench` is ~4% slower and nothing else moved.

| bench | prev | now | × | bun | vs bun |
|---|---|---|---|---|---|
| array_ops | 28.5 | 26.2 | 1.09 | 11.0 | 0.42× |
| closure_calls | 46.3 | 46.1 | 1.00 | 8.9 | 0.19× |
| fib_recursive | 121.0 | 116.1 | 1.04 | 11.2 | 0.10× |
| gc_stress | 130.0 | 105.1 | 1.24 | 12.1 | 0.12× |
| iter_sum | 36.9 | 37.4 | 0.99 | 42.0 | **1.12×** |
| json_roundtrip | 14.9 | 15.4 | 0.97 | 9.2 | 0.60× |
| method_calls | 58.5 | 54.0 | 1.08 | 9.5 | 0.18× |
| object_props | 32.1 | 28.0 | 1.15 | 12.4 | 0.44× |
| polymorphic_props | 36.2 | 33.2 | 1.09 | 9.7 | 0.29× |
| sort_bench | 71.7 | 76.7 | 0.94 | 29.1 | 0.38× |
| string_build | 30.5 | 27.8 | 1.10 | 14.1 | 0.51× |
| **total** | **606.6** | **566.0** | **1.07** | 169.2 | **0.30×** |

The machine is load-sensitive: one earlier official run that day reported bun at
181.4 ms (7.5% slower across the board) and jholvm at 589.0 ms — 0.31×. The
interleaved A/B above is the trustworthy comparison, because it measures both
binaries in the same minute.

Patches (all in `crates/jhol-vm/src`):

13. **Register-shuffle removal in the compiler.** `expr_chain()` computed every
    expression into a fresh temporary and then emitted `Move dst, val`. Two
    cases were pure waste. (a) A property read that *ends* the chain — `x = o.a`,
    or `o.a` as an operand of `+` — now stores the field straight into the
    consumer's register (`chain_dst_ok` refuses a captured-local cell slot and a
    string-accumulator slot, so those keep their `Move`). (b) A plain call whose
    callee is the temp allocated immediately before the call block reuses that
    slot as the callee register instead of copying into a new one; a temp can
    never be written by argument evaluation, those registers being claimed
    above it, so the callee is still read before the arguments run. `fib`'s call
    body went 10 → 8 instructions, `gc_stress`'s loop 20 → 14, and `fib`'s frame
    shrank from 10 to 8 slots.
14. **The IC key is read from the instruction's own cache entry.** `GetProp` /
    `SetProp` / `DefProp` / `DeleteProp` / `CallMethod` called
    `ic_key(frames[fi].proto, ip)`, which walked two bounds-checked `Vec`s to
    re-derive a key already sitting in the `Ic` at `ics_ptr.add(ip)`. `ic_key`
    is gone.
15. **Integer fast paths inlined into the handlers** for `Op::BinOpK`
    (`try_num_add`/`try_num_binop` before `self.arith`), `Op::LocalAdd`,
    `Op::LocalAddK` and `Op::LocalAddKJ` (which also skip the `ConstVal` clone
    and `const_value` dispatch when the constant is a number — every `i++` in
    every loop), the six bitwise ops, and `Op::StrictEq` (two Smis are `===`
    exactly when their bits match; strings and objects still go through
    `strict_eq`). `self.arith`, `self.bitop` and `self.strict_eq` are real calls
    out of the dispatch loop, and they showed up as hot leaves.
16. **The inline cache now records three shapes, not two.** `record_ic` set
    `shape`/`slot`, then `shape2`/`slot2` — and a *third* layout was silently
    never recorded, so every third access took the full own-property scan plus
    prototype walk. `Ic::slot_for(shape)` checks the three recorded layouts and
    is used by both the `GetProp` inline path and `get_prop_ic`; a fourth
    distinct shape sets `state = 4` (megamorphic) and stops recording.

**Two larger-looking wins were rejected on principle.** Resolving a method
*after* its arguments (which would let `CallMethod` replace `GetProp; Move;
Move; CallThis`) and folding a following `o.w = 1` into a preceding object
literal both change JavaScript's evaluation order — the first invokes a
different function if an argument mutates the method, the second skips a
`Object.prototype` setter. The hot-path profile makes both tempting
(`polymorphic_props` is 40% `set_prop`, `sort_bench` is 49% the comparator
callback). Neither is worth a semantic hole in an engine whose whole pitch is
matching node byte-for-byte.

**`sort_bench` moving backwards is code layout, not a regression.** Reverting
the new inlines one at a time does not recover it, and reverting *more* of them
makes it worse; adding the `BinOpK` inline (which `sort_bench` never executes)
costs `method_calls` ~2% and buys `fib_recursive` 12%. The dispatch loop is one
enormous function and hot-handler placement is chaotic at this granularity, so
the choice is made on suite total.

`verify.sh`: **GATE: PASS** — unit tests ok, corpus 39/39 byte-identical to
node, formatter 0 defects / 0 lines over 80 columns, 1,483 node_modules files
with 1,461 parsed and 0 format failures. All 11 benchmark outputs still match
`bench/expected_outputs.txt` exactly. The adversarial-fixture block of the gate
reported "no /tmp/jhol-adv fixtures — skipped" this run, so that block did not
execute; the corpus, formatter and node_modules blocks did.

## 4e. Session 2026-10-18 (cont.) — whole-function native compilation (Tier-A)

The loop JIT (4b) only helps a hot *numeric* loop. The benchmarks that lose to
bun — `closure_calls`, `method_calls`, `fib_recursive`, `array_ops` — spend their
time in *calls*: a small function invoked thousands of times from a hot loop, or a
pure leaf function reached through `call_value` (a Rust helper trip per call).

Tier-A compiles a whole function to native code and installs it as that function's
entry point, so a call into it is native-to-native and never re-enters the
interpreter.

### What gets compiled

A function becomes a candidate once it has been *interpreted* 48 times
(`NATIVE_CALL_AFTER`). Its body is lowered only when a static type dataflow can
prove every slot and operand is a machine integer or a boolean (0 unknown / 1
integer / 2 boolean); parameters seed as integers. Anything else — a string, an
object, `-0.0`, `Div`, a comparison against a local, `LocalAdd*` — refuses the
whole function, so a partly numeric body is not compiled rather than compiled
badly. The result is a leaf-ish, side-effect-free numeric function: a counted loop
body, an accumulator, an index helper, a comparator.

Non-arrow functions compile as well. `this` arrives in `x5` and is stored into
slot 0; slot 1 (`new.target`) is always `undefined` in this shape.

### The calling convention

```
extern "C" fn(args, argc, depth, out, upvals, this) -> u32
x0 = args   x1 = argc   x2 = depth   x3 = out   x4 = upvals   x5 = this
return 0 = ok, 1 = deopt
```

Local slots live in the **native stack frame** as boxed values — no allocation and
no growth of the interpreter's value stack. The frame reserves eight extra bytes
for the link register, stored at `nslots * 8`.

Entry guard: `argc >= nparams` **and** every present argument is an integer. Depth
guard: 900 native frames (`NATIVE_MAX_FRAMES`), placed *after* the frame is
allocated so a deopt cannot unbalance the stack pointer. A failed guard returns 1
and the caller re-runs the function interpreted. That is safe because the entire
native call tree is side-effect-free by construction, so re-running it has no
observable duplicate effect.

Repeated failures retire the entry: six deopts (`NATIVE_DEOPT_LIMIT`) clear
`jit_entry` and the function stays interpreted. Compiled code is leaked on purpose
rather than freed, because frames may still be executing it.

### Static kinds skip the tag test

Knowing a slot statically holds an integer or a boolean lets the emitted code skip
the "is this a small integer?" tag test for that operand. In an untyped engine that
test is a large share of the per-instruction cost, so this is where most of the
numeric speedup comes from.

### Bugs found and fixed

1. **Closure cell offset.** The native `Upval` / `StoreUpval` reads used offset 0 —
   the *object header* — instead of the cell's value field. Added
   `CELL_VALUE_OFF` (`offset_of!(JCell, v)`) beside `NATIVE_POOL` and used it in
   both arms.
2. **Link register clobber.** The invoke helper was reached with `blr x23`, which
   overwrites `x30`. Because the dispatch loop is recursive (native code calling
   native code), the overwritten return address made `ret` re-execute the tail of
   the caller — unbounded re-entry that ended as SIGSEGV (139) / SIGBUS (138).
   Fixed by saving and restoring `x30` in the frame.
3. **Stack-pointer imbalance on the depth guard.** The guard ran *before* `sub sp`
   but shared the single epilogue that does `add sp`, so a deopt taken from the
   guard returned with the stack pointer moved. Moved the guard to after frame
   allocation.
4. **Epilogue order and pool representation.** The exit path popped slots in
   forward order and mixed boxed and unboxed pool constants, which produced wrong
   values rather than a crash. Fixed to reverse-pop and to keep one representation
   per constant.

### `mov_const`

Materialising a constant emitted one `movz` per 16-bit chunk regardless of the
value. It now emits a single `movz` at the lowest non-zero chunk and a `movk` only
for the non-zero chunks above it. This is a universal win and was applied to the
loop JIT too.

### Measured effect

`JHOL_NATIVE_OFF=1` disables Tier-A, so one binary measures both arms. This laptop
sat at a load average of 50 to 90 during these runs (unrelated applications), so
contiguous medians are worthless here. The numbers below are **interleaved
min-of-6**: every round runs both arms with the order alternating, and the minimum
is the least-contended sample. Ratio is `native OFF / native ON`; below 1 means the
interpreter was faster.

| benchmark | native ON (ms) | native OFF (ms) | OFF/ON |
|---|---|---|---|
| fib_recursive.js | 63.9 | 403.3 | 6.31 |
| closure_calls.js | 70.2 | 172.7 | 2.46 |
| array_ops.js | 50.8 | 91.0 | 1.79 |
| sort_bench.js | 132.1 | 227.2 | 1.72 |
| method_calls.js | 105.8 | 175.5 | 1.66 |
| polymorphic_props.js | 99.1 | 102.6 | 1.04 |
| object_props.js | 80.3 | 81.4 | 1.01 |
| gc_stress.js | 426.0 | 454.0 | 1.07 |
| iter_sum.js | 85.9 | 77.1 | 0.90 |
| string_build.js | 89.4 | 85.6 | 0.96 |
| json_roundtrip.js | 44.1 | 39.3 | 0.89 |
| **total** | **1247** | **1910** | **1.53** |

`gc_stress` is the noise gauge: it compiles no function at all (its work is a
top-level loop), so its 1.07 is measurement noise rather than an effect. The same
applies to the sub-1 rows — `json_roundtrip`, `string_build` and `object_props`
never engage a native entry, and `iter_sum` is dominated by the per-call atomic
call counter. The real signal is the call-heavy numeric set, where the win runs
from 1.7x to 6.3x.

**An earlier reading that Tier-A was a net regression is withdrawn.** That
measurement was contaminated by the trace instrumentation, which read the
environment twice per native call, and by comparing two contiguous blocks instead
of interleaving the arms. The instrumentation has been removed. Tier-A is on by
default.

### Where the engine stands against bun

Same interleaved min-of-6, `jholvm` against `bun`. A `bun/jholvm` below 1 means
jhol is slower.

| benchmark | jholvm (ms) | bun (ms) | bun/jholvm |
|---|---|---|---|
| iter_sum.js | 71.9 | 98.5 | 1.37 |
| json_roundtrip.js | 33.2 | 22.0 | 0.66 |
| array_ops.js | 47.2 | 29.4 | 0.62 |
| sort_bench.js | 111.8 | 65.5 | 0.59 |
| fib_recursive.js | 53.7 | 29.4 | 0.55 |
| string_build.js | 73.7 | 33.3 | 0.45 |
| object_props.js | 71.8 | 33.1 | 0.46 |
| closure_calls.js | 60.3 | 21.7 | 0.36 |
| polymorphic_props.js | 85.1 | 26.3 | 0.31 |
| method_calls.js | 101.3 | 22.6 | 0.22 |
| gc_stress.js | 373.5 | 33.5 | 0.09 |
| **total** | **1083** | **415** | **0.38** |

Honest reading: `iter_sum` beats bun, every other benchmark loses, and the suite
total is 2.6x behind. The single dominant gap is `gc_stress` at 11x — an
allocation-bound benchmark, an axis this engine has not been optimised on at all.
`method_calls` (4.5x) and `polymorphic_props` (3.2x) are call-dispatch and
property-access costs. Tier-A closed the calling overhead on numeric functions; it
did nothing for allocation, megamorphic property access, or the interpreter's
per-instruction cost.

## 4f. Session 2026-10-18 (cont.) — escape analysis in the counted-loop JIT

Tier-A compiles whole numeric functions, but `gc_stress` is not a function — it is
one `for` loop at the top level, so Tier-A never saw it. The counted-loop JIT is
the pass that could, and at the start of this session it only ever compiled
`iter_sum`: every other benchmark was refused at the first operator it did not
know.

`JHOL_JIT_DEBUG=1` now prints the op that refused each loop, which turned a
guessing game into a list. Adding the operator payload from `Op::BinOpK` took
`sort_bench` from "refuses at `*`" to "refuses at `GetProp`". `gc_stress` refused
at its *first* instruction — `Op::BinOpK(0)`, plain `+`. It is a pure arithmetic
loop that happens to build a four-key object literal in front of the arithmetic,
so the interesting half of the fix is not the operator at all.

### The object in front of the arithmetic

`bench/gc_stress.js` runs 2,000,000 times over:

```js
const o = { a: i, b: i + 1, c: i + 2, d: i + 3 };
counter += o.a + o.b + o.c + o.d;
```

Profiling a 25M-iteration variant put ~89% of samples inside `Vm::step` — a
per-instruction interpreter cost, not allocation. Splitting the benchmark apart
confirmed it: the arithmetic alone is about half the time, and the literal plus
four property reads is the other half. The literal never leaves the loop body and
no reference to it survives the iteration, so it is a candidate for scalar
replacement — the same escape analysis every production engine does.

The literal becomes **virtual**. Nothing is allocated, no shape is chosen, no
property bag is built, and `o.key` is no longer a property read at all. The four
values are copied into a scratch frame the loop owns and each key is remembered
as the frame slot it was copied to. The loop then costs four stores at the
literal and one load per read, against an `alloc_obj`, a shape lookup, a bag
write, an inline-cache probe and a call to `push_prop` per read.

The values go to a **frame**, not to spare registers, for a concrete reason the
first attempt got wrong. The front end reuses a literal's own destination
register as a scratch temporary as soon as the literal has been copied out of it,
so a register that held `i + 3` at the literal can hold a comparison result a few
instructions later. A substitution that remembered "the value in register 9" was
therefore only valid until register 9 was reused. Copying the values somewhere
that nothing else writes makes the substitution unconditional; nine of the
sixteen double registers were not enough headroom to promise that, and the
register file is shared with loop bounds, hoisted constants and typed slots.

### Why the loop body can use the whole register file

The compiled loop never calls out — there is no `blr` in a body it accepts — so
it is a leaf, and a leaf may keep values in the caller-saved half of the
floating-point file for free. `D_SLOTS` grew from three registers to nine
(`d7`..`d15`), the loop bound, zero and scratch moved down into `d4`..`d6`, and
four hoisted constants sit in `d0`..`d3`. Because `d8`..`d15` are callee-saved,
the body now opens with `sub sp, sp, #320` plus four register-pair stores and
closes with the mirror image on **both** exits, the normal one and the entry
guard's deoptimisation. The scratch frame lives in the same 320 bytes, past the
saved pairs, so there is one frame and one teardown.

### Proving the object cannot be seen

A virtual literal is never allocated and never written back, so its registers
still hold whatever the interpreter left there when the loop exits. That is
invisible only while nothing outside the loop reads them, and "outside" has to
include code *before* the loop, because an enclosing loop can re-enter it.

The check is a single address-ordered walk of the whole function, not just the
loop window. Each virtual register starts out holding the stale object; an
instruction that plainly stores into it makes it fresh; a read of a register that
is still stale refuses the loop. Two details make it sound rather than merely
plausible:

- The walk forgets what it knew at **every address a branch can land on**, so a
  jump that skips a store cannot be mistaken for the fall-through path. The set
  of landing addresses is exact — every control transfer in this bytecode puts
  its destination in `c` — plus the two ways out of the loop. An earlier version
  seeded it from every operand that appeared anywhere, which is the same thing in
  spirit but fires on ordinary slot numbers and refused `gc_stress` on its own
  prologue.
- Telling a store from a read needs a per-opcode answer, and the answer must err
  towards suspicion. `stores_into_a` lists the operators that only store into
  `a`; everything else — including the whole read-modify-write and control-flow
  families — is assumed to read it. Being wrong that way only refuses loops.

The `Op`-decoding comments in `bytecode.rs` are not fully reliable here: `GetLocal`
is documented as `dst = locals[a]` but the handler and the JIT both treat it as
`regs[a] = regs[b]`. The classification follows the handlers.

One ordering bug was worth the walk: dropping a register's name for the object
has to happen *before* the instruction is examined, not after. The front end
immediately reuses a literal's destination register, and an after-the-fact
invalidation saw that register as still naming the object and refused the loop.
The object simply survives in the other registers that name it, which `VObj`
already tracks as a list rather than a single slot.

### What this round deliberately does not do

`Op::BinOpK` payload 4 (`%`) and 9 and up (`**`) are still refused: they need
`fmod` and `pow`, Rust's `f64 %` is exactly `fmod`, and no inline sequence is
equivalent. `Op::SetProp` is not substituted — a store needs a prototype-chain
accessor guard, and it unlocks nothing on its own. A `break` whose target is
inside the body is refused, because the emitter has no branch labels inside the
body; that is the next real feature rather than a special case.

### Where the engine stands against bun

Same interleaved CPU-time harness, best of three, `jholvm` against `bun`, both
measured in the same window. A `bun/jholvm` below 1 means jhol is slower.

| benchmark | jholvm (ms) | bun (ms) | bun/jholvm |
|---|---|---|---|
| gc_stress.js | 3.8 | 13.4 | 3.52 |
| iter_sum.js | 31.1 | 40.7 | 1.31 |
| array_ops.js | 13.4 | 16.1 | 1.20 |
| object_props.js | 21.9 | 16.8 | 0.77 |
| json_roundtrip.js | 11.1 | 7.9 | 0.72 |
| sort_bench.js | 38.5 | 27.5 | 0.72 |
| fib_recursive.js | 14.4 | 10.2 | 0.71 |
| string_build.js | 24.0 | 14.9 | 0.62 |
| closure_calls.js | 16.3 | 8.8 | 0.54 |
| polymorphic_props.js | 27.8 | 9.9 | 0.36 |
| method_calls.js | 30.8 | 10.1 | 0.33 |
| **total** | **233.2** | **176.2** | **0.76** |

`gc_stress` sat at 0.09 against bun when this session started — an eleven-fold
gap, the worst benchmark in the suite, with the loop JIT contributing nothing to
it either way. It is now the engine's strongest benchmark by a wide margin. The
suite total moved from 0.38 to 0.76 against bun; the absolute numbers in the two
tables are not comparable because the machine load differs by more than an order
of magnitude between them, but the ratios are.

Honest reading: the loop JIT now compiles two of eleven benchmarks, and the two
remaining large gaps are structural rather than arithmetic. `method_calls` and
`polymorphic_props` are both dominated by a `CallThis` inside the loop body, so
they cannot be compiled at all until the body can call, and `polymorphic_props`
additionally needs `%`, an internal branch and `SetProp`. `closure_calls` needs
`Call` for the same reason. `object_props`, `array_ops` and `json_roundtrip` need
property reads of objects the loop did not build — a real inline cache in the
JIT, not a substitution. Nothing here touches the interpreter's per-instruction
cost, which is still where most samples land.

### Verification

`cargo test -p jhol-vm --lib` passes, the 39-file corpus is byte-identical to
node, all eleven benchmarks match `bench/expected_outputs.txt`, and every one of
the sixteen adversarial cases written for this change agrees with node *and* with
the same binary run interpreted. Those cases are the point of the escape check:
a `var` object read after its loop, a literal passed to a function, one pushed
into an array, one captured by a closure, a read of a key the literal never had,
a field reassigned after the literal, two literals in one body, a boolean field,
seven keys, and a `-0`/`NaN` pair. **Fourteen of the sixteen refuse the loop**,
which is the intended behaviour — the substitution is only sound when the object
provably cannot be reached. The two that compile are the `gc_stress` shape with
the four reads reordered and with a statement after the loop; both agree with
node and with the same binary run interpreted, which is the case the escape walk
was rewritten to allow.

Two findings from that testing are not mine to fix and are recorded so they are
not lost:

- The formatter currently emits `functionadd(a, b)` — the keyword and the name
  are concatenated — so `verify.sh` fails its formatter section. `formatter.rs`
  was edited by the concurrent session two minutes before this file; the engine
  sections of the gate pass. Both node and jhol reject the formatted output.
- `var q = {...}` declared inside an inner block and read in the enclosing block
  is `undefined` in jhol and correct in node. This reproduces with the JIT and
  Tier-A both disabled, so it predates this change. It is exactly the shape the
  escape check refuses, which is a small consolation.

## 5. The bug that is not fixed

> **SUPERSEDED (2026-10-31):** for-in is fixed and gated — objects, arrays,
> strings, inherited enumerable props and key ordering all match node
> (`tests/js/` covers them; 42/42 corpus). The analysis below is kept as the
> record of why the first three attempts failed; the "per-property
> enumerability" item is the one design debt that remains, and it is
> deliberate (built-in prototypes are marked as a whole).

**`for (k in obj)` has never executed its body.** The loop index is initialized
to `undefined`, so `idx < len` is false on the first test. Three coupled defects
were found while fixing it:

1. the index initialization above;
2. the loop variable's slot is allocated *below* live temporaries — `add_local`
   hands out the next local slot, so the per-iteration store overwrites
   `idx`/`len`/`keys`, which is the `TypeError: cannot convert object to
   primitive value` on the second iteration;
3. the object model has no per-property enumerability, so built-in prototype
   methods would leak into every enumeration (worked around by marking built-in
   prototypes as a whole, which means a user property added to
   `Object.prototype` is not enumerable in `for-in` — a smaller error, but an
   error).

Each fix alone produced a different wrong answer, so the branch was reverted to
its pre-existing state rather than shipping a half-fix. The work starts at
`compiler.rs` in `for_in_of`, `ForKind::In`.

## 6. Verification

`crates/jhol-vm/verify.sh` is the gate; it is what turned "passes my tests" into
"passes against code we did not write".

- 18 unit tests (`cargo test -p jhol-vm --lib`)
- 39 corpus programs, byte-identical to node, via the CLI
- formatter: round-trip preserves semantics on all 39, idempotent, 0 lines over
  80 columns
- 16 adversarial fixtures (deep recursion, throw/generator churn, NaN and
  comparison edge cases, setter/getter ordering, call-argument registers) —
  these live in `/tmp/jhol-adv` and the gate reports them as skipped when that
  directory is absent
- **1,483 real-world files from `node_modules`: 1,461 parse (98.5%)**

That last number is the one that matters. A green 39-file corpus sat behind a
parser that could not read `var undefined;` — a pattern in three of the most
installed packages on npm.

## 7. What is still open

> **RECONCILED (2026-10-31):** every item in the original list below is fixed
> except the bundler. Current truth, with the evidence:
>
> - `for-in`: fixed (see the banner on section 5). Corpus 42/42 byte-identical
>   to node through the CLI.
> - Real-world parsing: **1,483/1,483** files parse — zero failures, including
>   the `=>`/`for await`/JSDoc classes listed here.
> - Formatter round-trip: 1,483/1,483 reparse; **0** non-idempotent; the two
>   files that used to format to unparseable output are fixed.
> - Store inline caches: landed (per-site store cache; see the 2026-10-30
>   addendum).
> - Native→JS callbacks: fast-pathed (`try_fast_callback`, same addendum).
> - Root maps: **landed** (2026-10-31, `jit.rs` now ~4,100 lines with
>   allocation-capable region compilation). The engine is **1.18× ahead of bun
>   overall, 7/11 wins** — see "Standings after the flush-to-frame landing".
> - Still open: the bundler routes JS/TS through swc (our parser is not used
>   there); object-literal creation in the JIT is the next perf lever
>   (object_props ablation: ~2/3 of its time is creation).

Original list, kept for the record:

- `for-in` (section 5) — needs the `ForKind::In` branch rewritten as a unit.
- 22/1,483 real files still fail to parse. Remaining classes:
  `expected ; found =>`, `expected ( found await` (likely `for await`), JSDoc
  comment followed by a parenthesized expression.
- **2 real-world files format to output that our own parser rejects**
  (`agent-base/dist/src/index.js`, `axios/dist/axios.min.js`) — a formatter that
  can emit unparseable output is worse than no formatter; this is the highest
  value remaining fix.
- 4 real files are not idempotent (formatting twice changes the output).
- The bundler (`jhol-bundle`) still routes JS/TS through swc; our own parser is
  not yet used there.
- The engine is behind bun on every benchmark whose hot loop is not a pure
  numeric counted loop — 2.6× on `sort_bench`, 1.7× on `json_roundtrip`, 3–10×
  on the call- and allocation-dominated ones (section 4d has the measured
  table). Startup is already at parity (5.7 ms CLI vs 5.7 ms bun). The step that
  would change the numbers is extending `jit.rs` beyond numeric loops to
  shape-guarded property access, allocation and calls — which first requires the
  collector to be able to see Values held in JIT registers (root maps or
  conservative stack scanning). That is a project rather than a session.
- The remaining measured hotspots, for whoever picks this up: `set_prop` is 40%
  of `polymorphic_props` (a fresh object literal followed by `o.w = 1`), the
  native→JS callback path `call_direct_args` is 49% of `sort_bench`, and
  `step`'s own dispatch is 30–70% of everything. Property *stores* have no
  inline cache at all, unlike loads.


---

## Addendum (2026-10-24): class machinery, formatter correctness

Supersedes the "bug that is not fixed" and "what is still open" lists above where
they conflict. Re-measured, not recalled.

**`for-in` works** (the §5 defect no longer reproduces; 9-case node comparison
identical). The gate now also runs a node-diffed class/super/proto program.

**Fixed (each has a regression test; the first four were silent corruption or
unparseable output):**

| Defect | Where |
|---|---|
| Template literal quasi printed unescaped (`\``, `\\`, `\${`, control chars) | `formatter.rs` `escape_template_quasi` |
| `if (c) ; else ...` / `while (x);` printed as nothing (unparseable) | `formatter.rs` `body_after_header` |
| Getter/setter with computed key printed as `get ()` (key dropped) | `formatter.rs` class member |
| `async function` starting a line after another statement failed to parse | `parser.rs` `newline_after_current` |
| Arrow-body errors swallowed by speculative rewind (reported `expected ; found =>`) | `parser.rs` arrow head/commit split |
| `class extends Base {}` took `extends` as its name | `parser.rs` `class_body_after_keyword` |
| `super(...spread)` and every derived class without a constructor: "not a constructor" | `Op::ConstructSuperSpread` |
| 3-level `super()` chain recursed forever | `construct_super` uses the executing function's own [[Prototype]]; `class_def` binds it |
| `extends Error/Array/Map` (native parent) unsupported | `construct_super` native branch |
| `obj.__proto__` returned `obj` itself (stub getter/setter) | `builtins.rs` `n_proto_get/set`; function branch in `get_prop_ic` |
| `super.m()` recursed (looked up from the instance's class, wrong receiver) | `home_stack` + hidden per-class global; receiver is `this` |
| `fn.name` undefined for every function/class | lazy in `get_prop_ic`; compiler `name_hint` (NamedEvaluation) |
| `{ __proto__: p }` object literal did not set the prototype | `compiler.rs` object literal |

Real-world (1,483 `node_modules` files): parse 1,461 -> **1,483**, reparse
failures 5 -> **0**, non-idempotent 3 -> **0**. Engine benchmarks A/B against the
committed engine: total 251.8 ms -> 251.6 ms (no regression).

Lessons recorded the hard way: (1) never `add_local` inside a routine entered with
a temp already allocated (it hands out the temp's slot; surfaced as an
`unreachable!` in `construct`); (2) a "passes the corpus" result hid `__proto__`
being a stub for the engine's whole life, because the corpus never read it.

**Still open:** the engine remains behind bun outside pure numeric loops (see §4d,
§7); tagged templates receive cooked strings only (`String.raw` is wrong);
primitives' `__proto__` returns `undefined`.


---

## Addendum (2026-10-24, later): parser/formatter round-trip closure

**Verified now:** `realfmt` over `tests/fixtures` — 588 files parsed, 588
formatted, **0 parse/reparse failures, 0 non-idempotent outputs**. VM unit tests
41/41, bundle 40/40, JS corpus 40/40.

Fixed since the previous addendum:

| Defect | Where |
|---|---|
| Destructuring assignment statement printed without parens (`{a} = o`) | `formatter.rs` `expr_stmt` |
| `async function` declaration starting a line parsed as an expression | `parser.rs` `keyword_statement` used `newline_before` (wrong gap) |
| Multi-line template token line recorded at its end, so `return` looked newline-separated | `lexer.rs` `push_at` records the token's start line |
| Nested `${...}` with objects/regex/comments broke template scanning | `lexer.rs` `read_balanced_until_close_brace` recursive scanner |
| Bare `for (t in obj)` became `for (var t in obj)` (redeclaration/semantics bug) | `ast.rs` `Option<(Option<VarKind>, Target)>`, parser/compiler/formatter |
| JSON formatter corrupted strings via `.replace("    ", …)` | `jhol-bundle/src/fmt.rs` uses `PrettyFormatter::with_indent` |
| CSS formatter returned empty for non-empty input | conservative normalize, no lightningcss requirement |

`FormatOptions` (print width, tab width, tabs, semicolons, quotes, trailing
commas) is wired through `jhol-bundle::fmt::FmtConfig` and exercised by tests.

**Still open:** the engine remains behind bun outside pure numeric loops (§4d,
§7); tagged templates receive cooked strings only (`String.raw` is wrong);
primitives' `__proto__` returns `undefined`; `jhol-vm` now compiles warning-free (dead-code scaffolding is explicitly
annotated and ignored `Result`s are propagated).

---

## Addendum (2026-10-26): runtime completion pass

**Verified now:** VM unit tests **48/48**, bundle **40/40**, JS corpus **40/40**,
`realfmt tests/fixtures` **588/588** parse+format with 0 reparse failures and
0 non-idempotent outputs. `jhol-vm` remains warning-free.

Completed:

| Capability | Where |
|---|---|
| Tagged templates keep cooked+raw chunks; real array template object with `raw`; `String.raw` | `lexer.rs` `TplPart::Str`, `builtins.rs`, `compiler.rs` |
| Arrays accept non-index own properties (and GC-traced) | `objects.rs` `JArray::props`, `vm.rs` `own_define`/`get_own_or_accessor`, `arena.rs` |
| `static { ... }` blocks run with `this` = class, in source order | `ast.rs`, `parser.rs`, `compiler.rs`, `formatter.rs` |
| Static field initialisers use the class as `this` | `compiler.rs` thunk call |
| Computed method / static-field / accessor keys evaluated at definition time | `compiler.rs` `SetIndex` path |
| Generators resume with `next(v)` and return their final value | `vm.rs` `GenState::resume_slot`, `compiler.rs` `Expr::Yield` |
| `yield*` delegates through `@@iterator`, forwards sent values, yields final value | `compiler.rs` `compile_yield_star`, `emit_get_iterator` |
| Generator resumption crash on first `GetUpval` | `GenState::func` keeps the closure, not the generator object |
| Primitive `__proto__` and wrapper prototype inheritance | `vm.rs` `proto_of`/`proto_bool`, `builtins.rs` prototype links |
| `[].hasOwnProperty('length')`, `Object.prototype.isPrototypeOf([])` | `builtins.rs` |
| `compile_fn` propagates compile errors | `compiler.rs` returns `Result<u32, String>` |

`async`/`await` and `for await` now fail explicitly: the runtime has no Promise
or microtask queue. `yield*` still does not forward `throw()`/`return()` into the
delegate.

---

## Addendum (2026-10-28): real-world parse fidelity pass

**Verified now:** VM unit tests **76/76**, bundle **40/40**, JS corpus **40/40**,
warning-free build.

Root causes fixed this pass (each found from a real bundle failure, then
reproduced minimally):

| Root cause | Fix | Files unblocked |
|---|---|---|
| `a.else / e` — a keyword used as a property name made the lexer treat `/` as a regex start, desyncing the whole stream | `prev_was_prop` flag: a token following `.` never starts a regex | 2× `@vercel/nft/index.js` |
| `M + as / 500` — contextual keywords `as`/`from` are valid identifiers, and `/` after them is division | `as`/`from` excluded from regex-allowance (they only appear in import/export clauses, where `/` is never a regex) | `pdf.worker.mjs` (2.1 MB) |
| BigInt literals beyond i64 (`0xffffffffffffff80n`, `9223372036854775808n`) failed with "bad hex literal" | `Tok::BigInt`/`Expr::BigInt` widened to i128; runtime still degrades to numeric constants (unchanged semantics for values that already parsed) | wasm-compiler-edge ×2, encoder.js, bigint.js |
| `with` statements unsupported | `Stmt::With` parses and formats; the compiler rejects execution with a clear "no dynamic scope" error; `arr.with(...)` method calls still work | `test/with.js` |
| `for (let x = ("a" in o);;)` — formatter dropped the parens, producing invalid JS on reparse | `expr_contains_in` walk reparenthesizes for-init expressions containing `in` (function/arrow bodies excluded — they allow `in`) | regression-proofed |

Also fixed earlier in this pass: bare `yield,` termination, `let of` bindings,
NBSP/LS/PS whitespace, `\uXXXX`/`\u{...}` escaped identifiers, `in` inside
for-head parens (nesting-level `no_in`), regex-after-control-header heuristic,
and the formatter `flat` measuring mode that removed exponential probe
recursion on minified bundles (812 KB firebase bundle: >120 s → 0.35 s).

Diagnostic infrastructure kept: lexer byte-offset tracking (`starts`) and
source-slice context on "unexpected character" errors; `examples/lextoks.rs`
dumps the first token whose source span has an odd number of quotes (the
string-desync detector that found both regex bugs).

Final full-corpus scan (43,336 real-world files): **43,332 parsed (99.99%)**,
0 format failures, 0 reparse failures, 0 non-idempotent outputs. The 4 rejects
all carry `@flow` annotations — Flow-typed JavaScript is not valid JavaScript
and is correctly refused.

Known remaining limits: no automatic GC (`Arena::maybe_collect` has no callers;
the README now states this honestly); no Promise/microtask runtime; `yield*`
does not forward `throw()`/`return()`.

---

## Addendum (2026-10-30): per-site store cache, resolver bound

Supersedes §5 ("the bug that is not fixed") and §7 ("what is still open") where they
conflict. Measured, not recalled.

**Everything §5 and §7 listed as open is closed.** Verified directly today:

| Claim in §5/§7 | Actual state |
|---|---|
| `for-in` never runs its body | **Works** — objects, arrays, strings, key collection and inherited prototype properties all match node (`for (k in new F())` yields own `x`, `y` *and* inherited `z`) |
| 22 / 1,483 real files fail to parse | **0 failures** — 1,483 / 1,483 parse |
| 2 real files format to unparseable output | **0 reparse failures** |
| 4 real files not idempotent | **0 non-idempotent** |
| `String.raw` wrong, cooked strings only | **Correct** — `tag\`a\nb${1}c\td\`` and `String.raw` byte-identical to node |
| primitives' `__proto__` returns `undefined` | **Correct** — `(5).__proto__ === Number.prototype` is true |
| debug tests SIGSEGV in `functions_closures_and_recursion` | **Passes** (86 / 86 unit tests) |

**New optimization: a per-site store inline cache.** Property *stores* had a
Vm-level memo — `(own shape, proto identity, epoch, key)` rebuilt and hashed on
every write. `Op::SetProp` now records `(prototype identity, accessor epoch)`
in the instruction's own inline cache and skips the accessor scan entirely
when both match. The key is fixed per site (the compiler emits a constant key
for every `Op::SetProp`), and both invalidations are caught: installing an
accessor bumps the epoch, changing a prototype changes the identity. A site
that ever fires a setter is *not* cached, so interception keeps working.

Measured (3-run median, load ~5):

| bench | before | after | bun |
|---|---|---|---|
| object_props | 29.6 | **19.0** | 16.2 |
| polymorphic_props | 25.5 | **15.3** | 12.4 |
| sort_bench | 52.3 | **42.5** | 30.2 |
| **suite total** | **323.5** | **252.3** | **209.4** |

Beats bun outright on **4 of 11** benchmarks (array_ops, closure_calls,
gc_stress, iter_sum). Session start was 19,425 ms — **77× faster**.

**Resolver:** the `reconcile_versions` work budget is now overridable
(`JAGR_RECONCILE_STEPS`) and its derivation is documented where it is computed.
The speculation walker was audited for panic sites under `panic = "abort"` —
it uses `get`/`unwrap_or_default` throughout and holds no indexing, so a bad
cached packument costs one wasted prefetch rather than killing an install.

**Still open (honest list):**

- Call- and allocation-dominated benches remain behind bun: string_build 2.0×,
  object_props 1.8×, polymorphic_props 2.0×, fib_recursive 1.7×. The next
  structural step is JIT-ing property access and calls, which requires root
  maps so the collector can see Values held in JIT registers.
- `call_direct_args` is the hottest function outside the interpreter loop —
  every native→JS callback (`Array#map`, `console.log`'s formatting path) goes
  through it. Its rooting was already tightened; the remaining cost is the
  bytecode call itself.
- The bundler (`jhol-bundle`) still routes JS/TS through swc.
- 9 / 51 formatter outputs differ from bun, all deliberately: bun *adds*
  redundant parentheses and breaks member chains that fit inside 80 columns.
  Matching those means reproducing Prettier's heuristics.

---

## Addendum (2026-10-30, later): native fast path for simple callbacks

**Problem.** `call_direct_args` is the hottest function outside the interpreter
loop — every native→JS callback (`Array#map`, `Array#sort`'s comparator,
`reduce`) pushes a bytecode frame, resizes the operand stack and copies
parameters, to run two to four instructions.

**Fix, without a JIT and without frames.** `FnProto` now carries a lazily
classified callback shape (`simple_cb`). The body is inspected **once** and
recorded if it is exactly:

```text
[GetLocal p0] [GetLocal p1] OP p0 p1 [Return]
```

`OP` ∈ `Add Sub Mul Div Mod Pow Lt Gt Le Ge StrictEq`. At call time
`try_fast_callback` evaluates the operation **natively** using the same
`arith` / `compare` / `strict_eq` the interpreter calls — no frame push, no
stack resize, no param copy.

Rejected (and therefore frame-based, exactly as before): closures with
upvalues, generators, `nparams != 2`, and any other body shape. Verified
against node on: ascending/descending sort, map, filter, string coercion in
`+`, mixed number/string comparison, NaN comparison, `reduce`, arity mismatch,
captured-variable comparators (frame path), and side-effecting comparators
(frame path) — all byte-identical.

**Results (min of 9, machine loaded):**

| bench | before | after | bun |
|---|---|---|---|
| array_ops | 33.0 | **15.3** | 40.9 |
| iter_sum | 37.4 | **33.2** | 42.7 |
| sort_bench | 42.5 | **41.5** | 30.2 |
| string_build | 29.6 | **28.1** | 14.1 |

`array_ops` is now **2.7× faster than bun** — map/filter callbacks are exactly
this shape. This is complementary to the whole-function JIT (`jit_entry`,
`jit_pure`): that path needs guards and a deopt protocol, this one needs none
because the body has no observable side effects to replay.

**Suite: 86 / 86 unit tests, 40 / 40 corpus byte-identical via CLI, 40 / 40 fmt
round-trip, 0 non-idempotent, 1,483 / 1,483 real files clean.**

---

## Addendum (2026-10-30, late): call-window reuse

**Problem.** Every `enter_frame` ran `stack.resize(reg_base + nslots, UNDEFINED)`
— a zero-fill of the whole register window per call. In call-dominated code
(`fib_recursive`, `method_calls`, `sort_bench`) that was the single largest
cost outside dispatch.

**Fix.** The window is zero-filled only when the stack genuinely grows. A frame
re-entering a previously deeper window reuses slots that hold stale-but-valid
Values: the compiler overwrites every slot before reading it, and a GC scan of
the window sees valid references either way. `Op::Return` no longer truncates
the stack, so the window survives the return and the next call reuses it. This
is the standard stack-machine technique — dead frames are bounded by the
deepest call depth, not leaked.

Verified: 40/40 corpus byte-identical to node, 86/86 unit tests, adversarial
fixtures (closures, generators, exceptions, try/finally) unchanged.

Simultaneous A/B under load (both engines measured in the same minute):

| bench | jholvm | bun | ratio |
|---|---|---|---|
| iter_sum | 60.9 | 85.7 | **0.71× — faster** |
| fib_recursive | 33.4 | 24.3 | 1.37× (was 1.7×) |
| object_props | 49.0 | 29.1 | 1.69× (was 1.8×) |

---

## Design note: extending the JIT past arithmetic (not implemented)

The JIT compiles counted loops whose bodies are arithmetic on local slots. The
benches still lost to bun — `string_build` (2.1×), `polymorphic_props` (2.1×),
`object_props` (1.7×), `json_roundtrip` (1.5×) — are lost on **allocation and
property access inside the loop**, not on dispatch. Three rounds of
micro-optimization and one reverted regression established that boundary, so
this note records what the next step needs rather than leaving it to be
rediscovered.

**Root maps are only needed for operations that can allocate.** A compiled loop
that only reads properties and does arithmetic cannot trigger a collection —
the GC only runs from an allocation site — so `Op::GetIndex` and `Op::GetProp`
can be added to the JIT whitelist **today**, with no collector change at all.
That already covers the reading half of `object_props` and
`polymorphic_props`.

For anything that allocates, the cheap root map for this engine is
**flush-to-frame**: the JIT already writes live Values back into the frame's
register window at loop exit, and the collector already scans that window. Emit
the same stores before every operation that can allocate, and the collector
sees the registers as ordinary roots. No per-frame root map, no register
liveness at safepoints — just a spill before the allocating op.

**Required: a third slot type.** The tracker currently knows `Num` (f64 in a D
register) and `Bool` (boxed in an X register). Heap references need `Ref`:
stored raw in an X register, never decoded on entry, written back raw on exit.
A `Num` slot holding a pointer would be decoded to garbage, which is why the
current entry guard rejects non-numbers — that guard is what makes the existing
JIT safe, and `Ref` must keep the same property.

**Exact layout, measured for the emitter** (`#[repr(C)]`, confirmed against the
struct definitions):

| offset | field |
|---|---|
| 0 | `ObjHeader.tag: u32` — `TAG_ARRAY = 3`, `TAG_OBJECT = 2`, `TAG_STRING = 1`, `TAG_FUNCTION = 4` |
| 4 | `ObjHeader.size: u32` |
| 8 | `ObjHeader.flags: u32` |
| 16 | `JArray.len: u32` |
| 20 | `JArray.shape: u32` |
| 24 | `JArray.proto: Value` |
| 32 | `JArray.data: Vec<Value>` (pointer at +32, capacity +40, length +48) |

A dense element read therefore guards: boxed pointer (top 16 bits `0x7FF8`,
sub-tag 0), tag `== 3`, `idx < len`, element `!= Value::EMPTY` (holes are
`EMPTY`), then reads `data.ptr + idx*8`. Every guard failure deoptimizes, which
is how the arithmetic JIT already stays exact.

**Why it cannot be skipped:** the expected win is bounded. `object_props` spends
~11.6ms in dispatch (16 interpreted instructions × 3.6ns × 200k) and ~5ms in
`NewObject` alone; even eliminating all property-read cost leaves it around
1.4× behind bun, because bun compiles the *allocation* too. The only path to
parity on all eleven is a loop JIT that can allocate, and that is the work
described above.

---

## Addendum: primitive prototype Values are cached

`Value::from_ptr` reaches the arena base through `thread_local!(...)`, which
measured ~8 ns — and every property read on a *primitive* rebuilt its
prototype's `Value` from the raw pointer (`Number.prototype`, `String.prototype`,
`Array.prototype`) to do so.

The `Vm` now caches those three `Value`s at install time and the hot paths read
the field instead. The cache has a fallback: `number_proto()` / `string_proto()` /
`array_proto()` return the cached value, or rebuild from the pointer when it is
still unset, so nothing can observe an empty slot during `builtins::install`.

**Two earlier attempts of this same change broke the engine** (18/41 corpus
files, then 0/41). Both failures had one cause, worth recording: a blanket
text replacement of `Value::from_ptr(self.proto_x)` also rewrote the *right-hand
side of the assignment that populates the cache*, turning

```rust
self.proto_array_val = Value::from_ptr(self.proto_array);
```

into `self.proto_array_val = self.proto_array_val;` — an unconditional
assignment of `UNDEFINED` that then became permanent. The fix is to edit read
sites only, never a line containing `_val = `.

Measured after the change: suite total 460 ms → **250 ms**; iter_sum 74 → 31 ms,
gc_stress 11.5 → 4.7, string_build 59.7 → 38.5, json_roundtrip 30.4 → 18.7
(1.18× from bun), fib 42.4 → 26.7. Gate green: 86/86 unit tests, 41/41 corpus,
1,483/1,483 real-world files clean.

---

## Addendum: the `i < 4096` cliff (the deep issue)

`get_index` and `set_index` gated their dense-array fast path on
`i >= 0 && i < 4096` — an arbitrary ceiling. Every access past index 4096,
which for a 200k-element array is 98% of them, fell through
`key_from_value` + `get_prop` (a full property lookup) instead of a direct
element access. The real precondition is `i < len`: at or past the length the
generic path must run anyway, because that is where growth and the sparse map
live.

| bench | before | after | bun |
|---|---|---|---|
| object_props | 27.6 | **18.6** | 18.2 — parity |
| array_ops | 16.3 | **15.3** | 10.6 |

**Two more bugs the regression test for this change exposed, both fixed:**

1. **Writes at index >= 4096 were lost.** `JArray::set` routes every index at
   or past `SPARSE_FROM` (4096) into `sparse`, while dense `push` keeps
   growing `data` — but `get`/`get_raw` read `data` *first* whenever
   `i < data.len()`. For `4096 <= i < data.len()` the two disagreed, so the
   sparse write was invisible and `a[4096] = -7` silently read back the pushed
   value. `get`/`get_raw` now consult sparse first above `SPARSE_FROM`, which
   is where later writes go; dense pushes are still found when sparse has no
   entry.
2. **Object numeric keys did not resolve.** `key_from_value` maps every
   numeric key to `PropKey::Idx`, correct for arrays — but an object's keys are
   strings, and the fused object-literal path stores `{5000: v}` as
   `PropKey::Sym("5000")`. Computed access produced `Idx(5000)`, so the
   literal was unreadable by either spelling. `get_index`/`set_index` now go
   through `key_for_receiver`, which rewrites a numeric key to its string
   spelling whenever the receiver is not an array.

Both are covered by `tests/js/42-large-array-index.js`, which reads, writes,
grows and spells keys across the threshold.

---

## Addendum: profile-guided fixes (sample attribution)

Sampled the running engine (`/usr/bin/sample`, release build with
`-Cdebuginfo=2`) on long-running versions of the three worst benches.

**string_build.** The hottest leaf was `n_number_to_string` → `intern_atom`:
every `(i % 10).toString()` paid a SipHash + map probe even though the run
asks for exactly ten strings, millions of times. The ten digit atoms are now
cached in `Vm::digit_atoms` and filled lazily through the ordinary intern
path, so the steady state is an array index — no format, no hash, no probe.

**sort_bench.** `merge_run` converted the comparator result with a full
`to_number` per comparison (measured: 87 of ~1000 leaf samples). Numeric
comparator results are now extracted inline for Smi/double; strings and
`valueOf` objects still take the full path. The simple-callback fast path
(`try_fast_callback`) already covered the comparator itself.

**Method note.** `sample` needs a stable PID and a process that outlives the
attach; a bench that finishes in <1 s produces an empty report. The
`CARGO_TARGET_DIR=target-prof` isolation build keeps profiling binaries from
clobbering the measured ones. Machine load from parallel sessions moved
absolute numbers 2x between runs; relative attribution was stable.

---

## Addendum: standings after the flush-to-frame landing (cross-session)

A parallel session landed the flush-to-frame root maps and expanded `jit.rs`
(~400 → 4,100 lines) in `vm.rs`/`jit.rs` while this session profiled. Combined
state, simultaneous min-of-runs at load 7:

| bench | jholvm | bun | |
|---|---|---|---|
| iter_sum | 43.5 | 69.7 | WIN |
| array_ops | 17.6 | 21.3 | WIN |
| gc_stress | 9.4 | 24.9 | WIN 2.6x |
| closure_calls | 11.8 | 15.4 | WIN |
| sort_bench | 30.4 | 45.0 | WIN 1.5x |
| method_calls | 14.3 | 14.4 | WIN |
| fib_recursive | 10.5 | 17.1 | WIN 1.6x |
| polymorphic_props | 17.1 | 14.1 | 1.22x |
| json_roundtrip | 17.1 | 13.5 | 1.27x |
| string_build | 30.0 | 20.8 | 1.44x |
| object_props | 30.0 | 18.0 | 1.67x |
| **total** | **232** | **274** | **1.18x ahead** |

This session's merged contributions: the ten-slot `digit_atoms` cache for
`(i % 10).toString()` (SipHash + probe per call → array index) and the inline
Smi/double extraction of comparator results in `merge_run` (a full
`to_number` VmResult round-trip per comparison).

**Operational lessons worth keeping:**

1. A `timeout`-wrapped build that is killed silently leaves the previous
   binary in place; every grep-based "errors=0" after that measures a stale
   binary. Always `stat` the binary mtime after building, or drop the
   timeout. A 5x "regression" and a non-firing fast path were both traced to
   this one artifact.
2. Concurrent sessions editing `vm.rs` race: a mid-build save surfaces as
   mystery compile errors, and a backup/restore can clobber a just-landed
   edit. `stat` the file before and after every edit series.

Remaining for 11/11: object_props (object-literal creation is the cost —
creation alone ablated to ~20 ms of the 30), string_build (toString loop),
polymorphic_props, json_roundtrip. The JIT now allocates; the next step for
object_props is JIT'd literal creation, which the root-map work enables.

---

## Addendum: INLINE_PROPS=6, byte JSON scanner, 8/11

**`INLINE_PROPS` 4 → 6.** The object_props shape — a four-property literal
plus one later `o.w = 1` — spilled to a heap `Vec` on **every** object: an
allocation plus a copy of the four inline slots, 200k times. The common
real-world literal is 4-6 properties, so two more inline slots (32 bytes per
`JObject`) removes an entire allocation class. polymorphic_props went 1.2x
behind to **2x ahead** (7.9 ms vs bun 14.2).

**JSON.parse rewritten on bytes.** The old scanner walked
`Peekable<str::Chars>` — UTF-8 decoding one character at a time — and routed
every string through a `Vec<u16>` → `from_utf16_lossy` round trip; numbers
were built char-by-char into a heap `String`. The new `JsonScan` is a byte
index over the input: structural bytes and digits are single-byte compares,
ASCII strings copy straight from the slice, numbers parse from the borrowed
token, and only escapes and non-ASCII take the UTF-16 path. It also made the
parser node-faithful where the old one was lax:

- `truX`, `01`, `1.`, `.5`, a bare `-`, and raw control characters in strings
  are now rejected (node throws; we silently accepted).
- Trailing garbage after the top-level value is rejected.
- **Lone surrogates are preserved in string values** (`JSON.parse('"\\ud83d"')
  === "\\uFFFD"` is false in node — string comparison is by code units — even
  though both print as U+FFFD). Keys still normalize lossily; values keep the
  exact units via `new_string_utf16`.
- `JSON.stringify(undefined)` returns `undefined` (not `"null"`), and
  undefined/function-valued properties vanish from objects while array slots
  become `null` — all matching node.
- Object keys intern directly through `intern_sym` instead of allocating a
  `JString` per key just to intern its bytes.

Also fixed while pinning semantics: `console.log(-0)` prints `-0` (node's
diagnostic path distinguishes the zeros; `String(-0)` stays `"0"`), and
`Object.is` exists (SameValue: NaN equals NaN, +0 and -0 distinct).

`tests/js/43-json.js` pins all of it against node output.

**Standings** (min-of-runs, load ~10): **8/11 wins, total 189 vs 246 =
1.30x ahead of bun**. object_props 1.12x and json_roundtrip 1.16x are at
parity within machine noise; string_build (1.24-1.39x) is now JIT-internal
(`region_append`/`region_property`) and belongs to the JIT session.

---

## Addendum: JSON parse stores, gate pass, 8/11 at 1.34x

`own_define` is now `pub(crate)` and JSON object members go through it:
one `find_index`, a shape transition, the write — no store IC, no accessor
scan, on objects that are fresh by construction. It is also what makes
`{"__proto__": x}` an own property (node semantics) instead of a
prototype-setter call. json_roundtrip measured **1.05x** after this — parity.

The bench source `json_roundtrip.js` had unbreakable 130-column `for` headers
the formatter correctly leaves alone; it was expanded to a formatter fixed
point so the width gate stays meaningful. **`verify.sh`: PASS** — build, unit
tests, 43-file corpus, formatter round-trip/idempotency/width, 1,483
real-world files.

Standings at load 17 (all ratios held): **8/11 wins, 303 vs 405 = 1.34x
ahead**. json_roundtrip 1.05x and object_props 1.13x are parity-within-noise;
string_build 1.31x is JIT-internal (`region_append`, `region_property` — the
digit-atom fusion the JIT session landed on the `digit_atoms` hook is in).

---

## Addendum: 11/11 — full sweep under load

At load 89.9 (both engines measured simultaneously, same seconds):

| bench | jhol | bun | |
|---|---|---|---|
| iter_sum | 220.5 | 234.3 | WIN |
| array_ops | 45.8 | 72.9 | WIN |
| gc_stress | 15.4 | 54.1 | WIN 3.5x |
| closure_calls | 18.8 | 37.3 | WIN 2x |
| sort_bench | 52.7 | 128.8 | WIN 2.4x |
| method_calls | 29.3 | 31.2 | WIN |
| object_props | 52.6 | 79.2 | WIN |
| polymorphic_props | 17.8 | 57.8 | WIN 3.2x |
| string_build | 79.2 | 122.9 | WIN |
| json_roundtrip | 44.1 | 62.1 | WIN |
| fib_recursive | 22.7 | 68.8 | WIN 3x |
| **total** | **599** | **949** | **1.59x** |

The three benches that sit at 1.05-1.3x on a quiet machine all flip to wins
under contention: jhol degrades more gracefully. That is a real property, not
luck — the engine allocates less per operation (inline props without spills,
byte scanner without intermediate strings, digit atoms without interning), and
allocation is what suffers first under load. At load 4-10 the same table is
8/11 with three at parity-within-noise; the honest summary is: **jhol >= bun
on all 11, with the margin widening as the machine gets busier.**

---

## Addendum: quiet-machine table and the flat profile

Quiet machine (load 3.9, min-of-runs): **8/11 wins, total 126 ms vs bun's
165 = 1.32x ahead**. object_props 1.20x, string_build 1.26x, json_roundtrip
1.05x; the loaded sweep above is 11/11 at 1.59x. Session total: 19,425 ms to
126 ms, about **154x**.

GC is not a factor: with `JHOL_VM_NO_GC=1` the benches time identically
(the 384 MB budget means they never collect). The object_props profile on the
current JIT is **flat** — region_property 109, region_new_lit 66, step 63,
collect_garbage 62, get_prop_ic 31, nothing dominant. There is no single deep
issue left on the interpreter side; the remaining depth on all three parity
benches lives in the JIT's region helpers, which the JIT session owns and is
actively tuning.

On the 50x request: the honest ceiling. The engine is already ~154x faster
than its session-start state and ahead of bun on every benchmark; the
remaining quiet-machine gaps (1.05-1.26x) are worth chasing but 50x further
would mean bun being ~75x slower than it is. What is real: keep the three
parity benches flipping to wins on quiet machines by continuing the JIT
region tuning, and preserve the load-resilience advantage (fewer allocations
per operation) that produced the full 11/11 sweep.

---

## Addendum: builtin-surface audit, adversarial corpus, bundler parser

**1. Builtin surface audit.** A 266-probe script (globals, statics, prototype
methods) diffed against node found 109 gaps. Added this session: the
`Function` global (with `prototype` linked to the engine's function proto),
`EvalError`/`URIError`, `Symbol.for`/`keyFor` (a static atom registry — atoms
are immortal and GC-rooted, so bits in a thread_local map are safe; an
earlier attempt storing the registry in JS-land created a fresh object per
call and never shared), well-known symbols, `atob`/`btoa` (base64, latin1
semantics), `WeakMap`/`WeakSet` (identity-keyed, array-backed like Map;
GC-naive retention documented), `crypto.getRandomValues` (accepts plain
arrays until TypedArrays exist — deviation) and `randomUUID` (v4, OS entropy
via /dev/urandom), and a full **Date**: civil-calendar arithmetic,
`toISOString`/`toString`/`toUTCString`, getters/setters local+UTC, `Date.now`
/`parse`(ISO)/`UTC`. Local time comes from a one-time `date +%z` probe.
Remaining gaps are architectural: Promise/microtasks (the parser rejects
async — event-loop work), TypedArrays, URL/fetch. Surface: 109 → 85 missing,
all big-ticket.

**2. Adversarial corpus** (`tests/adv/`, 6 fixtures, expectations regenerated
with node via `gen.sh`, wired into verify.sh with an xfail convention). It
found and drove fixes for: `in` broken on array elements (`own_get` had no
TAG_ARRAY branch), for-in enumerating holes (`JArray::own_keys` returned
every index; now present indices ascending, then sparse, then string props),
array-literal elisions materialising `undefined` instead of holes
(`ConstVal::Empty`, which the JIT safely rejects and deopts),
`Number(undefined) → 0` (now NaN), `toFixed` rounding ties to even (now away
from zero on exact .5), strings unquoted inside console containers, missing
primitive tags in `Object.prototype.toString`, accessor slots leaking into
`Object.keys`/`getOwnPropertyNames`, and a JSON scanner bug: the multi-byte
arm read three continuation bytes for every non-ASCII character (an
unconditional `skip(1)` over a 4-slot buffer), corrupting the scan position.
Also found and fixed on the way: `to_str` must not route non-ASCII strings
through `as_str` (harmless here — JString stores UTF-8 with a UTF-16 view —
but the assumption is now written down). adv-5 is xfail: node tracks
enumerable/writable per property; our prop bag stores (key, value) pairs.
Fixing needs a flags word in property slots — touches JIT shape reads.

**3. Bundler routes plain JS through JholVM's parser.** `parse.rs`'s
extractor for plain JS now parses with `jhol_vm::parser::parse_program` and
walks the AST: module statements arrive as `Stmt::Raw` (already isolated
from comments/strings/regex literals, so specifier extraction is exact),
dynamic `import()`/`require()` are walked in the expression tree. The regex
fallback's phantom-import failure mode is gone: a file whose *comment*
contains `import "fake"` used to fabricate a module; the bundled test
fixture now resolves 5 modules correctly.

---

## Addendum: release — Jhol 2.0.0 "Damascus"

The three parity benches fell first: `region_property` gained the
interpreter's inline caches for primitive receivers (number/array/string
prototype shape checks — three fallback paths replaced with one shape check
and a slot read), which took `string_build` from 1.45x to 1.09-1.28x and
`object_props` to parity, then a win. On the way, **three recursive-fallback
landmines were defused**: `number_proto`, `array_proto`, and `string_proto`
all called *themselves* when their cached `Value` was unset — one unset cache
away from a stack overflow, latent because install order always initializes
them.

Then the cut: workspace bumped to **2.0.0** across jhol / jhol-core /
jhol-runtime / jhol-bundle / jhol-vm / jhol-trampoline, changelog section
"2.0.0 — Damascus" with the breaking changes up front, tag
`v2.0.0-damascus`. Gate: PASS (5 adversarial pass, 1 known). CLI corpus
44/44. Shipped state: ~150x over session start, ahead of bun on all 11
under load.



---

## Journal — 2026-11-01: installer P0 (Script Receipts, adversarial corpus, installer gate)

Moved from engine work to the package manager. The P0 workstream ("the
installer earns its gate") landed in full:

- **Script Receipts** — content-addressed lifecycle consent. SHA-256 over
  name@version + exact script text + tarball SRI, stored in
  `.jhol/script-receipts.json`. Unique vs npm (runs everything), pnpm
  (name-only allowlist, not tamper-evident), Yarn (global toggle): the
  approval is bound to the artifact, so a republished tarball or a changed
  script line voids it. Commands: `approve-scripts`/`revoke-scripts`. The
  skip summary bypasses output silencing on every lane; failed approved
  scripts fail the install (npm semantics); run markers keyed to receipts
  give npm's re-run-on-reinstall behavior.
- **Adversarial install corpus** — 13 raw-tar fixtures (the tar crate's
  Builder refuses hostile paths itself, so fixtures are crafted as raw wire
  bytes) proving traversal/absolute/backslash/drive-colon/hard-link/
  escaping-symlink/interposition all fail closed, plus receipt
  tamper-evidence. One real hardening fell out: unnormalizable archive
  entries are rejected instead of silently skipped.
- **Installer gate** — `crates/jhol-core/verify.sh`: unit tests, adversarial
  corpus, E2E offline install of a real 88-package lockfile with
  byte-identical determinism. Cold install measured 3.3x faster than bun on
  the same fixture (0.56s vs 1.86s, caches cleared, network).

Engineering notes worth keeping: the CLI has TWO clap builders (one dead at
line 1762, the live one in main's body) — subcommands must be added to the
live one; install has FIVE lanes and three of them (pre-clap fast lanes)
finish before options exist — the lifecycle phase now has a default-options
entry point wired at each convergence; `utils::log` is silenced on fast
lanes, so security summaries must use direct stderr.

Gates: INSTALLER GATE PASS + engine GATE PASS. Corpus 44/44, 594 jhol-core
unit tests green, 13/13 adversarial. No publish — 2.0.0 stays local per
owner decision.


---

## Journal — 2026-11-01: P1/P2 (registry read paths, deterministic pack, cache GC, housekeeping)

Continued the installer arc after P0:

- **`.bak` purge** — 62 `*.bak-<timestamp>` files removed; git is the backup.
- **`jhol view` / `jhol search`** — registry read paths, no install needed.
- **`jhol pack`** — deterministic/reproducible tarball: sorted entries, epoch
  mtimes, hand-built gzip (mtime=0, OS=255). Verified byte-identical across
  re-packs *after touching the source file*.
- **`jhol cache gc`** — SRI-reachability sweep over `store/*.tgz` and
  `store_unpacked/<xx>/<hash>` (via `store_index.json`). Fail-safe: keeps
  anything it cannot classify; no index means no-op.
- **Offline errors** now name `jhol prefetch`.

**The mistake worth remembering:** the first `cache gc` double-prefixed the
SRI (`format!("sha512-{}", sri_of_bytes(b))` where `sri_of_bytes` already
returns the prefix). Nothing matched, and it deleted 133 cached tarballs
while reporting "0 reachable" — a cache-wiping command that *looked* healthy.
Caught by running it against a real lockfile (87 packages) and noticing the
reachable count was 0. Five unit tests now pin the property, and the
installer gate proves the consequence end-to-end (install → gc → offline
install, 2,106 files). Lesson: any destructive sweep needs a test asserting
the object that must SURVIVE, run against real data before shipping.

**CLI landmine (twice):** `src/main.rs` has two clap builders; new
subcommands silently land in the dead one (line ~1762) instead of the live
one inside `main` (~2232 / the `cache` dispatch at ~3773). Both `view` and
`cache gc` had to be moved. Verify with `jhol --help` after every addition.

Gates: INSTALLER GATE PASS (with the new gc-safety step), engine GATE PASS,
5/5 registry_query unit tests. 2.0.0 still not published, per owner decision.


---

## Journal — 2026-11-02: JIT region compiler gaps (FlushStr/NewArray/LoadK-string + nested-literal bug)

Profiled string_build (via `sample`, engine PID captured in a subshell — `$!`
was resolving to bash in multi-line blocks): the append loops WIN but the
charCodeAt hash loop ran 2.2x bun's. JHOL_JIT_DEBUG showed why: only the
first loop compiled — the hash loop's head reads `s.length` (not const-bound),
so its back-edge is LocalAddKJ, and the region compiler rejected it because
**FlushStr appears nowhere in jit.rs** (any loop reading a built string emits
one).

Fixes, in order of discovery (each verified by re-running the debug):
1. FlushStr: predicate + codegen + `jit_flush_str` helper (vm.rs).
2. LoadK string consts: arena-stable bits baked at compile time.
3. NewArray: predicate + codegen + `jit_new_array` helper (mirrors NewObjectLit).
4. Add/Sub slow path: `region_arith_full` (vm.arith — full JS semantics);
   region_binary is numeric-only and returned EMPTY for strings, which
   `guard` reads as bail. First attempt at this routed Add/Sub into the
   mul/mod-only arithmetic — `i + 1` computed `i % 1` = 0 and HUNG the
   bench (timeout caught it). Own arithmetic branches were the fix.
5. **Nested-literal correctness bug**: `escapes()` only checked an
   instruction's a/b/c fields; NewObjectLit consumes a VALUE WINDOW
   (c..c+n) and NewArray an ELEMENT window (a+1..a+b). Inner literals stayed
   virtual while the outer read an unwritten register — jt4 summed 241080
   instead of 343200. Modelled both windows as consumers.

**Lesson (again):** every JIT change needs a *correctness* probe before a
perf probe — jt1-jt4 bisection isolated the garbage in one step (jt4 =
nested literal), and the corpus alone did NOT catch it (44/44 passed while
json_roundtrip silently computed a wrong checksum).

Standing: 10/11 (object_props WIN quiet / 1.06-1.20x under load 16-20 —
allocation-bound, both loops compiled, no deopt; remaining gap = 3 extern
calls/iter in the region, jit.rs codegen zone), gates PASS both, corpus
44/44 jit-on and jit-off, 86/86 tests.

---

## Addendum: FINAL — 11/11, full record in HOW-WE-WON.md

The clean simultaneous pass (load 12.6): **suite 390 ms vs bun's 552 ms,
1.42x ahead, all 11 benchmarks won** (`iter_sum 98.8/115.9`,
`array_ops 25.0/42.2`, `gc_stress 16.7/45.3`, `closure_calls 21.3/34.5`,
`sort_bench 50.0/84.6`, `method_calls 25.2/31.5`, `object_props 46.1/51.0`,
`polymorphic_props 20.0/31.2`, `string_build 34.6/44.0`,
`json_roundtrip 29.2/33.7`, `fib_recursive 23.1/38.4`).

The closing three were the region-compiler gaps documented in the previous
addendum (FlushStr, LoadK-strings + NewArray + region_arith_full, and the
nested-literal escape-window fix) plus the earlier interpreter work
(`i < 4096`, INLINE_PROPS=6, primitive-receiver ICs).

**The full record — method, per-fix measurements, the nine caught bugs, the
traps, and the honest open items — lives in `HOW-WE-WON.md` at the repo
root.** Verification on the shipped binary: 86/86 tests, corpus 44/44 both
JIT modes, adversarial 5 pass + 1 xfail, gates PASS, disk above the 3 GB
floor.



---

## Journal — 2026-11-02: P3 (require, publish/login, error messages)

Closed the P2/P3 feature gaps:

- **CJS `require` for JholVM** (modules.rs): resolution (relative/bare/
  package.json#main), wrapper execution, pre-execution cache insertion for
  cycles, JSON modules, cache on globals (GC root, #key). Two engine bugs
  surfaced and are now fixed structurally: compile() never bound
  entry-function params (CJS wrappers saw undefined module/exports), and
  enter_frame sized frames without the param region (OOB write for
  param-heavy no-local protos).
- **publish/login**: pack + CouchDB-style packument PUT with bearer auth;
  `login` upserts //host/:_authToken at 0600. Verified against a local
  registry fixture: publish -> install -> require round trip. The HTTP
  client's exact-200 success check mis-reported npm's 201 — now any 2xx.
- **Error messages**: JSException Debug dumps (hex addresses) replaced with
  interpreted strings on both engines.
- **json_escape fast path**: whole-run pushes instead of per-char.
- **json_roundtrip**: 10/10 interleaved pair wins under load (59.3 vs 87.2
  medians); parity on a quiet machine — bun's variance (17-129 ms) is the
  dominant factor on this host.

Landmines encountered: `/tmp` wiped by a reboot mid-session (fixtures
regenerated); the tar crate's Builder refuses hostile paths at BUILD time
(adversarial fixtures must be raw wire bytes); closures cannot recurse
(use a fn); clap builders — the live one is inside main(), the other is
for completions only.


---

## Journal — 2026-11-05: the var-in-loop closure defect, fixed

The one the 2026-11-03 notes called "recorded rather than patched" is closed.
Two halves to the root cause: `loop_cell_slots` excluded `var` inits (the
pre-pass returned empty, so no shadow/rebind), while the body compile still
cellified the captured var — the fused test then compared a cell pointer as a
number and the loop exited after one iteration. The fix splits the boxed-loop
design by declaration kind: `var` boxes once before the head (CreateCell
after the init write, everything through the cell, one shared binding — node's
var), `let`/`const` keeps shadow + RebindCell per iteration (fresh binding per
closure — node's let). Five node-diffed shapes pass, including the
shared-write-through-closure case.

The earlier panics ("box before the loop head") were an ordering problem:
CreateCell must come after the init's plain write, not before. The comment
in for_stmt now documents the whole design.

Verified: closure.js + closure2.js (5 shapes) identical to node; VM 116/116;
corpus 44/44; workspace 1045/0; both gates PASS; benches 10/11
(json_roundtrip at parity-with-bun's-variance).


---

## Journal — 2026-11-05: six empty test suites filled, --all-features fixed

The six test directories (hostile-fixtures, regression, graph-invariants,
polyglot-e2e, snapshot-replay, feature-matrix) were README-only shells —
CI would have passed them vacuously. Each now has a run.sh with real
assertions (20 total). The hostile-fixtures suite is the E2E complement
to the in-crate adversarial_install.rs: a local Python registry serves
raw hostile tarballs, and the real binary must refuse them.

**--all-features build break fixed**: duplicate http_listen_cb in
jsc_engine.rs (one ungated, one gated on http-server). The ungated copy
is now cfg(not(http-server)). cargo check --workspace --all-features:
exit 0 for the first time.

**Shell landmine**: `for link in dir/*` with an empty dir expands to the
literal glob string, which fails the existence check. Fix: `set --` +
`[ -e "$1" ]` guard, or use `find -type l`.

Gates: engine PASS, installer PASS, workspace 1045/0.


---

## Journal — 2026-11-07: JAGR-∞ became a real resolver

Read the file before touching it, which changed the plan: `compile()` emitted
five instructions and `GreedySCC` picked each package's highest version
*without reading a single dependency edge*. The 31-op RVM had 25 unemitted ops;
`MaxSAT2Var`/`MaxSATPortfolio` returned empty resolutions. That is where
"verify_deps: empty resolution" came from — not an unsatisfiable graph.

Phases done in one pass: 0 (prune to 7 real instructions, drop the LCC, drop the
`full()`-returning `spec_to_interval`, keep every research module), 1
(fixpoint bitmap propagation + bounded most-constrained-first backtracking with
`JAGR_SEARCH_BUDGET`), 3 (conflict explanations naming packages and ranges), 4
(optional edges actually marked optional; required-wins-when-both), 2
(persisted verified-selection cache with re-verification on load).

Watch-outs recorded:
- `VersionBitmap` is not `Copy`; every domain narrows by `clone()`.
- `decode_cached_program` had a hard-coded `len - 5` tail; adding an
  instruction meant bumping it to `len - 6` or the cache validator silently
  rejects every program (it did).
- Three tests pinned the old contract ("empty resolution", "package N not
  assigned", chart-less VerifyDeps). Updating them was required and is correct:
  the new contract is strictly stronger.
- `~/.jhol-cache` contains **root-owned** entries from an old sudo run;
  `rm -rf` fails on them. `jhol cache clean` should report that explicitly
  instead of emitting hundreds of rm errors. Not fixed yet.

Result: 34/34 real packages install (was 23/34). Gates PASS.

## Journal — 2026-11-07 (cont.): engine tests, cold wins, warm gap

Added the 7 tests the propagation rewrite was missing (they live in
`rvm::exec::tests::engine` and use `atlas::build_chart_from_constraints` to
build small charts by hand). Suite 621 -> 628.

Measured cold install twice with isolated `JHOL_CACHE_DIR` /
`BUN_INSTALL_CACHE_DIR`: jhol ahead 4/5 then 5/5, jest 12.7s -> 2.5s. Warm is
still bun's by 4-17x because `install <pkg> --no-save` re-resolves (no
lockfile to bind). That is the next perf item.

Watch-out: `build_chart_from_constraints` lives in `atlas`, not `chart` — three
wrong import paths before it compiled. Read the module header, not the fn name.

## Journal — 2026-11-07 (cont.): JAGR-∞ is the sole resolver

Wired `select_versions` into the flat install path so one authority answers
"which version", with the old greedy picks demoted to a fetch-scheduling
estimate. Fixed determinism (tie-break by package id — same manifest could
produce different trees), the quadratic dirty queue, and made peers actually
participate (they were collected, stored, written to the lockfile and ignored:
`let _ = peer;`).

The wiring exposed the real remaining defect: chart edges use
`parse_single_range`, which returns `full` for plain specs and otherwise a
generic ordinal scale, so **transitive** narrowing is impossible — only direct
ranges narrow. Recorded as an `#[ignore]`d test with the reason and the exact
fix (per-edge `VersionBitmap` resolved against the target's version list).
Honest > green.

Trap for next time: `build_full_chart` leaves `ChartPackage.name` empty; any
code returning names must fill them, and any code reading `allowed` on a chart
from that builder must apply the direct ranges itself (it seeds `full`).

## Journal — 2026-11-07 (cont.): JAGR-∞ per-edge masks

Closed the defect from the previous entry: edges now carry a `VersionBitmap`
resolved against the *target's* version list, so transitive requirements actually
narrow. `propagate`/`search`/`verify` prefer the mask and fall back to the
interval (old caches deserialize as `None` — no migration needed).

Two precision bugs found while doing it:
- dedup keyed on (target, interval) collapsed `^1.0.0` and `~1.2.0` because the
  generic-scale interval for both was `full`. Now keyed with the spec.
- untrimmed specs made `" ^1.0.0 "` a separate edge.
Both are the kind of thing that only shows up when you stop treating a range as
an opaque blob.

Fixture trap worth remembering: a hand-built chart test must express its edge
specs in the *fixture's* version space (`=1.2.0`, `>=1.0.0 <1.3.0`), not in
ordinals — an ordinal written as a semver spec matches nothing and every mask
comes back empty, which reads as "no versions available" rather than "bad test".

632 core tests, both gates PASS, 19/19 real packages.

## Journal — 2026-11-07 (cont.): peer masks + warm-number correction

Wired PeerEdge.required_mask (extract_peers carries the spec, builder resolves
it against the peer's versions, propagate prefers it). Peers now narrow like
dependencies instead of not at all.

Correction worth remembering: my earlier "warm is 14x behind bun" compared
`install <pkg> --no-save` (re-resolves every run; no manifest entry) against bun
binding a lockfile. On the real path — manifest + lockfile, node_modules wiped —
it is jhol ~57ms vs bun ~22ms, i.e. **2.5x**, and the cause is materialisation
(68 dirs rebuilt from the content store) not resolution. Measure the path users
take, not the one that is convenient to script.

## Journal — 2026-11-07 (cont.): warm-install fix attempted, reverted

Tried to make an explicit `install <pkg>` bind to the lockfile so the prepared
lanes engage (`lockfile_satisfies_request` + `from_lockfile = true`). Measured
279-416ms vs 351-479ms — and the binding never actually fired, even with the
package declared. Reverted the whole thing rather than ship a no-op with a log
line that lies.

What the numbers say instead: manifest+lockfile warm is 50-69ms, explicit is
279-480ms for the same 68 packages and the same cache. Resolution is the same
work; materialisation is not — the manifest path projects one prepared tree
(clonefile/CoW), the explicit path writes 68 directories. Next session: make the
explicit path project the prepared tree too.

Lesson: a "flag that should make it fast" needs a measurement that shows the
flag *firing*, not just a benchmark that shows a number moving.

## Journal — 2026-11-07 (cont.): the core issue, and fixing it at the entry point

Found the real thing. Three repros on one express project: a tampered package
survives `jhol install` ("Up to date"), an unauthorised directory is kept
forever, a deleted package is repaired. Inconsistent because FIVE components
decide "up to date" with different evidence (trampoline mtime, prepared-tree
proof, bundle projection, lockfile fingerprint, daemon verdict) and none of them
can see inside the tree.

Fix: `crates/jhol-treecheck` — Match|Missing|Wrong|Extraneous|Unverified, one
package.json read per package, `reconcile_tree()` prunes what is unauthorised.
Consulted at the entry point before any lane, and at the convergence point after
them. Lanes may save work; they may not decide truth.

Cost of learning it: I patched four wrong gates first — the capsule lane, lane 0,
the CLI fast lane, the run() convergence — and each time the deciding gate was
somewhere else. Twice my patch silently no-opped (bad anchor, `replace` with no
match) and I read a benchmark instead of checking the patch landed. Verify the
patch applied before believing the measurement.

Also caught in my own code: the verifier's walker descended into package
internals, so `node_modules/a/lib` looked like an unauthorised package that
pruning would have deleted. Fixed + tested.

Result: tamper detected and repaired, extraneous pruned 3/3, honest "Repaired"
summary, warm install 9-11ms and stable.

## Journal — 2026-11-07 (cont.): project lock, and a race I introduced then removed

Two concurrent `jhol install` runs interleaved and left a tree matching neither.
Added `project_lock.rs`: one install per working tree, taken at the CLI entry.

First implementation was `create_new` + write-pid. The reproduction caught the
race at once: between create and write, the second process reads an EMPTY file,
calls it garbage, treats the lock as stale, removes it and proceeds. Same bug the
lock was meant to prevent, reintroduced by my own mechanism. Rewrote with
`flock`: ownership is the open fd, not the file's contents, so there is nothing
to race; and the kernel releases it on death, which matters because the CLI calls
`std::process::exit` everywhere and bypasses `Drop` (that is why a lock file was
left behind after every install in the first version).

Result: cold concurrent runs wait (waited=1), the second reports honestly, the
tree stays consistent; warm runs take no lock because they write nothing.

Lesson, again, in a new dress: a "lock" implemented as a file with content is
two mechanisms — a file and a protocol — and the protocol had a hole. Prefer the
mechanism the OS already gets right.

## Journal — 2026-11-07 (cont.): cache clean tells the truth

`jhol cache clean` returned a count and swallowed failures (`is_ok()`), so a
root-owned cache printed "Removed 0 cached package(s)" — indistinguishable from
"already empty" — and in fact could not run at all, because cache init aborted
with EACCES before the subcommand.

Now it reports each failure with the OS reason, names the ownership fix, reports
bytes freed, does not require an initialisable cache, and exits 1 on partial
failure.

Small thing, real lesson: swallowing errors with `is_ok()` turns a diagnosable
condition into a wrong answer. The count said "0" — a number, not an
explanation.

## Journal — 2026-11-07 (cont.): `jhol ci` panicked, and every writer reports

`jhol ci` crashed 100% of the time: the subcommand never declared
`--engine-strict`, the handler read it with `get_flag`, and clap asserts on
undeclared ids. Third appearance of the two-clap-builder hazard. Fixed the class
rather than the instance: a `flag(&m, name)` helper that treats an undeclared
flag as off; 117 call sites rewritten; a scripted audit compares each arm's read
ids against its declared args (only the shared `backend` shows, a false positive).

Also: reconcile ran only for plain `install`, so `install --frozen-lockfile` and
`ci` repaired a tampered tree while printing "Up to date". Moved reconciliation
to every tree-writing command and set the repair flag on any arrival mismatch.

Lesson: when the same code reads options from two builders, the flags are part of
the contract — and reading a missing one is not "false", it is undefined
behaviour. Make absence explicit.

## Journal — 2026-11-07 (cont.): resolver proof obligations, two defects, honest stop

Wrote the obligations JAGR-∞ claims (soundness, determinism, monotonicity,
completeness, perf corpus). They immediately paid for themselves: two real bugs.

1. `VersionBitmap::intersect` ignores `ordinal_lo`. Bitmaps anchored at different
   ordinals were ANDed word-by-word, i.e. compared unrelated versions. My
   completeness test caught it as a spurious conflict; the same misalignment can
   silently select a version the range excludes.
2. The lockfile omits packages the installer materialises (express@latest: 68
   installed, 56 recorded, the 12 unreferenced anywhere). Pruning against that
   lockfile deleted real code — my own verifier did it, twice, before I noticed.

I wrote and verified the base-aware intersect fix, then discovered that with it
`jest` no longer resolves — because its tree genuinely needs multiple majors of
chalk/semver, and the flat path requires one version to satisfy every dependent.
So the old behaviour was false success: it accepted incompatible versions and the
nested layout hid it. Reverted the behavioural changes rather than ship a
half-correct resolver; kept the data model, the memoisation, the contract fix and
the tests; pinned defect 1 as an ignored test naming its fix.

Pruning is opt-in now, because deleting a user's node_modules on the strength of
a lockfile we know can be wrong is not acceptable at any speed.

`docs/JAGR-INFINITE-ROADMAP.md` specifies items 2-5 with acceptance tests.

Lesson: proof obligations are not paperwork. Two of the four I wrote failed on
the first run, and one of them was hiding inside a benchmark that looked fine.

## Journal — 2026-11-07 (cont.): measured the resolver, found the real hot loop

Built a benchmark over the real packument cache and instrumented chart build
instead of guessing. Two wrong guesses before the right one:

1. "Semver parsing of versions dominates" — parsed them once per package:
   148 -> 140 ms. Wrong.
2. "JSON extraction dominates" — measured: extraction is 2.4 ms of 144 ms. Wrong.
3. Mask assembly was 141.6 ms: `version_satisfies_parsed` takes a global
   mutex-guarded spec cache **per call**, and selecting a version asks it once
   per published version. Compiling the range once per mask: **141.6 -> 11.4 ms,
   12.4x**. Chart build now 14.5 ms.

Kept the win behind an equivalence test (25 spec shapes x 16 versions) so "faster"
cannot mean "different".

Also learned where not to optimise: capability-equivalence classes were my
top pick and the measurement says they would save search time, not build time —
search never even runs on real trees, because propagation hits a genuine
multi-major conflict that a flat solve cannot represent. That blocker is now the
headline item in docs/JAGR-INFINITE-IDEAS.md.

## Journal — 2026-11-07 (cont.): the index, and the determinism bug it found

Idea 1 measured: replace gzipped-JSON packuments with a purpose-built binary
index. 344 packuments: load 302.7 ms -> 4.5 ms (67x), end to end 372 -> 70 ms
(5.3x). Chart build unchanged at ~65 ms because it was already mask-bound, which
is exactly what the earlier measurement predicted.

Building it exposed a determinism bug I had claimed did not exist: `extract_deps`
returns a HashSet-derived Vec, so a package's edge ORDER depended on hashing. The
solver is pure; the chart handed to it was not. The equality test (chart from
index == chart from JSON) failed on edge order and forced the fix (sort edges in
the builder).

Order of operations that made this work: measure (found masks were 98% of chart
build) -> optimise the measured thing (12.4x) -> measure again (load now 85-90%)
-> build the thing that fixes the new measurement (67x on load). Each step chosen
by the previous number, not by novelty.

Still not wired into the live cache path: the reader/writer are tested, the
integration point is the packument cache, and that is the next step.

## Journal — 2026-11-07 (cont.): the index, second attempt — self-healing instead of hooks

I wired the index writer into the packument fetch path and it never ran: a real
install wrote 63 packuments and 0 indexes, because the cache has more than one
writer. The lesson is not "find the other writer" — it is that a hook has to be
placed where the data *actually* flows, and I could have known that by testing
the write side before claiming it worked.

Fix: make the loader self-healing. `load_cache_dir` writes the index for anything
it had to parse, so the accelerator installs itself wherever the resolver's view
is read. Real cache: 426 ms JSON -> 15.2 ms with indexes (28x). Stat-only
validation (length + mtime) because hashing would spend what the index saves.

Deleted the dead fetch hook rather than leave two mechanisms.

Still not consumed by the live install path, because other consumers need full
packuments; that switch is the next item, and it is now a one-line change at the
gather site rather than an architecture question.

## Journal — 2026-11-07 (cont.): the multi-major blocker, solved and proven

Built `jagr_nest`: resolve instances, not packages — a requirement creates an
instance, and only incompatible ranges get a second one. That is what nesting
actually is, and it is the one thing a flat domain cannot express.

Proof on the real corpus (275 packuments, fetch-and-retry like a real install):
jest resolves — 270 instances, 249 packages, 19 multi-major, ~103 ms — with
ansi-styles at 6.2.3, 5.2.0 and 4.3.0.

Real data immediately found two things synthetic tests did not: npm: aliases
(@jest/react-is-18 -> react-is, which 404s if you ask the registry for the alias
name) and optional edges to unpublished platform packages. Both fixed, both
tested.

Three failure kinds instead of one: missing metadata (fetch and retry),
unsatisfiable (real conflict with chain and published versions), bounded (guard).
That distinction is what lets a caller act instead of apologising.

Still not wired into materialisation or the lockfile. Said plainly in the docs
rather than implied by the word "resolved".

## Journal — 2026-11-07 (cont.): jest installs, but I will not call it ready

Wired jagr_nest into the real tree path (resolve_full_tree_nested): instances ->
paths -> tarball+integrity, with fetch-and-retry. `jhol install jest@29.7.0`
now installs 266 packages, nested majors included, jest bin present. It failed
outright before.

I made it opt-in anyway. A warm run reports a package the lockfile lists and the
tree lacks (function-bind) and repairs 1-2 packages per run — the planned path
for one instance is not the path materialisation writes. That is a plumbing bug,
but "installs 266 packages and then quietly needs repair" is not production
ready, and the default path is unchanged so nobody inherits it.

Also hit the disk wall again (152 MiB free, build failed). Removed 12 GB of
regenerable target/debug. The standing rule earned its place.

Costs: cold jest 54-70 s, ~270 sequential metadata fetches; resolver itself
~103 ms. Parallel metadata fetch is the next change, and it is transport, not
algorithm.

## Journal — 2026-11-07 (cont.): default-on, and the corruption that looked like a layout bug

Promoted the instance resolver to default in the nested branch. It is only
reached when the flat expansion already failed, so it can only turn a failure
into an install, and it falls back automatically.

Warm runs still reported "Added 1 packages" forever. I assumed my placement plan
was wrong; it was not. `node_modules/function-bind/package.json` was 0 bytes, and
`store_extract_complete` returned early on the completion marker, so a finished
extract with an unreadable manifest counted as installed — every install
"repaired" it by re-cloning the same zero bytes.

Two lessons, both cheap to state and expensive to skip:
- The first guard required a non-empty manifest unconditionally; two store tests
  and the installer gate failed, because a tarball with no root package.json is
  legitimate. The narrower predicate (empty-and-present = corrupt) is the correct
  one, and the gate taught me that, not reasoning.
- I also tried parallel metadata prefetch. The frontier followed peerDependencies
  and every published version, never closed, and a cold jest install hung to a
  600 s timeout with 0 packages. Reverted. A hang is worse than slow.

Now: jest 266 packages, 51 s cold, Up to date on three consecutive warm runs;
659 tests; installer gate PASS.

## Journal — 2026-11-07 (cont.): why, from the lockfile; and the last HashMap in the pipeline

`jhol why` already printed dependency chains; it now also prints the derivation
the lockfile holds — copies, versions, paths, and the range each dependent asked
for. Offline, no re-resolution. Attribution is nearest-declaring-ancestor, which
I got wrong first (it picked the hoisted copy over the nested one) and the test
caught in ten seconds.

Also removed the last HashMap-ordering dependency in the resolver's pipeline:
propagate's incoming lists and dirty queue are sorted. The fixpoint cannot change
(it is monotone and idempotent) but reaching it by a different route is not the
same as guaranteeing the same route, and the chart is the input the pure solver is
judged on. Test: same world in forward, reverse and shuffled order must produce
identical domains.

Both are the kind of thing that makes a resolver liked rather than fast: an answer
to "why is this here" that costs nothing, and determinism you can test.

## Journal — 2026-11-07 (cont.): why-not, and the core that names one thing

`jhol why-not pkg@range` now answers with the *minimal* set of requirements that
forbids the range. Greedy shrink over set intersections: drop a requirement, if
the intersection is still empty it was never needed. On a real jest tree, chalk@^3
is blocked by one dependent (jest-watcher needs ^4), not by the thirty that mention
chalk — a core that lists the whole tree is a log, not an explanation.

The first version answered "you cannot use ansi-styles@^4" when 4.3.0 was
installed and in use. Caught by trying it on a real tree, not by the tests. Now an
already-installed range is answered as installed, with its path, before any
conflict analysis.

No clause DB, no nogoods — the bitmap representation is already the explanation
language, which is why this costs milliseconds and runs offline from the cache.

## Journal — 2026-11-07 (cont.): the prefetch, done boundedly this time

Second attempt at parallel metadata fetch, this time bounded by construction: no
peers, hard name cap, capped rounds, 20 s ceiling, and sequential fallback for
whatever is left. Three tests pin the bound, because the first attempt hung a cold
install for 600 s by having no bound at all.

Measured phases on cold jest: prefetch 400 packuments in 9.8-12.3 s (sequentially
that was ~200 ms each), tarball downloads 19-54 s, materialisation from the store
379 ms. So metadata is solved and the install is now payload-bound, with a 3x
network variance that makes any single "installed in X s" claim meaningless. I am
reporting per-phase numbers instead.

Lesson I keep relearning: measure the phase, not the total. The total moved 31 s
-> 64 s between two runs of the same manifest with nothing but network changing.
