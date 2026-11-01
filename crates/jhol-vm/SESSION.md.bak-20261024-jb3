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
