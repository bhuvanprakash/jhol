//! Bytecode format.
//!
//! Instructions are a fixed 16-byte `{ op, a, b, c }` record. A register file
//! per frame, `Jump` operands are absolute instruction indices, and every
//! instruction index doubles as an inline-cache slot (`proto.ics[ip]`), so no
//! operand space is spent on caching metadata.
//!
//! Register convention:
//! `0 .. nparams-1` parameters, then function-level `var`/`let`/`const` locals,
//! then evaluation temporaries. Locals that any nested function captures are
//! boxed into `JCell`s by `CreateCell` at entry and reached with `CellGet` /
//! `CellSet`.

use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU8};
use std::sync::{Arc, OnceLock};

use crate::value::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Op {
    /// `dst = src`
    Move,
    /// `dst = k`
    LoadK,
    LoadUndefined,
    LoadNull,
    LoadTrue,
    LoadFalse,
    LoadThis,
    /// `dst = new Array` with the `b` register values as elements
    NewArray,
    /// `dst = {}`
    NewObject,
    /// `dst = {…}` built in one shot. `b` = const index of the static key list
    /// (`ConstVal::Keys`), `c` = register base of the values, in the same order
    /// as the keys. The shape chain is computed once per call site and cached
    /// in that site's `Ic`, and the property bag is filled directly, so a
    /// literal no longer dispatches one `DefProp` (plus a `shape_for`
    /// transition) per key. Object literals were the hottest `own_define`
    /// caller in `gc_stress`.
    NewObjectLit,
    /// `dst = new Array` of length `b` (holes)
    NewArrayLen,

    /// `dst = locals[a]`
    GetLocal,
    /// `locals[a] = src(b)`
    SetLocal,
    /// `locals[a]` is a cell; `dst = *cell`
    CellGet,
    /// `locals[a]` is a cell; `*cell = src(b)`
    CellSet,
    /// box `locals[a]` into a fresh cell
    CreateCell,
    /// `let` loop head: box `locals[b]` into a *new* cell stored at `locals[a]`.
    /// The value lives in its own register so the per-iteration cell is never
    /// mutated by the loop's update expression.
    RebindCell,
    /// `dst = upvals[a][b]`
    GetUpval,
    /// `upvals[a][b] = src(c)`
    SetUpval,
    /// `dst = new Closure(proto = consts[a])` capturing `b` cells from this frame
    NewClosure,

    /// `dst = global[k]` (IC)
    GetGlobal,
    /// `global[k] = src`
    SetGlobal,
    /// `global[k] = src` creating the property; used by hoisted declarations
    DefGlobal,

    /// `dst = obj[key]` (IC)
    GetProp,
    /// `obj[key] = src`
    SetProp,
    /// `dst = obj[locals[idx]]`
    GetIndex,
    /// `obj[locals[idx]] = src`
    SetIndex,
    /// `obj[key] = src` as a define (`Object.defineProperty`-like, writable+configurable+enumerable)
    DefProp,
    /// `dst = delete obj[key]`
    DeleteProp,

    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    /// `dst = regs[b] <op> consts[c]`, operator in the payload:
    /// 0 `+`, 1 `-`, 2 `*`, 3 `/`, 4 `%`, 5 `<`, 6 `>`, 7 `<=`, 8 `>=`.
    ///
    /// `i + 1`, `n - 2` and `i < n` are among the most executed bytecodes in
    /// the suite, and each cost a `LoadK` plus a binop — two dispatch turns and
    /// two decodes for one arithmetic step. Keeping the operator in the opcode
    /// lets the constant be addressed straight out of the pool, and because a
    /// local's slot *is* a register index the left operand can be a plain local
    /// too, which folds the `GetLocal` in as well.
    BinOpK(u8),
    Neg,
    /// `dst = typeof src`
    TypeOf,
    /// `dst = a in b`
    In,
    /// `dst = a instanceof b`
    InstanceOf,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    BitNot,
    /// loose `==`
    Eq,
    StrictEq,
    Lt,
    Gt,
    Le,
    Ge,
    Not,

    /// `ip = c`
    Jump,
    /// `if !truthy(locals[a]) { ip = c }`
    JumpIfFalse,
    /// `if truthy(locals[a]) { ip = c }`
    JumpIfTrue,
    /// `if isNullOrUndefined(locals[a]) { ip = c }`
    JumpIfNilish,
    /// `if !isNullOrUndefined(locals[a]) { ip = c }` — the `??` shape: skip the
    /// right operand when the left one is present.
    JumpIfNotNilish,
    /// `if isNullOrUndefined(locals[a]) { ip = c }` — kept operand for debugging
    JumpIfNilishKeep,

    /// `dst = call(locals[a], locals[a+1..a+1+b])`
    Call,
    /// `dst = call(obj = locals[a], locals[a+1..a+1+b])` with method IC
    CallMethod,
    /// `dst = new(locals[a], locals[a+1..a+1+b])`
    Construct,
    /// `dst = new(locals[a], locals[a+1..a+1+b])` with the prototype taken from
    /// `new.target` — used by `super(...)` inside a derived constructor.
    ConstructSuper,
    /// Suspend the current generator, publishing `locals[a]`, and snapshot its
    /// registers so a later `next()` resumes mid-body.
    Yield,
    /// `locals[a].__proto__ = locals[b]`. The internal prototype slot is not a
    /// data property, so `class … extends` gets its own instruction rather than
    /// routing through a global helper (whose call-site register layout is easy
    /// to get wrong).
    SetProto,
    /// Append `locals[b]` to the string accumulator held for local slot `a`.
    /// The accumulator is a per-frame byte buffer, not a `Value`, so it cannot
    /// be aliased by `var t = s`. A read of the local flushes it first.
    AppendStr,
    /// Materialise the accumulator for local slot `a` back into `locals[a]`.
    FlushStr,
    /// `dst = f(locals[b], locals[b+1..a+b])` with `this = locals[c]`.
    /// Used for computed method calls (`obj[k](...)`) and super calls, where the
    /// receiver is neither the callee nor `undefined`.
    CallThis,
    /// Materialise the (already evaluated) `this` into `dst`.
    LoadNewTarget,
    /// `dst` = array of the current frame's arguments from index `b` on.
    BuildRest,

    /// `return locals[a]`
    /// `reg[a] = reg[a] + reg[b]` — the read-modify-write form of `x += y`.
    /// Four instructions (GetLocal, GetLocal, Add, SetLocal) collapse to one
    /// without duplicating coercion: the handler calls the same `arith` the
    /// `Add` opcode does.
    LocalAdd,
    /// `if !(reg[a] CMP K) goto c` — a local compared against a constant,
    /// branched on in one instruction. `b` packs the comparison in its top
    /// byte (0=`<`, 1=`>`, 2=`<=`, 3=`>=`, the codes `compare()` takes) with
    /// the constant-pool index below it. Loop tests were four instructions —
    /// load local, load constant, compare, branch — three of which only ever
    /// fed the branch.
    JumpLocalCmp,
    /// `reg[a] = reg[a] + K` — `x += 1` without materialising the constant
    /// into a register first. Same `arith` path as `LocalAdd`.
    LocalAddK,
    /// `reg[a] += K[b]; goto c` — the increment and the loop's back-jump in
    /// one instruction. A counted loop ended `AddK, Jump`; both exist only to
    /// get back to the top, and the jump is never observable on its own.
    LocalAddKJ,
    Return,
    /// `throw locals[a]`
    Throw,
    /// push an exception handler: `c` = catch target ip, `b` = slot for the value
    PushTry,
    /// pop innermost handler
    PopTry,
    /// close (nil out) cells for locals `>= a`
    CloseUpvalues,

    /// `dst = a == null ? b : c` fused ternary for the common shape
    Select,

    /// Placeholder for superinstructions emitted by the finaliser.
    Nop,
    /// `dst = delete obj[regs[c]]` — computed-key delete. The key comes from a
    /// register (like GetIndex/SetIndex), unlike `DeleteProp` (dot syntax),
    /// whose key is baked into the inline cache at compile time.
    DeleteIndex,
    /// `dst = super(...locals[b])` — `ConstructSuper` for a call with spread
    /// arguments: `locals[b]` is the array the compiler built from them. Appended
    /// at the end of the enum so existing opcode numbers do not move.
    ConstructSuperSpread,
}

#[derive(Clone, Debug)]
pub enum ConstVal {
    Undefined,
    /// An array-literal elision slot: materialises `Value::EMPTY`, which
    /// reads back as `undefined` but keeps the slot a *hole* for `in` and
    /// `for-in`. JIT const loading rejects this and deopts to the
    /// interpreter, which is the safe default.
    Empty,
    Null,
    True,
    False,
    Num(f64),
    /// Materialised from the module's string pool; owned by the arena so it is
    /// stable for the function's whole lifetime.
    Str(Value),
    BigInt(i64),
    /// Index into `Module::protos`.
    Fn(u32),
    /// The static key list of an object literal, in insertion order. Referenced
    /// by `Op::NewObjectLit`'s `c` operand. Stored in the const pool so the
    /// literal's keys are reachable from the instruction stream without an
    /// extra operand or a register window.
    Keys(Box<[crate::objects::PropKey]>),
}

#[derive(Clone, Copy, Debug)]
pub struct Instr {
    pub op: Op,
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl Instr {
    #[inline]
    pub const fn new(op: Op, a: u32, b: u32, c: u32) -> Self {
        Instr { op, a, b, c }
    }
    #[inline]
    pub fn target(&self) -> usize {
        self.c as usize
    }
}

/// Per-call-site inline-cache state. Stored parallel to the code stream so it
/// costs nothing to reach (`proto.ics[ip]`).
#[derive(Clone, Copy, Debug)]
pub struct Ic {
    /// Expected object shape, `0` = uninitialised.
    pub shape: u32,
    /// Cached key (validated before the slot is trusted).
    pub key: crate::objects::PropKey,
    /// Cached slot inside the object.
    pub slot: u32,
    /// Hits at the current shape.
    pub hits: u32,
    /// Number of distinct miss shapes seen.
    pub misses: u16,
    /// 0 = cold, 1 = one shape, 2 = two shapes, 3 = three shapes, 4 = megamorphic
    pub state: u8,
    /// Second shape for the polymorphic case.
    pub shape2: u32,
    pub slot2: u32,
    /// Third shape. Shape-polymorphic code cycles through three layouts, and
    /// stopping at two left every third read walking the prototype chain from
    /// scratch — the single hottest leaf in that benchmark.
    pub shape3: u32,
    pub slot3: u32,
    /// `Op::NewObjectLit`: the shape the whole literal settles on, computed once
    /// by walking the transition chain for its static key list. `0` = not yet
    /// computed (a non-empty literal's final shape is always `>= 1`).
    pub obj_shape: u32,
    /// String-primitive receivers have no object to validate against, so the
    /// cache records where String.prototype holds the key plus the prototype's
    /// shape. Mutating String.prototype changes its shape and invalidates it,
    /// exactly like the object cache.
    pub str_shape: u32,
    pub str_slot: u32,
    pub has_str: bool,
    /// Same idea for arrays: an array has no property bag of its own, so
    /// `arr.push` resolves on `Array.prototype`. Record where that prototype
    /// keeps the key and validate it against the prototype's shape, so the
    /// per-call `#get:` accessor probe and the prototype walk disappear.
    pub arr_shape: u32,
    pub arr_slot: u32,
    pub has_arr: bool,
    /// Primitive receivers (numbers and booleans) share `Number.prototype` and
    /// own no property bag. Mirrors the string/array entries: record where the
    /// prototype keeps the key and validate against its shape, so
    /// `(i % 10).toString()` does not walk `Number.prototype` on every call.
    pub num_shape: u32,
    pub num_slot: u32,
    pub has_num: bool,
    /// Store site: the receiver's prototype identity and the property epoch
    /// for which this site last saw "no setter can intercept". Re-checking
    /// `setter_can_intercept` rebuilt a tuple and hashed it on every write; the
    /// per-site entry compares two words instead. Any accessor installed
    /// anywhere bumps the epoch, and changing the prototype changes the bits,
    /// so both invalidations are caught.
    pub store_proto: u64,
    pub store_epoch: u64,
    pub store_seen: bool,
    /// Compiled counted-loop code for this site (JIT). Null when absent.
    pub jit_code: *const u8,
    /// Instruction the VM continues at when the compiled loop completes.
    pub jit_exit: u32,
    /// 0 = counting executions, 2 = compiled, 3 = ineligible or deoptimized.
    pub jit_state: u8,
}

impl Ic {
    /// Slot recorded for `shape`, if this site has seen it. Checked most-likely
    /// first so the monomorphic case stays a single compare; `None` means the
    /// caller must fall back to the full walk.
    #[inline]
    pub fn slot_for(&self, shape: u32) -> Option<u32> {
        if self.state >= 1 && self.shape == shape {
            return Some(self.slot);
        }
        if self.state >= 2 && self.shape2 == shape {
            return Some(self.slot2);
        }
        if self.state >= 3 && self.shape3 == shape {
            return Some(self.slot3);
        }
        None
    }
}

impl Default for Ic {
    fn default() -> Self {
        Ic {
            shape: 0,
            key: crate::objects::PropKey::Sym(0),
            slot: 0,
            hits: 0,
            misses: 0,
            state: 0,
            shape2: 0,
            slot2: 0,
            shape3: 0,
            slot3: 0,
            obj_shape: 0,
            str_shape: 0,
            str_slot: 0,
            has_str: false,
            arr_shape: 0,
            arr_slot: 0,
            has_arr: false,
            num_shape: 0,
            num_slot: 0,
            has_num: false,
            store_proto: 0,
            store_epoch: 0,
            store_seen: false,
            jit_code: std::ptr::null(),
            jit_exit: 0,
            jit_state: 0,
        }
    }
}

/// One compiled function.
#[derive(Debug)]
pub struct FnProto {
    pub name: String,
    pub code: Vec<Instr>,
    pub consts: Vec<ConstVal>,
    pub ics: Vec<Ic>,
    /// Total register slots the frame needs.
    pub nslots: usize,
    pub nparams: usize,
    pub nupvals: usize,
    /// Lazily classified simple callback shape for the native fast path.
    /// 0 = not yet classified; otherwise `(kind << 1) | 1`, where a set low bit
    /// means "classified" and the rest names the operation. Cached on the proto
    /// so the body is inspected once, not once per call.
    pub simple_cb: std::sync::atomic::AtomicU8,
    /// Proto indices of functions nested inside this one.
    pub nested: Vec<u32>,
    pub is_arrow: bool,
    pub is_strict: bool,
    pub is_generator: bool,
    /// Number of upvalue slots whose cells come from locals of this frame.
    pub local_cells: Vec<u32>,
    /// `(from_local, index)` per upvalue: `from_local` means "cell #index of the
    /// *enclosing* frame's locals", otherwise "upvalue #index of the enclosing
    /// frame". This is what `NewClosure` needs to wire a closure's cells without
    /// a runtime name lookup.
    pub upval_desc: Vec<(bool, u32)>,
    /// `var` names, in hoist order, so the frame can pre-create the slots.
    pub hoisted_vars: usize,
    /// Source line per instruction (0 = unknown).
    pub lines: Vec<u32>,
    pub entry_file: Option<String>,
    /// Native entry point of the whole-function JIT (null until compiled).
    pub jit_entry: AtomicPtr<u8>,
    /// Own executable storage until the last closure/frame releases this proto.
    pub(crate) jit_mapping: OnceLock<(usize, usize)>,
    /// Interpreted calls seen so far — the compile trigger.
    pub jit_calls: AtomicU32,
    /// Deopts observed since compilation; native entry is disabled past the limit.
    pub jit_deopts: AtomicU32,
    /// 1 when the compiled function writes no upvalue, so a native caller may
    /// invoke it without risking a repeated side effect on deopt.
    pub jit_pure: AtomicU8,
    /// Leaf proof: every guard precedes writes, and success returns an Smi.
    /// Regions resume at individual instructions, so committed cell writes
    /// are safe there; whole-function callers still require `jit_pure`.
    pub(crate) jit_region_safe: AtomicU8,
    /// 1 = `return p0 + p1`, 2 = `return p0 * p1`. 0 = not that shape.
    pub(crate) jit_fold: AtomicU8,
}

impl FnProto {
    pub fn new(name: &str) -> Self {
        FnProto {
            name: name.to_string(),
            code: Vec::new(),
            consts: Vec::new(),
            ics: Vec::new(),
            nslots: 0,
            nparams: 0,
            nupvals: 0,
            simple_cb: AtomicU8::new(0),
            nested: Vec::new(),
            is_arrow: false,
            is_strict: false,
            is_generator: false,
            local_cells: Vec::new(),
            upval_desc: Vec::new(),
            hoisted_vars: 0,
            lines: Vec::new(),
            entry_file: None,
            jit_entry: AtomicPtr::new(std::ptr::null_mut()),
            jit_mapping: OnceLock::new(),
            jit_calls: AtomicU32::new(0),
            jit_deopts: AtomicU32::new(0),
            jit_pure: AtomicU8::new(0),
            jit_region_safe: AtomicU8::new(0),
            jit_fold: AtomicU8::new(0),
        }
    }

    #[inline]
    pub fn ic(&self, ip: usize) -> Ic {
        self.ics[ip]
    }

    #[inline]
    pub fn set_ic(&mut self, ip: usize, ic: Ic) {
        self.ics[ip] = ic;
    }

    /// Number of instructions.
    #[inline]
    pub fn len(&self) -> usize {
        self.code.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }
}

impl Drop for FnProto {
    fn drop(&mut self) {
        if let Some(&(address, len)) = self.jit_mapping.get() {
            unsafe { crate::jit::release_exec(address as *mut u8, len) };
        }
    }
}

#[derive(Debug)]
pub struct Module {
    pub protos: Vec<Arc<FnProto>>,
    pub entry: u32,
    pub name: String,
}

impl Module {
    pub fn entry(&self) -> &Arc<FnProto> {
        &self.protos[self.entry as usize]
    }
}
