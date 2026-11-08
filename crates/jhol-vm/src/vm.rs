//! The JholVM interpreter.
//!
//! One flat register-file loop drives every function: `Call` pushes a frame and
//! the same loop keeps going, so a JavaScript call costs a frame push and *no*
//! Rust stack frame. Exceptions unwind through a handler table rather than
//! through `?`, which is what lets `try/catch` work across nested calls.
//!
//! # Inline caches
//!
//! Every `GetProp`/`SetProp`/`GetGlobal` site owns an [`Ic`] slot indexed by its
//! instruction index. The first execution runs the slow path and records the
//! object's shape plus the slot the key was found in; from then on the site
//! validates with one shape compare and reads the slot directly. The slot is
//! re-validated against the key on every hit, so a stale index can never escape
//! even if two shapes collide.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::arena::{self, Arena};
use crate::bytecode::*;
use crate::errors::{throw_string, type_error, VmError, VmResult};
use crate::objects::*;
use crate::value::{f64_to_js_string, ArithOp, Value};

const MAX_FRAMES: usize = 900;
pub const ITER_SYM: &str = "@@iterator";
#[allow(dead_code)]
const ENUM_KEYS: &str = "__jhol_enum_keys";
/// Register slots the VM reserves in every non-arrow frame, mirroring the
/// compiler's layout: 0 = `this`, 1 = `new.target`.
const THIS_SLOT: u32 = 0;
const NEWTARGET_SLOT: u32 = 1;

#[allow(dead_code)]
type PropStore = HashMap<u32, Vec<u8>>;

pub(crate) struct Frame {
    func: *mut JFunction,
    /// Raw pointer into the closure's `Arc<FnProto>` (the `JFunction` this frame
    /// was entered for keeps that `Arc` alive for the frame's whole lifetime).
    proto: *const FnProto,
    reg_base: usize,
    arg_base: usize,
    /// Absolute stack index where this frame's register window starts; the stack
    /// is truncated back here on return. This is *not* `arg_base`, which points
    /// into the caller's register array.
    stack_base: usize,
    /// Drop any arguments appended by bound-function dispatch on return.
    return_stack_base: usize,
    argc: u32,
    ip: usize,
    this_val: Value,
    new_target: Value,
    #[allow(dead_code)]
    super_ctor: Value,
    /// Where the return value should be written, in absolute stack terms.
    ret: Option<(usize, u32)>,
    /// Some(index) when this frame is a suspended generator body.
    gen: Option<usize>,
    /// Per-slot string accumulation buffers. A slot that is the target of
    /// `s += x` keeps its bytes here instead of allocating a new string per
    /// append; `FlushStr` writes them back to the register. Because this is
    /// never a `Value`, no other binding can observe or alias it.
    builders: Vec<Option<Vec<u8>>>,
    /// Temporary registers spilled during a call (receiver/callee pairs).
    _scratch: u32,
}

impl Frame {
    /// Borrow this frame's function prototype. Safe because the `JFunction`
    /// this frame runs keeps the `Arc<FnProto>` alive until the frame is popped.
    #[inline]
    fn proto_ref(&self) -> &FnProto {
        unsafe { &*self.proto }
    }
}

pub(crate) struct Handler {
    /// Frame index this handler belongs to.
    frame_idx: usize,
    catch_ip: usize,
    /// Absolute index of the register that receives the thrown value.
    slot: usize,
}

/// Dispatch cache for the top frame.
///
/// Every instruction used to re-derive the frame's raw pointers from
/// `frames[fi].proto`, which is three separate dependent loads through an
/// `Arc` (`code`, `consts`, `ics`) plus two Vec bounds checks — all of it
/// identical from one instruction to the next. The cache holds them instead
/// and is rebuilt only when the frame stack actually changes depth, which is
/// exactly when the top frame can change.
#[derive(Clone, Copy)]
struct FrameCache {
    /// `frames.len()` the cache was built for; `usize::MAX` forces a build.
    len: usize,
    fi: usize,
    reg_base: usize,
    code: *const Instr,
    consts: *const ConstVal,
    ics: *const Ic,
}

pub struct Vm {
    pub stack: Vec<Value>,
    pub(crate) frames: Vec<Frame>,
    fc: FrameCache,
    /// Bumped every time an accessor is installed anywhere. Together with a
    /// prototype's identity it answers "can any setter intercept here?" without
    /// re-walking the chain on every store.
    /// The installed arena, captured once. `Vm` cannot outlive it: `Engine`
    /// installs the arena before building the VM and uninstalls on drop, and no
    /// sub-engine is ever created while one is live. Kept as a null-checked
    /// pointer so a VM built without an arena still falls back to TLS.
    /// Compiled counted loops, keyed by (proto address, loop-head ip). The
    /// per-site `Ic` holds a raw code pointer for the hot check; this map owns
    /// the executable memory.
    jit: std::collections::HashMap<(usize, usize), crate::jit::JitLoop>,
    #[cfg(test)]
    completed_native_loops: usize,
    arena_ptr: *mut Arena,
    /// Memo for that question: `(proto identity, epoch, has_setter)`.
    setter_scan: Option<(u64, u64, bool)>,
    /// Scratch: whether the accessor scan in `set_prop`'s slow path
    /// intercepted. `Op::SetProp` reads it to decide if the site's store cache
    /// may be populated.
    set_scan_clear: bool,
    /// Per-key memo for `setter_can_intercept`: an exact
    /// `(own shape, proto identity, epoch, key)` whose prototype chain was
    /// already scanned for this key's `#set:` slot and came back empty, so the
    /// store is a plain own definition.
    ///
    /// The coarse chain test is true for almost every object — `Object.prototype`
    /// owns an accessor — so without this every ordinary property write walked
    /// its prototype chain. Exact match only: a wrong entry would silently skip
    /// a setter.
    set_clear: [(u64, u64, u64, u64); 8],
    set_clear_used: u32,
    pub(crate) handlers: Vec<Handler>,
    pub globals: *mut JObject,
    /// Directories of the modules currently executing (entry script at the
    /// bottom, nested requires on top). `require` resolves relative specs
    /// against the top entry — the same rule node uses.
    pub module_dirs: Vec<String>,
    pub(crate) protos: Vec<Arc<FnProto>>,
    /// Roots that the GC must see but that live outside the register file.
    roots: Vec<Value>,
    last_result: Value,
    pending: Option<VmError>,
    /// Return value of the most recently invoked *native* function. A native
    /// call leaves no frame behind, so `run_until` cannot observe its value;
    /// this bridges the gap for builtin-to-JS call sites.
    pub native_result: Value,
    /// Suspended generator bodies, indexed by their object's `#gen` slot.
    pub gens: Vec<Option<GenState>>,
    gen_owners: HashMap<u64, usize>,
    free_gens: Vec<usize>,
    pub exit_code: i32,
    pub timers: Vec<Timer>,
    pub timer_seq: u64,
    pub proto_strings: HashMap<PropKey, Vec<u8>>,
    proto_string: *mut JObject,
    proto_object: *mut JObject,
    pub(crate) proto_function: *mut JFunction,
    proto_array: *mut JObject,
    /// `Number.prototype` (and `Boolean.prototype`): primitives have no heap
    /// object to hang a prototype on, so they look it up here.
    proto_number: *mut JObject,
    /// Cached prototype `Value`s: rebuilding these from the raw pointer called
    /// `arena_base()` (a thread-local lookup) on every primitive property read.
    /// Set during `builtins::install`, before any lookup can observe them.
    proto_number_val: Value,
    /// `0`.toString() .. `9`.toString(), interned once. `(i % 10).toString()`
    /// in a build loop asks for exactly ten strings millions of times; the
    /// single-digit path otherwise pays a SipHash + map probe per call.
    pub(crate) digit_atoms: [Value; 10],
    proto_string_val: Value,
    proto_array_val: Value,
    proto_bool: *mut JObject,
    empty_array_shape: u32,
    empty_obj_shape: u32,
    #[allow(dead_code)]
    obj_named_fn_fn: Option<NativeFn>,
    #[allow(dead_code)]
    global_ic: HashMap<u32, Value>,
    pub depth_guard: usize,
}

#[derive(Clone)]
pub struct Timer {
    pub id: u64,
    pub kind: TimerKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    Timeout,
    Interval,
}

pub struct Engine {
    pub vm: Vm,
    started: bool,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Release the arena this engine installed, so a process (or a test
        // suite) that builds several engines does not leak one reservation each.
        clear_timers();
        crate::arena::Arena::uninstall();
    }
}

#[derive(Clone)]
pub struct TimerEntry {
    pub id: u64,
    pub kind: TimerKind,
    pub fire_at: std::time::Instant,
    pub interval: u64,
    pub cb: Value,
    pub args: Vec<Value>,
}

impl Vm {
    /// The arena, reached through the pointer captured at VM construction
    /// rather than an 8ns thread-local lookup. Mirrors what `arena::active()`
    /// returned — it hands out the same `&'static mut` today.
    /// `Number.prototype`'s `Value`, cached to avoid `Value::from_ptr`, which
    /// performs a thread-local lookup through `arena_base()`. The cache is set
    /// during `builtins::install`; before that this rebuilds from the pointer,
    /// so nothing can ever observe an unset slot.
    #[inline]
    pub(crate) fn number_proto(&self) -> Value {
        let v = self.proto_number_val;
        if v.is_undefined() {
            // Third of the recursive-fallback traps (see array_proto/
            // string_proto): rebuild from the raw pointer.
            Value::from_ptr(self.proto_number)
        } else {
            v
        }
    }

    #[inline]
    #[inline]
    pub(crate) fn string_proto(&self) -> Value {
        let v = self.proto_string_val;
        if v.is_undefined() {
            // Same recursive-fallback trap array_proto had; rebuild from the
            // raw pointer instead.
            Value::from_ptr(self.proto_string)
        } else {
            v
        }
    }

    #[inline]
    pub(crate) fn array_proto(&self) -> Value {
        let v = self.proto_array_val;
        if v.is_undefined() {
            // The fallback rebuilds from the raw pointer. The previous text
            // called `self.array_proto()` here — infinite recursion, one
            // unset cache away from a stack overflow.
            Value::from_ptr(self.proto_array)
        } else {
            v
        }
    }

    #[inline]
    pub fn arena_ref(&self) -> &'static mut Arena {
        let p = self.arena_ptr;
        if !p.is_null() {
            unsafe { &mut *p }
        } else {
            arena::active()
        }
    }

    pub fn arena(&mut self) -> &'static mut Arena {
        self.arena_ref()
    }

    /// Register read. Bounds are a compile-time invariant: every frame resizes
    /// the stack to its `nslots` on entry, so the index is always in range.
    #[inline]
    fn ld(&self, base: usize, r: u32) -> Value {
        unsafe { *self.stack.get_unchecked(base + r as usize) }
    }

    #[inline]
    fn st(&mut self, base: usize, r: u32, v: Value) {
        unsafe {
            *self.stack.get_unchecked_mut(base + r as usize) = v;
        }
    }

    // ---------------- allocation helpers ----------------

    #[cfg(test)]
    pub(crate) fn compiled_loop_count(&self) -> usize {
        self.jit.len()
    }

    #[cfg(test)]
    pub(crate) fn completed_loop_count(&self) -> usize {
        self.completed_native_loops
    }

    pub fn new_string(&mut self, s: &str) -> Value {
        Value::from_ptr(self.arena().alloc_string(s))
    }

    pub fn new_string_utf16(&mut self, units: &[u16]) -> Value {
        Value::from_ptr(self.arena().alloc_string_utf16(units))
    }

    /// ASCII `s += rhs` into the frame builder. Returns 1 when the JIT must
    /// leave the instruction to the interpreter (nothing was written).
    /// `Op::NewArray` for JIT'd regions: elements live at `dest+1 .. dest+n`
    /// in the frame (the compiler's window convention, same as the
    /// interpreter handler), the array goes to `dest`. Allocation cannot
    /// collect — only interpreter checkpoints do — so reading the elements
    /// after `new_array` is safe.
    pub(crate) fn jit_new_array(&mut self, regs: *mut Value, dest: u32, n: u32) -> u32 {
        let v = self.new_array();
        unsafe {
            let arr = self.as_array_mut(v);
            let base = dest as usize + 1;
            for i in 0..n as usize {
                arr.push(*regs.add(base + i));
            }
            *regs.add(dest as usize) = v;
        }
        0
    }

    /// Materialise the string accumulator in `slot` into a real string Value,
    /// exactly as `Op::FlushStr` does in the interpreter. Called from JIT'd
    /// loop regions that *read* a string being built (`s.length`,
    /// `s.charCodeAt`) — without this the whole region was rejected and the
    /// loop silently ran interpreted forever.
    pub(crate) fn jit_flush_str(&mut self, slot: u32) -> u32 {
        let Some(fi) = self.frames.len().checked_sub(1) else {
            return 1;
        };
        let buf = self
            .frames
            .get(fi)
            .and_then(|f| f.builders.get(slot as usize).and_then(|b| b.clone()));
        if let Some(buf) = buf {
            let sv = String::from_utf8(buf).unwrap_or_default();
            let v = self.new_string(&sv);
            self.frames[fi].builders[slot as usize] = None;
            let reg_base = self.frames[fi].reg_base;
            self.st(reg_base, slot, v);
        }
        0
    }

    pub(crate) fn jit_append_str(&mut self, slot: u32, rhs: Value) -> u32 {
        let Some(fi) = self.frames.len().checked_sub(1) else {
            return 1;
        };
        let reg_base = self.frames[fi].reg_base;
        let current = self.stack[reg_base + slot as usize];
        if (rhs.is_string() && unsafe { rhs.as_string() }.ascii == 0)
            || (current.is_string() && unsafe { current.as_string() }.ascii == 0)
        {
            return 1;
        }
        let needs_seed = {
            let frame = &self.frames[fi];
            frame
                .builders
                .get(slot as usize)
                .and_then(|b| b.as_ref())
                .is_none()
        };
        let seed = if needs_seed {
            current
                .as_str()
                .map(|s| s.as_bytes().to_vec())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let owned;
        let bytes: &[u8] = if rhs.is_string() {
            unsafe { rhs.as_string().as_str() }.as_bytes()
        } else {
            owned = match self.to_string(rhs) {
                Ok(s) => s,
                Err(_) => return 1,
            };
            owned.as_bytes()
        };
        let frame = &mut self.frames[fi];
        if slot as usize >= frame.builders.len() {
            frame.builders.resize(slot as usize + 1, None);
        }
        let buf = frame.builders[slot as usize].get_or_insert(seed);
        buf.extend_from_slice(bytes);
        0
    }

    pub(crate) fn jit_new_object_lit(
        &mut self,
        regs: *mut Value,
        dest: u32,
        val_base: u32,
        ic: *mut crate::bytecode::Ic,
        consts: *const ConstVal,
        key_idx: u32,
    ) -> u32 {
        if consts.is_null() || regs.is_null() || ic.is_null() {
            return 1;
        }
        let keys: &[PropKey] = match unsafe { &*consts.add(key_idx as usize) } {
            ConstVal::Keys(k) => k,
            _ => return 1,
        };
        let n = keys.len();
        let cached = unsafe { (*ic).obj_shape };
        let shape = if cached != 0 {
            cached
        } else {
            let mut s = self.empty_obj_shape;
            for k in keys {
                s = self.arena().shape_for(s, *k);
            }
            unsafe { (*ic).obj_shape = s };
            s
        };
        let base = self.arena().base();
        let proto = Value::from_ptr_in(self.proto_object, base);
        // Four inline slots are the object. Fill them in the value that gets
        // written once, instead of zeroing them and then pushing over the zeros.
        if n <= INLINE_PROPS {
            let mut small = [(PropKey::Sym(0), Value::UNDEFINED); INLINE_PROPS];
            for (i, k) in keys.iter().enumerate() {
                small[i] = (*k, unsafe { *regs.add(val_base as usize + i) });
            }
            let o = self.arena().alloc_obj(|| JObject {
                hdr: ObjHeader::new(TAG_OBJECT, 0, 0),
                shape,
                proto,
                nprops: n as u32,
                extensible: true,
                spill: None,
                small,
                index: None,
            });
            unsafe { *regs.add(dest as usize) = Value::from_ptr_in(o, base) };
            return 0;
        }
        let o = self.arena().alloc_object(shape, proto);
        let v = Value::from_ptr_in(o, base);
        unsafe {
            let ob = &mut *o;
            ob.reserve(n);
            for (i, k) in keys.iter().enumerate() {
                ob.push_prop(*k, *regs.add(val_base as usize + i));
            }
            if n > crate::objects::DENSE_MAX {
                ob.rebuild_index();
            }
            *regs.add(dest as usize) = v;
        }
        0
    }

    pub(crate) fn jit_set_prop(
        &mut self,
        obj: Value,
        val: Value,
        ic: *mut crate::bytecode::Ic,
    ) -> u32 {
        if ic.is_null() {
            return 1;
        }
        let key = unsafe { (*ic).key };
        match self.set_prop(obj, key, val) {
            Ok(()) => 0,
            Err(_) => 1,
        }
    }

    pub fn new_object(&mut self) -> Value {
        let shape = self.empty_obj_shape;
        let proto = Value::from_ptr(self.proto_object as *mut JObject);
        Value::from_ptr(self.arena().alloc_object(shape, proto))
    }

    pub fn new_array(&mut self) -> Value {
        let shape = self.empty_array_shape;
        let proto = self.array_proto();
        Value::from_ptr(self.arena().alloc_array(shape, proto))
    }

    pub fn new_array_with(&mut self, items: Vec<Value>) -> Value {
        let v = self.new_array();
        for it in items {
            self.as_array_mut(v).push(it);
        }
        v
    }

    pub fn new_function_native(&mut self, name: &str, f: NativeFn, len: u32) -> Value {
        let proto_obj = Value::UNDEFINED;
        let fnv = self
            .arena()
            .alloc_function(JFunction::native(Value::UNDEFINED, f, proto_obj));
        let v = Value::from_ptr(fnv);
        let nm = self.new_string(name);
        unsafe {
            (*fnv).name = nm;
            (*fnv).proto = Value::from_ptr(self.proto_function as *mut JFunction);
            let _ = len;
        }
        v
    }

    /// True when `v` might have an accessor in its prototype chain. Objects
    /// never get this bit unless an accessor is actually installed on them.
    pub fn has_accessors(&mut self, v: Value) -> bool {
        if !v.is_ptr() {
            return false;
        }
        unsafe {
            let tag = v.obj_tag();
            match tag {
                Some(TAG_OBJECT) => {
                    v.as_object().hdr.flags & FLAG_ACCESSOR != 0
                        || v.as_object().proto.is_ptr() && self.has_accessors(v.as_object().proto)
                }
                // `proto` sits at a different offset in JArray than in JObject,
                // so the two layouts must not be read through the same cast.
                Some(TAG_ARRAY) => {
                    v.as_array().hdr.flags & FLAG_ACCESSOR != 0
                        || v.as_array().proto.is_ptr() && self.has_accessors(v.as_array().proto)
                }
                Some(TAG_STRING) => false,
                Some(TAG_FUNCTION) => {
                    v.as_function().hdr.flags & FLAG_ACCESSOR != 0
                        || v.as_function().proto.is_ptr()
                            && self.has_accessors(v.as_function().proto)
                }
                _ => false,
            }
        }
    }

    /// Memoized dispatch, tagged by pointer so a `Number`/`Boolean` primitive
    /// does not run the tag checks twice.
    #[inline]
    pub fn proto_of(&mut self, v: Value) -> Value {
        match unsafe { v.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { v.as_object().proto },
            Some(TAG_ARRAY) => unsafe { v.as_array().proto },
            Some(TAG_FUNCTION) => unsafe { v.as_function().proto },
            Some(TAG_STRING) => self.string_proto(),
            _ => {
                if v.is_bool() {
                    Value::from_ptr(self.proto_bool)
                } else if v.is_number() {
                    self.number_proto()
                } else {
                    Value::UNDEFINED
                }
            }
        }
    }

    /// Shared [[SetPrototypeOf]] validation and cache invalidation. Prototype
    /// changes anywhere in an ancestor chain invalidate negative setter memos.
    pub fn set_prototype(&mut self, object: Value, prototype: Value) -> VmResult<()> {
        let object_like = |v: Value| v.is_object() || v.is_array() || v.is_function();
        if object.is_null() || object.is_undefined() {
            return Err(type_error("cannot set prototype of null or undefined"));
        }
        if !object_like(prototype) && !prototype.is_null() {
            return Err(type_error("prototype must be an object or null"));
        }
        if !object_like(object) {
            return Ok(());
        }
        let old = self.proto_of(object);
        if old.bits() == prototype.bits() || (!object_like(old) && prototype.is_null()) {
            return Ok(());
        }
        if object.is_object() && std::ptr::eq(unsafe { object.as_object() }, self.proto_object) {
            return Err(type_error("Object.prototype has an immutable prototype"));
        }
        let extensible = unsafe {
            match object.obj_tag() {
                Some(TAG_OBJECT) => object.as_object().extensible,
                Some(TAG_ARRAY) => object.as_array().extensible,
                Some(TAG_FUNCTION) => {
                    object.as_function().hdr.flags & (FLAG_FROZEN | FLAG_SEALED) == 0
                }
                _ => false,
            }
        };
        if !extensible {
            return Err(type_error("object is not extensible"));
        }
        // Floyd's walk also rejects pre-existing cycles without an allocation
        // or a depth limit that would reject a valid long prototype chain.
        let mut slow = prototype;
        let mut fast = prototype;
        while object_like(slow) {
            if slow.bits() == object.bits() {
                return Err(type_error("cyclic prototype value"));
            }
            slow = self.proto_of(slow);
            if object_like(fast) {
                fast = self.proto_of(fast);
            }
            if object_like(fast) {
                fast = self.proto_of(fast);
            }
            if object_like(slow) && slow.bits() == fast.bits() {
                return Err(type_error("cyclic prototype value"));
            }
        }
        unsafe {
            match object.obj_tag() {
                Some(TAG_OBJECT) => object.as_object_mut().proto = prototype,
                Some(TAG_ARRAY) => object.as_array_mut().proto = prototype,
                Some(TAG_FUNCTION) => object.as_function_mut().proto = prototype,
                _ => unreachable!(),
            }
        }
        self.arena().invalidate_property_caches();
        self.setter_scan = None;
        self.set_clear_used = 0;
        Ok(())
    }

    pub fn as_object_mut(&mut self, v: Value) -> &'static mut JObject {
        unsafe { v.as_object_mut() }
    }
    pub fn as_array_mut(&mut self, v: Value) -> &'static mut JArray {
        unsafe { v.as_array_mut() }
    }

    // ---------------- property access ----------------

    #[inline]
    pub fn get_prop(&mut self, obj: Value, key: PropKey) -> VmResult<Value> {
        let result = self.get_prop_ic(obj, key, None);
        self.retain_outcome(&result);
        result
    }

    #[inline]
    fn retain_outcome(&mut self, result: &VmResult<Value>) {
        match result {
            Ok(value) => self.arena().retain_value(*value),
            Err(VmError::Exception(error)) => self.arena().retain_value(error.value),
            Err(_) => {}
        }
    }

    pub(crate) fn pin_temporaries(&mut self, values: &[Value]) {
        self.arena().pin_temporaries(values);
    }

    /// Collect at a VM safe point, including roots held outside register slots.
    pub fn collect_garbage(&mut self) {
        let base = self.arena().base();
        let mut roots = self.roots.clone();
        roots.extend_from_slice(&self.stack);
        roots.extend([self.last_result, self.native_result]);
        for pointer in [
            self.globals,
            self.proto_object,
            self.proto_array,
            self.proto_string,
            self.proto_number,
            self.proto_bool,
        ] {
            if !pointer.is_null() {
                roots.push(Value::from_ptr_in(pointer, base));
            }
        }
        if !self.proto_function.is_null() {
            roots.push(Value::from_ptr_in(self.proto_function, base));
        }
        if let Some(VmError::Exception(error)) = &self.pending {
            roots.push(error.value);
        }
        roots.extend(self.global_ic.values().copied());
        for frame in &self.frames {
            roots.push(Value::from_ptr_in(frame.func, base));
            roots.extend([frame.this_val, frame.new_target, frame.super_ctor]);
        }
        for proto in &self.protos {
            for constant in &proto.consts {
                if let ConstVal::Str(value) = constant {
                    roots.push(*value);
                }
            }
        }
        TIMERS.with(|timers| {
            for timer in timers.borrow().iter() {
                roots.push(timer.cb);
                roots.extend_from_slice(&timer.args);
            }
        });
        let owners = &self.gen_owners;
        let states = &self.gens;
        unsafe { &mut *self.arena_ptr }.collect_with(&[&roots], |arena, offset, work| {
            let owner = Value::from_ptr_in(unsafe { base.add(offset) }, base);
            if let Some(state) = owners.get(&owner.bits()).and_then(|&i| states[i].as_ref()) {
                arena.mark_value(state.func, work);
                arena.mark_value(state.this_val, work);
                for &value in &state.regs {
                    arena.mark_value(value, work);
                }
                for constant in &state.proto.consts {
                    if let ConstVal::Str(value) = constant {
                        arena.mark_value(*value, work);
                    }
                }
                state.regs.capacity() * std::mem::size_of::<Value>()
            } else {
                0
            }
        });
        let dead: Vec<_> = self
            .gen_owners
            .iter()
            .filter_map(|(&owner, &index)| {
                let header = unsafe { Value(owner).ptr_in::<ObjHeader>(base) };
                (unsafe { (*header).tag } == 0).then_some((owner, index))
            })
            .collect();
        for (owner, index) in dead {
            self.gen_owners.remove(&owner);
            self.gens[index] = None;
            self.free_gens.push(index);
        }
    }

    #[inline]
    fn collect_if_due(&mut self) {
        if self.arena().collection_due() {
            self.collect_garbage();
        }
    }

    fn accessor_keys(&mut self, base: u32, prefix: &str) -> u32 {
        self.arena().accessor_sym(base, prefix)
    }

    /// Own-property read for objects and functions (they share the layout).
    #[inline]
    fn own_get(&mut self, v: Value, key: PropKey) -> Option<Value> {
        unsafe {
            match v.obj_tag() {
                Some(TAG_OBJECT) => v.as_object().get(key),
                Some(TAG_FUNCTION) => v.as_function().get(key),
                Some(TAG_ARRAY) => {
                    // Array elements are own properties: `2 in sparse` must
                    // consult the element slots (get_raw returns None for
                    // holes), and non-index keys live in the props bag.
                    let a = v.as_array();
                    match key {
                        PropKey::Idx(i) => a.get_raw(i),
                        _ => a.get_prop(key),
                    }
                }
                _ => None,
            }
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn own_set(&mut self, v: Value, key: PropKey, val: Value) {
        unsafe {
            match v.obj_tag() {
                Some(TAG_OBJECT) => {
                    v.as_object_mut().set(key, val);
                }
                Some(TAG_FUNCTION) => {
                    v.as_function_mut().set(key, val);
                }
                _ => {}
            }
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn own_delete(&mut self, v: Value, key: PropKey) -> bool {
        unsafe {
            match v.obj_tag() {
                Some(TAG_OBJECT) => v.as_object_mut().delete(key),
                Some(TAG_FUNCTION) => v.as_function_mut().delete(key),
                _ => false,
            }
        }
    }

    /// Create an own property, growing the object's shape through the arena's
    /// transition table so inline caches stay cheap.
    pub(crate) fn own_define(&mut self, v: Value, key: PropKey, val: Value) {
        unsafe {
            match v.obj_tag() {
                Some(TAG_OBJECT) => {
                    let o = v.as_object_mut();
                    // Grow the hidden class only when the key set really
                    // changes. Transitioning on an overwrite minted a fresh
                    // shape for every store to an existing property, and each
                    // shape copied its parent's key list — quadratic time and
                    // memory for something as ordinary as `o.x = v` in a loop.
                    // One probe feeds both decisions: `set()` used to repeat
                    // the same find_index on top of this check.
                    match o.find_index(key) {
                        None => {
                            o.shape = self.arena().shape_for(o.shape, key);
                            o.insert_new(key, val);
                        }
                        Some(i) => {
                            o.set_slot(i, val);
                        }
                    }
                }
                Some(TAG_FUNCTION) => {
                    let f = v.as_function_mut();
                    if f.find_index(key).is_none() {
                        f.shape = self.arena().shape_for(f.shape, key);
                    }
                    f.set(key, val);
                }
                Some(TAG_ARRAY) => {
                    // Only non-index keys reach `own_define` for arrays: the
                    // indexed ones were handled in `set_prop`.
                    v.as_array_mut().set_prop(key, val);
                }
                _ => {}
            }
        }
    }

    /// Read `obj`'s own property (or run its own getter). `receiver` is the
    /// object the chain started from: an accessor found on a prototype still
    /// runs with the *receiver* as `this`, not with the prototype.
    fn get_own_or_accessor(
        &mut self,
        obj: Value,
        key: PropKey,
        receiver: Value,
        want_getter: bool,
    ) -> VmResult<Option<Value>> {
        // An index key is a real property on an object too (`cache[n] = v`), so
        // it resolves here rather than being dropped outright.
        let sym = match key {
            PropKey::Sym(s) => s,
            PropKey::Idx(_) => {
                return Ok(unsafe {
                    match obj.obj_tag() {
                        Some(TAG_OBJECT) => obj.as_object().get(key),
                        Some(TAG_FUNCTION) => obj.as_function().get(key),
                        Some(TAG_ARRAY) => match key {
                            PropKey::Idx(i) => obj.as_array().get_raw(i),
                            PropKey::Sym(_) => None,
                        },
                        _ => None,
                    }
                });
            }
        };
        let direct = match unsafe { obj.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { obj.as_object() }.get(key),
            Some(TAG_FUNCTION) => unsafe { obj.as_function() }.get(key),
            Some(TAG_ARRAY) => match key {
                PropKey::Idx(i) => unsafe { obj.as_array() }.get_raw(i),
                PropKey::Sym(_) => unsafe { obj.as_array() }.get_prop(key),
            },
            _ => None,
        };
        if let Some(value) = direct {
            return Ok(Some(value));
        }
        // Only reached when the plain value is absent: building the hidden
        // `#get:` symbol interns a formatted name, which used to run on EVERY
        // property read (the own value was checked after it). Hoisting it here
        // is pure code motion — `accessor_keys` is idempotent.
        let _scope = self.arena().root_scope(&[obj, receiver]);
        let acc = self.accessor_keys(sym, if want_getter { "#get:" } else { "#set:" });
        if let Some(v) = self.own_get(obj, PropKey::Sym(acc)) {
            if v.is_function() && want_getter {
                return self.call0(v, receiver).map(Some);
            }
            if v.is_function() && !want_getter {
                return Ok(Some(v));
            }
            return Ok(Some(Value::UNDEFINED));
        }
        if want_getter {
            let setter = self.accessor_keys(sym, "#set:");
            if self.own_get(obj, PropKey::Sym(setter)).is_some() {
                return Ok(Some(Value::UNDEFINED));
            }
        }
        Ok(None)
    }

    /// Core read path. `ic` is the call site's cache slot, when there is one.
    pub(crate) fn get_prop_ic(
        &mut self,
        obj: Value,
        key: PropKey,
        mut ic: Option<&mut Ic>,
    ) -> VmResult<Value> {
        if obj.is_undefined() || obj.is_null() {
            return Err(type_error(&format!(
                "cannot read properties of {} (reading '{}')",
                if obj.is_null() { "null" } else { "undefined" },
                self.prop_name(key)
            )));
        }
        let cur_is_object = obj.is_object();
        if !cur_is_object {
            // Arrays and functions have their own fast paths.
            match unsafe { obj.obj_tag() } {
                Some(TAG_ARRAY) => {
                    let a = unsafe { obj.as_array() };
                    if let PropKey::Sym(_) = key {
                        if self.is_length_key(key) {
                            return Ok(Value::from_i32(a.len as i32));
                        }
                    }
                    if let PropKey::Idx(i) = key {
                        return Ok(a.get(i));
                    }
                }
                Some(TAG_FUNCTION) => {
                    if let PropKey::Sym(_) = key {
                        if self.is_length_key(key) {
                            let arity = unsafe { obj.as_function().arity() };
                            return Ok(Value::from_i32(arity as i32));
                        }
                        let kn = self.key_name(key);
                        if kn == "prototype" {
                            return Ok(unsafe { obj.as_function().proto_obj });
                        }
                        // `fn.__proto__`: a function's own [[Prototype]] (the parent
                        // class for `class B extends A`, which is how static
                        // `super.m()` and inherited statics resolve).
                        if kn == "__proto__" && unsafe { obj.as_function() }.get(key).is_none() {
                            let p = unsafe { obj.as_function() }.proto;
                            return Ok(if p.is_object() || p.is_function() || p.is_array() {
                                p
                            } else {
                                Value::NULL
                            });
                        }
                        // `fn.name`: synthesized lazily (an own property on every
                        // closure would cost an allocation per function). A name
                        // the program assigned wins; otherwise the compiler's
                        // inferred one (`const f = () => 1` -> "f"), else "".
                        if kn == "name" && unsafe { obj.as_function() }.get(key).is_none() {
                            let f = unsafe { obj.as_function() };
                            if f.name.is_string() {
                                return Ok(f.name);
                            }
                            let nm = match &f.kind {
                                FnKind::Bytecode(p) => p.name.clone(),
                                FnKind::Native(_) => String::new(),
                                #[allow(unreachable_patterns)]
                                _ => String::new(),
                            };
                            return Ok(self.new_string(&nm));
                        }
                    }
                }
                Some(TAG_STRING) => {
                    // `as_str()` is a zero-copy view into the arena. Copying the
                    // string here used to run on EVERY property read of a
                    // primitive — `s.charCodeAt(i)` in a loop therefore copied
                    // the whole (growing) string twice per call, O(n²).
                    if let PropKey::Idx(i) = key {
                        let i = i as usize;
                        if let Some(unit) = unsafe { obj.as_string() }.code_unit(i) {
                            return Ok(self.new_string_utf16(&[unit]));
                        }
                        return Ok(Value::UNDEFINED);
                    }
                    if self.is_length_key(key) {
                        let len = unsafe { obj.as_string().utf16_len as usize };
                        return Ok(Value::from_i32(len as i32));
                    }
                    // Method names (`s.charCodeAt`) need no content at all —
                    // fall through to the prototype chain below.
                }
                _ => {}
            }
        }

        // Monomorphic inline-cache fast path — with cached function pointer.
        if let Some(ref mut ic) = ic {
            if ic.state >= 1 && obj.is_object() {
                let o = unsafe { obj.as_object() };
                if ic.key == key {
                    if let Some(slot) = ic.slot_for(o.shape) {
                        if let Some((k, v)) = o.get_at(slot as usize) {
                            if k == key {
                                ic.hits += 1;
                                return Ok(v);
                            }
                        }
                    }
                }
            }
        }

        // Array-method prototype cache — the same trick as the string one
        // below, and the reason `arr.push(x)` in a loop no longer walks to
        // Array.prototype (and computes a `#get:` symbol) on every iteration.
        if obj.is_array() {
            if let Some(ref mut ic) = ic {
                if ic.has_arr && ic.key == key {
                    let p = self.array_proto();
                    if p.is_object() {
                        let o = unsafe { p.as_object() };
                        if o.shape == ic.arr_shape {
                            if let Some((k, v)) = o.get_at(ic.arr_slot as usize) {
                                if k == key {
                                    ic.hits += 1;
                                    return Ok(v);
                                }
                            }
                        }
                    }
                }
            }
        }

        // String-primitive prototype cache. A primitive has no shape of its
        // own, so validate against String.prototype's shape instead; the slot
        // is then read straight out of the prototype.
        if obj.is_string() {
            if let Some(ref mut ic) = ic {
                if ic.has_str && ic.key == key {
                    let p = self.string_proto();
                    if p.is_object() {
                        let o = unsafe { p.as_object() };
                        if o.shape == ic.str_shape {
                            if let Some((k, v)) = o.get_at(ic.str_slot as usize) {
                                if k == key {
                                    ic.hits += 1;
                                    return Ok(v);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Numbers and booleans share `Number.prototype` and own nothing, so
        // validate against that prototype's shape instead of walking its key
        // list on every call — `(i % 10).toString()` in a build loop asks this
        // question once per iteration.
        if obj.is_number() || obj.is_bool() {
            if let Some(ref mut ic) = ic {
                if ic.has_num && ic.key == key {
                    let p = self.number_proto();
                    if p.is_object() {
                        let o = unsafe { p.as_object() };
                        if o.shape == ic.num_shape {
                            if let Some((k, v)) = o.get_at(ic.num_slot as usize) {
                                if k == key {
                                    ic.hits += 1;
                                    return Ok(v);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Slow path: own properties, then the prototype chain. Objects, arrays
        // and functions all carry a property bag, so all three must be walked.
        let mut cur = obj;
        let mut guard = 0usize;
        loop {
            if cur.is_object() || cur.is_array() || cur.is_function() {
                if let Some(found) = self.get_own_or_accessor(cur, key, obj, true)? {
                    // Only record an IC for the object itself: a prototype hit
                    // is not a shape the receiver can validate against.
                    if let Some(ref mut ic) = ic {
                        if cur.bits() == obj.bits() {
                            self.record_ic(ic, cur, key);
                        }
                        if obj.is_string() {
                            self.record_str_ic(ic, obj, key);
                        }
                        if obj.is_array() {
                            self.record_arr_ic(ic, key);
                        }
                        if obj.is_number() || obj.is_bool() {
                            self.record_num_ic(ic, key);
                        }
                    }
                    return Ok(found);
                }
            } else if cur.is_string() {
                if self.is_length_key(key) {
                    let len = unsafe { cur.as_string().utf16_len as usize };
                    return Ok(Value::from_i32(len as i32));
                }
            }
            cur = self.proto_of(cur);
            if !(cur.is_object() || cur.is_array() || cur.is_function() || cur.is_string()) {
                break;
            }
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        Ok(Value::UNDEFINED)
    }

    /// Flag `obj` as carrying an accessor, so `set_prop` knows to scan.
    pub fn mark_accessor(&mut self, obj: Value) {
        // Any cached "this chain has no setter" answer is now suspect.
        self.arena().invalidate_property_caches();
        if obj.is_object() || obj.is_function() {
            unsafe {
                let h = match obj.obj_tag() {
                    Some(TAG_OBJECT) => &mut obj.as_object_mut().hdr,
                    Some(TAG_FUNCTION) => &mut obj.as_function_mut().hdr,
                    _ => return,
                };
                h.flags |= FLAG_ACCESSOR;
            }
        }
    }

    /// Does `key` name one of the hidden `#get:`/`#set:` accessor slots?
    #[allow(dead_code)]
    fn is_accessor_key(&mut self, key: PropKey) -> bool {
        match key {
            PropKey::Sym(s) => {
                let name = self.arena().sym_name(s);
                name.starts_with("#get:") || name.starts_with("#set:")
            }
            PropKey::Idx(_) => false,
        }
    }

    /// Cache where String.prototype keeps `key`, validated later against the
    /// prototype's shape.
    fn record_str_ic(&mut self, ic: &mut Ic, obj: Value, key: PropKey) {
        if !obj.is_string() {
            return;
        }
        let p = self.string_proto();
        if !p.is_object() {
            return;
        }
        let o = unsafe { p.as_object() };
        if let Some(slot) = o.find_index(key) {
            ic.str_shape = o.shape;
            ic.str_slot = slot as u32;
            ic.has_str = true;
        }
    }

    /// Cache where `Number.prototype` keeps `key`, validated later against its
    /// shape. Numbers and booleans both resolve through it and own no bag.
    fn record_num_ic(&mut self, ic: &mut Ic, key: PropKey) {
        let p = self.number_proto();
        if !p.is_object() {
            return;
        }
        let o = unsafe { p.as_object() };
        if let Some(slot) = o.find_index(key) {
            ic.num_shape = o.shape;
            ic.num_slot = slot as u32;
            ic.has_num = true;
        }
    }

    /// Cache where `Array.prototype` keeps `key`, validated later against the
    /// prototype's shape. Mirrors `record_str_ic`; arrays own no property bag.
    fn record_arr_ic(&mut self, ic: &mut Ic, key: PropKey) {
        let p = self.array_proto();
        if !p.is_object() {
            return;
        }
        let o = unsafe { p.as_object() };
        if let Some(slot) = o.find_index(key) {
            ic.arr_shape = o.shape;
            ic.arr_slot = slot as u32;
            ic.has_arr = true;
        }
    }

    /// Can anything intercept a store of `key` to `obj` — an own accessor or
    /// one up its prototype chain?
    ///
    /// The answer is recomputed only when the receiver's prototype changes or
    /// an accessor is installed somewhere (which bumps `the arena property epoch`). The
    /// flag invariant makes it sound: every path that installs a getter or
    /// setter marks the object that owns it, and `has_accessors` reads each
    /// tag's own `proto` offset rather than one layout.
    ///
    /// The chain test alone answers too coarsely to be useful: `Object.prototype`
    /// owns an accessor, so "some accessor exists on this chain" is true for
    /// nearly every object, and an ordinary `o.x = v` would then scan the chain
    /// for `x`'s `#set:` slot on every store. The real answer is stable while
    /// the receiver's own shape, its prototype and the epoch are, so it is
    /// memoized per key.
    fn setter_can_intercept(&mut self, obj: Value, key: PropKey) -> bool {
        // Own accessors are a per-object flag, not a chain property. The
        // receiver's own shape is only known for plain objects, and only those
        // take part in the memo.
        let mut shape = 0u64;
        let mut memoizable = false;
        unsafe {
            match obj.obj_tag() {
                Some(TAG_OBJECT) => {
                    let o = obj.as_object();
                    if o.hdr.flags & FLAG_ACCESSOR != 0 {
                        return true;
                    }
                    shape = o.shape as u64;
                    memoizable = true;
                }
                Some(TAG_ARRAY) => {
                    if obj.as_array().hdr.flags & FLAG_ACCESSOR != 0 {
                        return true;
                    }
                }
                Some(TAG_FUNCTION) => {
                    if obj.as_function().hdr.flags & FLAG_ACCESSOR != 0 {
                        return true;
                    }
                }
                _ => return true,
            }
        }
        let proto = self.proto_of(obj);
        if !proto.is_ptr() {
            return false;
        }
        let pb = proto.bits();
        let epoch = self.arena().property_epoch();
        let memo = (shape, pb, epoch, key.discriminant());
        let slot = (memo.3 as usize ^ (shape as usize).wrapping_mul(31)) & 7;
        if memoizable && self.set_clear_used & (1 << slot) != 0 && self.set_clear[slot] == memo {
            return false;
        }
        let has = match self.setter_scan {
            Some((b, e, has)) if b == pb && e == epoch => has,
            _ => {
                let has = self.has_accessors(proto);
                self.setter_scan = Some((pb, epoch, has));
                has
            }
        };
        if !has {
            return false;
        }
        // A setter for a symbol key lives at its `#set:` symbol; an indexed
        // write cannot name one.
        let acc = match key {
            PropKey::Sym(sym) => self.arena().accessor_sym(sym, "#set:"),
            PropKey::Idx(_) => return false,
        };
        if self.chain_setter(obj, acc) {
            return true;
        }
        if memoizable {
            self.set_clear[slot] = memo;
            self.set_clear_used |= 1 << slot;
        }
        false
    }

    /// Does anything on the chain from `cur` upwards hold a function at
    /// accessor symbol `acc`? Mirrors the store slow path's scan.
    fn chain_setter(&mut self, mut cur: Value, acc: u32) -> bool {
        let mut guard = 0;
        while guard < 64 {
            if cur.is_object() || cur.is_array() || cur.is_function() {
                if let Some(s) = self.own_get(cur, PropKey::Sym(acc)) {
                    if s.is_function() {
                        return true;
                    }
                }
            }
            if !cur.is_ptr() {
                break;
            }
            cur = self.proto_of(cur);
            guard += 1;
        }
        false
    }

    fn record_ic(&mut self, ic: &mut Ic, obj: Value, key: PropKey) {
        if !obj.is_object() {
            return;
        }
        let o = unsafe { obj.as_object() };
        if let Some(slot) = o.find_index(key) {
            let sh = o.shape;
            if ic.state == 0 {
                ic.shape = sh;
                ic.slot = slot as u32;
                ic.key = key;
                ic.state = 1;
            } else if ic.shape != sh && ic.shape2 != sh && ic.shape3 != sh {
                // Three layouts are recorded, then the site is treated as
                // megamorphic: past that point another compare is worse than
                // just walking, which is what the old two-shape cap did from
                // the third shape onwards.
                match ic.state {
                    1 => {
                        ic.shape2 = sh;
                        ic.slot2 = slot as u32;
                        ic.state = 2;
                    }
                    2 => {
                        ic.shape3 = sh;
                        ic.slot3 = slot as u32;
                        ic.state = 3;
                    }
                    _ => ic.state = 4,
                }
            }
        }
    }

    fn is_length_key(&mut self, key: PropKey) -> bool {
        match key {
            PropKey::Sym(s) => self.arena().sym_name(s) == "length",
            PropKey::Idx(_) => false,
        }
    }

    fn key_name(&mut self, key: PropKey) -> String {
        match key {
            PropKey::Sym(s) => self.arena_ref().sym_name(s).to_string(),
            PropKey::Idx(_) => String::new(),
        }
    }

    pub fn prop_name(&self, key: PropKey) -> String {
        match key {
            PropKey::Sym(s) => self.arena_ref().sym_name(s).to_string(),
            PropKey::Idx(i) => i.to_string(),
        }
    }

    /// `key_from_value` maps every numeric key to `PropKey::Idx`, which is
    /// right for arrays and wrong for objects: an object's own property keys
    /// are *strings*, and the fused object-literal path stores `{5000: v}`
    /// under `PropKey::Sym("5000")`. The receiver decides — on a non-array the
    /// numeric form is rewritten to its string spelling, so literal and
    /// computed access agree.
    fn key_for_receiver(&mut self, obj: Value, key: Value) -> PropKey {
        let k = self.key_from_value(key);
        // Only plain objects need the rewrite. Arrays index with Idx, and
        // string/number receivers already resolve Idx natively — rewriting for
        // those broke `s.charCodeAt`-style reads off indexed string access.
        if matches!(unsafe { obj.obj_tag() }, Some(TAG_OBJECT)) {
            if let PropKey::Idx(i) = k {
                let s = i.to_string();
                let sym = self.arena().intern_sym(&s);
                return PropKey::Sym(sym);
            }
        }
        k
    }

    pub fn key_from_value(&mut self, v: Value) -> PropKey {
        if v.is_smi() || v.is_double() {
            if v.is_smi() {
                let i = v.as_i64();
                if i >= 0 && i <= u32::MAX as i64 {
                    return PropKey::Idx(i as u32);
                }
            }
            let f = v.as_f64();
            if f.fract() == 0.0 && f >= 0.0 && f <= u32::MAX as f64 {
                return PropKey::Idx(f as u32);
            }
            let s = f64_to_js_string(f);
            let sym = self.arena().intern_sym(&s);
            return PropKey::Sym(sym);
        }
        if v.is_string() {
            let s = unsafe { v.as_string().as_str() };
            // A numerically-shaped string still indexes arrays.
            if let Ok(n) = s.parse::<u32>() {
                return PropKey::Idx(n);
            }
            let sym = self.arena().intern_sym(s);
            return PropKey::Sym(sym);
        }
        PropKey::Sym(0)
    }

    /// Assign a property: write through a prototype accessor when one exists,
    /// otherwise create/overwrite an own property.
    pub fn set_prop(&mut self, obj: Value, key: PropKey, val: Value) -> VmResult<()> {
        self.set_prop_impl(obj, key, val, false)
    }

    /// `skip_scan` is set by `Op::SetProp` when this site's inline cache has
    /// already established that no accessor on the receiver or its prototype
    /// chain can intercept. It does not change what gets written — only how
    /// much checking happens first.
    #[inline]
    fn set_prop_impl(
        &mut self,
        obj: Value,
        key: PropKey,
        val: Value,
        skip_scan: bool,
    ) -> VmResult<()> {
        if skip_scan {
            self.own_define(obj, key, val);
            if obj.is_function() && self.key_name(key) == "prototype" {
                unsafe { obj.as_function_mut() }.proto_obj = val;
            }
            return Ok(());
        }
        if obj.is_undefined() || obj.is_null() {
            return Err(type_error("cannot set properties of null or undefined"));
        }
        if let Some(TAG_ARRAY) = unsafe { obj.obj_tag() } {
            if let PropKey::Idx(i) = key {
                self.as_array_mut(obj).set(i, val);
                return Ok(());
            }
            // `arr.length = n` resizes rather than adding a property.
            if self.is_length_key(key) {
                let n = self.to_uint32(val).unwrap_or(0);
                self.as_array_mut(obj).set_len(n);
                return Ok(());
            }
        }
        // Only a *setter* on the prototype intercepts an assignment. A writable
        // data property on the prototype does NOT: `obj.x = v` creates an own
        // property. Writing data through instead made `P_C.constructor = C`
        // overwrite `Shape.prototype.constructor`, which turned every later
        // `super()` into a recursive call back into the subclass.
        //
        // Only symbol keys can name an accessor, and the accessor symbol depends
        // only on the key — so compute it once instead of once per prototype
        // level. A store to an indexed array slot skips the scan entirely.
        // Fast path: nothing on the object or up its chain can intercept, so
        // skip the scan and the `#set:` symbol computation entirely. This is the
        // common case — plain property writes on ordinary objects.
        //
        // Only objects, arrays and functions take it; primitives keep the old
        // path so string/number prototype behavior is untouched.
        if (obj.is_object() || obj.is_array() || obj.is_function())
            && !self.setter_can_intercept(obj, key)
        {
            self.own_define(obj, key, val);
            if obj.is_function() && self.key_name(key) == "prototype" {
                unsafe { obj.as_function_mut() }.proto_obj = val;
            }
            return Ok(());
        }

        self.set_scan_clear = true;
        let acc = match key {
            PropKey::Sym(sym) => Some((sym, self.arena().accessor_sym(sym, "#set:"))),
            PropKey::Idx(_) => None,
        };
        let mut cur = obj;
        let mut guard = 0;
        let mut on_receiver = true;
        loop {
            if let Some((sym, set_sym)) = acc {
                if cur.is_object() || cur.is_array() || cur.is_function() {
                    if let Some(setter) = self.own_get(cur, PropKey::Sym(set_sym)) {
                        if setter.is_function() {
                            self.call_direct_args(setter, obj, &[val])?;
                            self.set_scan_clear = false;
                            return Ok(());
                        }
                    }
                    if on_receiver {
                        // The receiver's OWN accessor has no setter: sloppy-mode
                        // assignment is a silent no-op. Falling through to
                        // own_define below created a data property that shadowed
                        // the getter (spec: [[Set]] returns before
                        // CreateDataProperty when the own descriptor is an
                        // accessor without [[Set]]).
                        let get_sym = self.arena().accessor_sym(sym, "#get:");
                        if self.own_get(cur, PropKey::Sym(get_sym)).is_some() {
                            return Ok(());
                        }
                    }
                }
            }
            on_receiver = false;
            cur = self.proto_of(cur);
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        // Assigning `.prototype` on a function retargets `new`, since
        // instances take their prototype from that property.
        self.own_define(obj, key, val);
        if obj.is_function() && self.key_name(key) == "prototype" {
            unsafe { obj.as_function_mut() }.proto_obj = val;
        }
        Ok(())
    }

    /// `Object.defineProperty`-style: always creates the property.
    pub fn define_prop(&mut self, obj: Value, key: PropKey, val: Value) {
        if let Some(TAG_ARRAY) = unsafe { obj.obj_tag() } {
            if let PropKey::Idx(i) = key {
                self.as_array_mut(obj).set(i, val);
                return;
            }
        }
        if !(obj.is_object() || obj.is_function()) {
            return;
        }
        self.own_define(obj, key, val);
    }

    pub fn delete_prop(&mut self, obj: Value, key: PropKey) -> Value {
        if obj.is_array() {
            if let PropKey::Idx(i) = key {
                // JS `delete arr[i]` leaves a hole and keeps `length`, so
                // `join` renders it empty and `Object.keys` omits the index.
                let a = unsafe { obj.as_array_mut() };
                if (i as usize) < a.data.len() {
                    a.data[i as usize] = Value::EMPTY;
                }
                return Value::TRUE;
            }
        }
        if !obj.is_object() {
            return Value::TRUE;
        }
        let o = self.as_object_mut(obj);
        Value::from_bool(o.delete(key))
    }

    pub fn has_prop(&mut self, obj: Value, key: PropKey) -> bool {
        let mut cur = obj;
        let mut guard = 0;
        while (cur.is_object() || cur.is_array() || cur.is_function() || cur.is_string())
            && guard < 64
        {
            if self.own_get(cur, key).is_some() {
                return true;
            }
            if cur.is_string() && self.is_length_key(key) {
                return true;
            }
            cur = self.proto_of(cur);
            guard += 1;
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeError;

/// Converts any value to a JS string, allocating lazily.
// ---------------------------------------------------------------------------
// conversions
// ---------------------------------------------------------------------------

impl Vm {
    pub fn to_string(&mut self, v: Value) -> VmResult<String> {
        let prim = self.to_primitive(v, false)?;
        // `to_primitive` hands back the same value for anything it cannot
        // unwrap; only objects mean "give up" here, because a primitive is
        // already its own string.
        if prim.bits() == v.bits() && v.is_ptr() && !v.is_string() {
            return Ok("[object Object]".into());
        }
        if prim.is_string() {
            return Ok(unsafe { prim.as_string().as_str().to_string() });
        }
        if prim.is_smi() || prim.is_double() {
            return Ok(f64_to_js_string(prim.as_f64()));
        }
        if prim.is_bool() {
            return Ok(if prim.as_bool() {
                "true".into()
            } else {
                "false".into()
            });
        }
        if prim.is_null() {
            return Ok("null".into());
        }
        if prim.is_undefined() {
            return Ok("undefined".into());
        }
        if prim.is_array() {
            let a = unsafe { prim.as_array() };
            let mut parts: Vec<String> = Vec::with_capacity(a.data.len());
            for x in a.data.iter() {
                if x.is_empty() {
                    parts.push(String::new());
                } else {
                    parts.push(self.to_string(*x)?);
                }
            }
            return Ok(parts.join(","));
        }
        if prim.is_function() {
            let n = unsafe { prim.as_function().name };
            let name = if n.is_string() {
                unsafe { n.as_string().as_str().to_string() }
            } else {
                String::new()
            };
            return Ok(format!("function {}() {{ [native code] }}", name));
        }
        Ok("[object Object]".into())
    }

    pub fn to_number(&mut self, v: Value) -> VmResult<f64> {
        match () {
            _ if v.is_smi() => Ok(v.as_i64() as f64),
            _ if v.is_double() => Ok(f64::from_bits(v.bits())),
            _ if v.is_bool() => Ok(if v.as_bool() { 1.0 } else { 0.0 }),
            _ if v.is_null() => Ok(0.0),
            _ if v.is_undefined() => Ok(f64::NAN),
            // A hole in an array is `Value::EMPTY`. It is pointer-shaped but is
            // not an object, so it used to fall into the object branch below,
            // where `obj_tag()` returned garbage and `to_primitive` recursed
            // until the stack overflowed (crash on `[1,,3].map(x => x*2)`).
            _ if v.is_empty() => Ok(f64::NAN),
            _ if v.is_string() => {
                let s = unsafe { v.as_string().as_str().to_string() };
                let t = s.trim();
                if t.is_empty() {
                    return Ok(0.0);
                }
                if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    if let Ok(n) = i64::from_str_radix(hex, 16) {
                        return Ok(n as f64);
                    }
                    return Ok(f64::NAN);
                }
                Ok(t.parse::<f64>().unwrap_or(f64::NAN))
            }
            _ => {
                let p = self.to_primitive(v, true)?;
                // `to_primitive` only unwraps objects; if it hands back the same
                // value we would recurse forever.
                if p.bits() == v.bits() && v.is_ptr() {
                    return Ok(f64::NAN);
                }
                self.to_number(p)
            }
        }
    }

    /// Ordinary primitive conversion: numeric hints prefer `valueOf`.
    pub fn to_primitive(&mut self, v: Value, number_hint: bool) -> VmResult<Value> {
        if !v.is_ptr() || v.is_string() {
            return Ok(v);
        }
        // `EMPTY` is pointer-tagged but is not an object; unwrapping it as one
        // reads a shape that does not exist. It means "no value" here.
        if v.is_empty() {
            return Ok(Value::UNDEFINED);
        }
        let scope = self.arena().root_scope(&[v]);
        let result = self.to_primitive_inner(v, number_hint);
        drop(scope);
        self.retain_outcome(&result);
        result
    }

    fn to_primitive_inner(&mut self, v: Value, number_hint: bool) -> VmResult<Value> {
        if !v.is_ptr() {
            return Ok(v);
        }
        let has_valueof = match unsafe { v.obj_tag() } {
            Some(TAG_OBJECT) | Some(TAG_ARRAY) | Some(TAG_FUNCTION) => true,
            _ => false,
        };
        if !has_valueof {
            return Ok(v);
        }
        let order: [&str; 2] = if number_hint {
            ["valueOf", "toString"]
        } else {
            ["toString", "valueOf"]
        };
        for name in order {
            let sym = self.arena().intern_sym(name);
            let f = self.get_prop(v, PropKey::Sym(sym))?;
            if f.is_function() {
                let base = self.stack.len();
                self.stack.push(f);
                self.stack.push(v);
                let r = self.call_direct(f, v, 0)?;
                self.stack.truncate(base);
                if !r.is_ptr() || r.is_string() {
                    return Ok(r);
                }
            } else {
                let _ = f;
            }
        }
        Err(type_error("cannot convert object to primitive value"))
    }

    pub fn to_int32(&mut self, v: Value) -> VmResult<i32> {
        let n = self.to_number(v)?;
        Ok(js_to_int32(n))
    }

    pub fn to_uint32(&mut self, v: Value) -> VmResult<u32> {
        let n = self.to_number(v)?;
        Ok(js_to_uint32(n))
    }

    // ---------------- calling ----------------

    /// Call with arguments already laid out at `stack[arg_base .. arg_base+argc]`.
    /// `ret` says where the result goes when this is a bytecode call.
    pub fn call_value(
        &mut self,
        callee: Value,
        this: Value,
        arg_base: usize,
        argc: u32,
        ret: Option<(usize, u32)>,
    ) -> VmResult<()> {
        if !callee.is_function() {
            // node splits these: an undeclared identifier call is a
            // ReferenceError, a missing method is a TypeError. The VM has only
            // the value here, and `undefined` is far more often a missing
            // property (`o.m()`) than an undeclared global, so the TypeError
            // branch is the one that does not mislead. undeclared-global calls
            // are therefore reported as TypeError — see the VM gaps table.
            let n = self.to_string(callee).unwrap_or_default();
            return Err(type_error(&format!("{} is not a function", n)));
        }
        let f = unsafe { callee.as_function() };
        let mut this = match &f.kind {
            FnKind::Bytecode(proto) if proto.is_arrow => f.this_val,
            _ => this,
        };
        // Sloppy-mode calls coerce a missing receiver to the global object;
        // strict functions keep `undefined`. Without this, `f()` inside a method
        // threw "cannot read properties of undefined" where node sees
        // `globalThis`, and `o.m.call(null)` threw instead of reading globals.
        if let FnKind::Bytecode(proto) = &f.kind {
            if !proto.is_strict && (this.is_undefined() || this.is_null()) {
                this = Value::from_ptr(self.globals);
            }
        }
        if f.hdr.has(FLAG_BOUND) {
            let target = f.this_val;
            let bound_this = f.bound_this;
            let n = f.bound.len();
            let mut argv: Vec<Value> = Vec::with_capacity(n + argc as usize);
            for b in f.bound.iter() {
                argv.push(*b);
            }
            for i in 0..argc {
                argv.push(self.stack[arg_base + i as usize]);
            }
            return self.call_bound(target, bound_this, &argv, ret);
        }
        let leaf_flags = f.hdr.flags;
        match f.kind {
            FnKind::Native(f) => {
                // Inline the argument buffer for small calls: the vast
                // majority of native calls (arr.push, arr.map, console.log)
                // pass a handful of arguments, and a heap Vec per call was
                // pure allocation traffic. Stack-allocated is also
                // reentrancy-safe — a native that calls back into JS cannot
                // clobber it the way a shared scratch buffer would.
                const INLINE: usize = 8;
                let n = argc as usize;
                if n <= INLINE {
                    let mut inline = [Value::UNDEFINED; INLINE];
                    inline[..n].copy_from_slice(&self.stack[arg_base..arg_base + n]);
                    // Allocation alone cannot collect; only JS re-entry can.
                    // Opaque natives retain scoped roots. Audited leaf bodies
                    // avoid root-vector writes on every push/character load.
                    let primitive = |v: Value| !v.is_ptr() || v.is_string();
                    let leaf = leaf_flags & FLAG_NATIVE_LEAF != 0
                        || (leaf_flags & FLAG_NATIVE_PRIMITIVE_LEAF != 0
                            && primitive(this)
                            && inline[..n].iter().copied().all(primitive));
                    let scope = if leaf {
                        None
                    } else {
                        let scope = self.arena().root_scope(&[callee, this]);
                        self.pin_temporaries(&inline[..n]);
                        Some(scope)
                    };
                    let r = f(self, this, &inline[..n])?;
                    self.native_result = r;
                    if let Some((cb, d)) = ret {
                        self.st(cb, d, r);
                    }
                    drop(scope);
                    self.arena().retain_value(r);
                    return Ok(());
                }
                let mut args: Vec<Value> = Vec::with_capacity(n);
                for i in 0..n {
                    args.push(self.stack[arg_base + i]);
                }
                let scope = self.arena().root_scope(&[callee, this]);
                self.pin_temporaries(&args);
                let r = f(self, this, &args)?;
                self.native_result = r;
                if let Some((cb, d)) = ret {
                    self.st(cb, d, r);
                }
                drop(scope);
                self.arena().retain_value(r);
                Ok(())
            }
            FnKind::Bytecode(ref proto) => {
                // No `Arc::clone` here: this is the hot call path (millions of
                // calls in recursive/lambda-heavy code) and two atomic RMWs per
                // call were pure overhead. The `JFunction` in the arena holds
                // the only `Arc` that matters, and `callee` keeps it rooted on
                // the operand stack for the whole call.
                let proto: *const FnProto = &**proto;
                let proto = unsafe { &*proto };
                if proto.is_generator {
                    return self.make_generator(callee, proto, this, arg_base, argc, ret);
                }
                // Whole-function native code, when it exists. Everything is a
                // raw pointer into the operand stack; nothing in the native
                // call tree allocates or grows the stack, so the pointer stays
                // valid for the call and no collection can run while it is
                // live. A non-zero status means "guard failed": fall through
                // and let the interpreter evaluate the call properly.
                let entry = proto.jit_entry.load(std::sync::atomic::Ordering::Relaxed);
                if !entry.is_null() {
                    let sbase = self.stack.as_mut_ptr();
                    // SAFETY: `ret` names a slot of the caller's register
                    // window, which is inside `stack`.
                    let out: *mut Value = match ret {
                        Some((cb, d)) => unsafe { sbase.add(cb + d as usize) },
                        None => &mut self.native_result as *mut Value,
                    };
                    let args: *const Value = unsafe { sbase.add(arg_base) };
                    let fun: extern "C" fn(
                        *const Value,
                        u32,
                        u32,
                        *mut Value,
                        *const *mut crate::objects::JCell,
                        u64,
                        u64,
                    ) -> u32 = unsafe { std::mem::transmute(entry) };
                    // x5 carries `this`. x6 carries the callee object so a
                    // self-recursive leaf can recognise its own closure.
                    let st = fun(
                        args,
                        argc,
                        self.frames.len() as u32,
                        out,
                        f.upvalues.as_ptr(),
                        this.0,
                        callee.0,
                    );
                    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                        static ONCE: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(false);
                        if !ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            eprintln!("native st={st}");
                        }
                    }
                    if st == 0 {
                        return Ok(());
                    }
                    let d = proto
                        .jit_deopts
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if d + 1 >= crate::jit::NATIVE_DEOPT_LIMIT {
                        proto
                            .jit_entry
                            .store(std::ptr::null_mut(), std::sync::atomic::Ordering::Relaxed);
                    }
                } else {
                    let n = proto.jit_calls.load(std::sync::atomic::Ordering::Relaxed);
                    if n < crate::jit::NATIVE_CALL_AFTER {
                        proto
                            .jit_calls
                            .store(n + 1, std::sync::atomic::Ordering::Relaxed);
                    } else if n == crate::jit::NATIVE_CALL_AFTER {
                        proto
                            .jit_calls
                            .store(n + 1, std::sync::atomic::Ordering::Relaxed);
                        if let Some(e) = crate::jit::try_compile_func(proto) {
                            proto
                                .jit_entry
                                .store(e as *mut u8, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                self.enter_frame(callee, proto, this, arg_base, argc, Value::UNDEFINED, ret)
            }
        }
    }

    fn call_bound(
        &mut self,
        target: Value,
        this: Value,
        argv: &[Value],
        ret: Option<(usize, u32)>,
    ) -> VmResult<()> {
        if !target.is_function() {
            return Err(type_error("bound target is not a function"));
        }
        let base = self.stack.len();
        self.stack.extend_from_slice(argv);
        let depth = self.frames.len();
        let r = self.call_value(target, this, base, argv.len() as u32, ret);
        if self.frames.len() > depth {
            let frame = self.frames.last_mut().unwrap();
            frame.return_stack_base = frame.return_stack_base.min(base);
        } else {
            self.stack.truncate(base);
        }
        r
    }

    /// Direct call used by internals (`call`/`apply`/array methods) where the
    /// arguments come from a slice rather than the register file.
    pub fn call_direct(&mut self, callee: Value, this: Value, argc: u32) -> VmResult<Value> {
        self.call_direct_args(callee, this, &vec![Value::UNDEFINED; argc as usize])
    }

    pub fn call_direct_args(
        &mut self,
        callee: Value,
        this: Value,
        args: &[Value],
    ) -> VmResult<Value> {
        self.call_direct_args_inner(callee, this, args, true)
    }

    /// Builtin callback results are consumed before the next callback. Avoid
    /// retaining every intermediate reduce/comparator result until native exit.
    pub(crate) fn call_callback(
        &mut self,
        callee: Value,
        this: Value,
        args: &[Value],
    ) -> VmResult<Value> {
        self.call_direct_args_inner(callee, this, args, false)
    }

    /// Classify a callback body as two parameters feeding one numeric or
    /// relational op, then a return. Compressors and filters are almost always
    /// exactly this — `(a, b) => a - b`, `x => x * 2` — and setting one up
    /// costs a frame push, a stack resize and a param copy per call.
    fn classify_simple_cb(proto: &crate::bytecode::FnProto) -> u8 {
        let mut code: &[Instr] = &proto.code;
        // `compile_fn` always emits a dead `Return` after an explicit one.
        if code.len() >= 2
            && code[code.len() - 1].op == Op::Return
            && code[code.len() - 2].op == Op::Return
        {
            code = &code[..code.len() - 1];
        }
        let base = if proto.is_arrow { 0 } else { 2 };
        let (p0, p1) = (base, base + 1);
        let kind_of = |op: Op| -> u8 {
            match op {
                Op::Sub => 1,
                Op::Add => 2,
                Op::Mul => 3,
                Op::Div => 4,
                Op::Mod => 5,
                Op::Pow => 6,
                Op::Lt => 7,
                Op::Gt => 8,
                Op::Le => 9,
                Op::Ge => 10,
                Op::StrictEq => 11,
                _ => 0,
            }
        };
        // The compiler emits `GetLocal t0, p0; GetLocal t1, p1; OP d, t0, t1; Return d`
        // for `return a - b`. An older shape reads the parameter slots directly.
        if let [a, b, op, ret] = code {
            if a.op == Op::GetLocal
                && b.op == Op::GetLocal
                && ret.op == Op::Return
                && a.b != b.b
                && op.a == ret.a
            {
                let kind = kind_of(op.op);
                let direct = op.b == a.b && op.c == b.b && a.a == b.a && ret.a == a.a;
                let temps = a.b == p0 && b.b == p1 && op.b == a.a && op.c == b.a && a.a != b.a;
                if kind != 0 && (direct || temps) {
                    return kind;
                }
            }
        }
        // `x => x * 2` is one `BinOpK` and a return. `x => x % 3 === 0` is
        // that mod, a load of the zero, and `StrictEq`. Both are 1-param arrows.
        if proto.nparams == 1 {
            if let [op, ret] = code {
                if let Op::BinOpK(sub) = op.op {
                    if (0..=4).contains(&sub)
                        && ret.op == Op::Return
                        && op.a == ret.a
                        && op.b == p0
                        && Self::proto_num(proto, op.c).is_some()
                    {
                        return 32 + sub;
                    }
                }
            }
            if let [m, load, eq, ret] = code {
                if let Op::BinOpK(4) = m.op {
                    if m.b == p0
                        && load.op == Op::LoadK
                        && eq.op == Op::StrictEq
                        && ret.op == Op::Return
                        && eq.b == m.a
                        && eq.c == load.a
                        && eq.a == ret.a
                        && Self::proto_num(proto, m.c).is_some()
                        && Self::proto_num(proto, load.b).is_some()
                    {
                        return 40;
                    }
                }
            }
        }
        if let [op, ret] = code {
            let kind = kind_of(op.op);
            if ret.op == Op::Return && kind != 0 && op.b == p0 && op.c == p1 && op.a == ret.a {
                return kind;
            }
        }
        0
    }

    fn proto_num(proto: &crate::bytecode::FnProto, idx: u32) -> Option<f64> {
        match proto.consts.get(idx as usize)? {
            crate::bytecode::ConstVal::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// `1` when the callback is the numeric `a - b` shape. Sort uses this to
    /// compare two Smis with no frame and no `arith` call.
    pub(crate) fn callback_arith_kind(&mut self, callee: Value) -> u8 {
        if !callee.is_function() {
            return 0;
        }
        let f = unsafe { callee.as_function() };
        let proto: &crate::bytecode::FnProto = match &f.kind {
            FnKind::Bytecode(p) => p.as_ref(),
            _ => return 0,
        };
        if !f.upvalues.is_empty() || proto.is_generator || proto.nupvals != 0 {
            return 0;
        }
        let mut kind = proto.simple_cb.load(std::sync::atomic::Ordering::Relaxed);
        if kind == 0 {
            kind = (Self::classify_simple_cb(proto) << 1) | 1;
            proto
                .simple_cb
                .store(kind, std::sync::atomic::Ordering::Relaxed);
        }
        if kind & 1 == 0 {
            0
        } else {
            kind >> 1
        }
    }

    /// Right-hand numeric constant of a `return p0 <op> k` or `p0 % k === 0` callback.
    pub(crate) fn callback_rhs(&mut self, callee: Value) -> Option<f64> {
        let kind = self.callback_arith_kind(callee);
        if !((32..37).contains(&kind) || kind == 40) {
            return None;
        }
        let f = unsafe { callee.as_function() };
        let proto: &crate::bytecode::FnProto = match &f.kind {
            FnKind::Bytecode(p) => p.as_ref(),
            _ => return None,
        };
        let idx = proto.code.first()?.c;
        Self::proto_num(proto, idx)
    }

    /// Native evaluation of a classified simple callback. Returns `None` when
    /// the callee is not eligible, so the caller falls back to the frame-based
    /// path — the two must agree on the result, and they do: every op below is
    /// the same `arith`/`compare`/`strict_eq` the interpreter calls.
    fn try_fast_callback(&mut self, callee: Value, args: &[Value]) -> Option<Value> {
        if !callee.is_function() || args.is_empty() {
            return None;
        }
        let f = unsafe { callee.as_function() };
        let proto: &crate::bytecode::FnProto = match &f.kind {
            FnKind::Bytecode(p) => p.as_ref(),
            _ => return None,
        };
        // Captures, generators and this-dependent bodies all need a frame.
        if !f.upvalues.is_empty()
            || proto.is_generator
            || proto.nupvals != 0
            || (proto.nparams != 1 && proto.nparams != 2)
        {
            return None;
        }
        if proto.nparams == 2 && args.len() < 2 {
            return None;
        }
        let mut kind = proto.simple_cb.load(std::sync::atomic::Ordering::Relaxed);
        if kind == 0 {
            kind = (Self::classify_simple_cb(proto) << 1) | 1;
            proto
                .simple_cb
                .store(kind, std::sync::atomic::Ordering::Relaxed);
            if std::env::var_os("JHOL_CB_DEBUG").is_some() {
                eprintln!(
                    "cb kind={kind} nparams={} nupvals={} ups={} ops={:?}",
                    proto.nparams,
                    proto.nupvals,
                    f.upvalues.len(),
                    proto
                        .code
                        .iter()
                        .map(|i| (i.op, i.a, i.b, i.c))
                        .collect::<Vec<_>>()
                );
            }
        }
        if kind & 1 == 0 {
            return None; // classified: not simple
        }
        let knd = kind >> 1;
        // 32+sub is `return p0 <op> k` (sub is BinOpK's number). 40 is
        // `return (p0 % k) === 0`. Map/filter arrows are this shape and
        // ignore the index/array arguments the method passes.
        if knd >= 32 {
            let x = args[0];
            if knd == 40 {
                let m = proto.code.first()?;
                let load = proto.code.get(1)?;
                let n = Self::proto_num(proto, m.c)?;
                let z = Self::proto_num(proto, load.b)?;
                let r = self.arith(ArithOp::Mod, x, Value::from_f64(n)).ok()?;
                return Some(Value::from_bool(
                    self.strict_eq(r, Value::from_f64(z)).ok()?,
                ));
            }
            let n = Self::proto_num(proto, proto.code.first()?.c)?;
            let y = Value::from_f64(n);
            let v = match knd - 32 {
                0 => self.arith(ArithOp::Add, x, y).ok()?,
                1 => self.arith(ArithOp::Sub, x, y).ok()?,
                2 => self.arith(ArithOp::Mul, x, y).ok()?,
                3 => self.arith(ArithOp::Div, x, y).ok()?,
                4 => self.arith(ArithOp::Mod, x, y).ok()?,
                _ => return None,
            };
            return Some(v);
        }
        if args.len() < 2 {
            return None;
        }
        let (x, y) = (args[0], args[1]);
        let v = match kind >> 1 {
            1 => self.arith(ArithOp::Sub, x, y).ok()?,
            2 => self.arith(ArithOp::Add, x, y).ok()?,
            3 => self.arith(ArithOp::Mul, x, y).ok()?,
            4 => self.arith(ArithOp::Div, x, y).ok()?,
            5 => self.arith(ArithOp::Mod, x, y).ok()?,
            6 => self.arith(ArithOp::Pow, x, y).ok()?,
            7 => self.compare(x, y, 0).ok()?,
            8 => self.compare(x, y, 1).ok()?,
            9 => self.compare(x, y, 2).ok()?,
            10 => self.compare(x, y, 3).ok()?,
            11 => Value::from_bool(self.strict_eq(x, y).ok()?),
            _ => return None,
        };
        Some(v)
    }

    fn call_direct_args_inner(
        &mut self,
        callee: Value,
        this: Value,
        args: &[Value],
        retain: bool,
    ) -> VmResult<Value> {
        // Simple arithmetic/relational callbacks evaluate natively: no frame
        // push, no stack resize, no param copy. Sort comparators and map
        // callbacks are almost always this shape, and they are the single
        // hottest path outside the interpreter loop.
        if let Some(v) = self.try_fast_callback(callee, args) {
            return Ok(v);
        }
        // Arguments live on the VM stack; bytecode frames root callee/this,
        // and re-entrant natives install their own scope. Duplicating those
        // roots here retained callback allocations and taxed each comparator.
        let scope = self.arena().guest_scope();
        let base = self.stack.len();
        let root_base = self.roots.len();
        let depth = self.frames.len();
        self.stack.extend_from_slice(args);
        self.native_result = Value::UNDEFINED;
        let result = (|| {
            self.call_value(callee, this, base, args.len() as u32, None)?;
            if self.frames.len() > depth {
                self.run_until_inner(depth)
            } else {
                Ok(self.native_result)
            }
        })();
        self.stack.truncate(base);
        self.roots.truncate(root_base);
        drop(scope);
        if retain || result.is_err() {
            self.retain_outcome(&result);
        }
        result
    }

    #[allow(dead_code)]
    fn take_ret(&mut self) -> VmResult<Value> {
        match self.roots.pop() {
            Some(v) => Ok(v),
            None => Ok(Value::UNDEFINED),
        }
    }

    /// Pop the top frame and release everything it owns.
    ///
    /// Handlers installed by the frame are dropped with it. `handle_exception`
    /// skips handlers whose frame is gone, so leaving them behind was harmless
    /// while every return bounced back out to `run()` — but now a whole call
    /// tree can run inside one `step()` invocation, and a `try` body that
    /// returns without executing its `PopTry` would otherwise accumulate one
    /// dead handler per call.
    #[inline]
    fn pop_frame(&mut self) -> Frame {
        let f = self.frames.pop().unwrap();
        while let Some(h) = self.handlers.last() {
            if h.frame_idx >= self.frames.len() {
                self.handlers.pop();
            } else {
                break;
            }
        }
        self.stack.truncate(f.return_stack_base);
        if let Some(g) = f.gen {
            if let Some(state) = self.gens[g].as_mut() {
                state.done = true;
                state.regs = Vec::new();
                state.func = Value::UNDEFINED;
                state.this_val = Value::UNDEFINED;
            }
        }
        f
    }

    #[allow(clippy::too_many_arguments)]
    fn enter_frame(
        &mut self,
        func: Value,
        proto: &FnProto,
        this: Value,
        arg_base: usize,
        argc: u32,
        new_target: Value,
        ret: Option<(usize, u32)>,
    ) -> VmResult<()> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(throw_string(
                "RangeError",
                "Maximum call stack size exceeded",
            ));
        }
        let nslots = proto.nslots;
        let reg_base = self.stack.len();
        // Reserve up front: growing the operand stack one frame per
        // call would re-copy the whole stack on every doubling.
        if self.stack.capacity() < reg_base + nslots + 4096 {
            self.stack.reserve(4096);
        }
        // Zero-fill only when the stack genuinely grows. A frame re-entering
        // a previously deeper window reuses slots holding stale-but-valid
        // Values: the compiler overwrites every slot before reading it, and a
        // GC scan of the window sees valid references either way. This removes
        // one zero-fill of `nslots` words per call — the single largest cost in
        // call-dominated code.
        let is_arrow = proto.is_arrow;
        // The frame window must cover the parameter slots too: non-arrow
        // functions keep params at `2..2+nparams` (arrows at `0..nparams`).
        // A proto whose body uses no locals could otherwise report an nslots
        // smaller than the param region, and the copy below would write past
        // the resize (observed with CommonJS module wrappers: five params, a
        // body with no top-level locals, `nslots` clamped to 2).
        let nslots = nslots.max(if is_arrow {
            proto.nparams
        } else {
            proto.nparams.saturating_add(2)
        });
        let need = reg_base + nslots;
        if self.stack.len() < need {
            self.stack.resize(need, Value::UNDEFINED);
        }
        if !is_arrow {
            let this_v = if this.is_undefined() {
                Value::UNDEFINED
            } else {
                this
            };
            self.stack[reg_base + THIS_SLOT as usize] = this_v;
            self.stack[reg_base + NEWTARGET_SLOT as usize] = Value::UNDEFINED;
        }
        let nparams = proto.nparams;
        let copy = (argc as usize).min(nparams);
        // Arrows own no `this`/`new.target` registers, so their parameters start
        // at slot 0 instead of slot 2.
        let param_base = if is_arrow { 0 } else { 2 };
        for i in 0..copy {
            self.stack[reg_base + param_base + i] = self.stack[arg_base + i];
        }
        if std::env::var_os("JHOL_LIFE_DEBUG").is_some() {
            let nm = if let FnKind::Bytecode(pr) = &unsafe { func.as_function() }.kind {
                pr.name.clone()
            } else {
                "?".into()
            };
            eprintln!(
                "enter {nm}: reg_base={reg_base} arg_base={arg_base} argc={argc} nparams={nparams} nslots={nslots} p0_obj={p0:?} p1_obj={p1:?} copied={copied}",
                nm = nm,
                reg_base = reg_base,
                arg_base = arg_base,
                argc = argc,
                nparams = nparams,
                nslots = nslots,
                p0 = self.stack.get(arg_base).map(|v| v.is_object()),
                p1 = self.stack.get(arg_base + 1).map(|v| v.is_object()),
                copied = (0..copy)
                    .map(|i| {
                        self.stack
                            .get(reg_base + param_base + i)
                            .map(|v| v.is_object())
                            .unwrap_or(false)
                    })
                    .collect::<Vec<bool>>()
                    .iter()
                    .map(|b| if *b { "O" } else { "." })
                    .collect::<String>(),
            );
        }
        let super_ctor = Value::UNDEFINED;
        self.frames.push(Frame {
            func: unsafe { func.as_function_mut() },
            proto: proto as *const FnProto,
            reg_base,
            arg_base,
            stack_base: reg_base,
            return_stack_base: reg_base,
            argc,
            ip: 0,
            this_val: if is_arrow {
                this
            } else {
                self.stack[reg_base + THIS_SLOT as usize]
            },
            new_target: if is_arrow {
                Value::UNDEFINED
            } else {
                new_target
            },
            super_ctor,
            ret,
            gen: None,
            // Empty, not `vec![None; nslots]`: the array only holds
            // string-accumulator buffers, which most frames never use, and
            // allocating it per call was one heap allocation per function
            // invocation. The append/flush paths grow it on demand.
            builders: Vec::new(),
            _scratch: 0,
        });
        self.invalidate_frame_cache();
        Ok(())
    }

    /// Fill a freshly created closure's upvalue array from the currently
    /// running frame, boxing captured locals into cells on first use.
    fn capture_upvalues(&mut self, proto: &FnProto, reg_base: usize, closure: Value) {
        if proto.upval_desc.is_empty() {
            return;
        }
        let mut upvals: Vec<*mut JCell> = Vec::with_capacity(proto.upval_desc.len());
        let fi = self.frames.len() - 1;
        for (from_local, index) in proto.upval_desc.iter() {
            if *from_local {
                let slot_abs = reg_base + *index as usize;
                let cur = self.stack[slot_abs];
                let cell = if cur.is_cell() {
                    unsafe { cur.as_cell_mut() }
                } else {
                    let c = self.arena().alloc_cell(cur);
                    let cv = Value::from_ptr(c);
                    self.stack[slot_abs] = cv;
                    c
                };
                upvals.push(cell);
            } else {
                let cell = unsafe {
                    let fp = self.frames[fi].func;
                    let slot = (*fp).upvalues.as_ptr().add(*index as usize);
                    *slot
                };
                upvals.push(cell);
            }
        }
        // The closure being created is the one most recently allocated.
        if closure.is_function() {
            unsafe {
                self.arena().note_external_allocation(
                    upvals.capacity() * std::mem::size_of::<*mut JCell>(),
                );
                closure.as_function_mut().upvalues = upvals;
            }
        }
    }

    pub fn new_closure(&mut self, proto_idx: u32) -> Value {
        let module_base = self
            .frames
            .last()
            .map_or(0, |frame| unsafe { (*frame.func).module_base });
        let proto = self.protos[module_base + proto_idx as usize].clone();
        let v = self.new_function_native("", |_, _, _| Ok(Value::UNDEFINED), 0);
        let f = unsafe { v.as_function_mut() };
        f.name = Value::UNDEFINED;
        f.kind = FnKind::Bytecode(proto.clone());
        f.module_base = module_base;
        f.proto = Value::from_ptr(self.proto_function as *mut JFunction);
        f.proto_obj = Value::UNDEFINED;
        if proto.is_arrow {
            f.hdr.flags |= crate::objects::FLAG_ARROW;
            f.this_val = self
                .frames
                .last()
                .map_or(Value::UNDEFINED, |frame| frame.this_val);
        }
        v
    }
}

#[inline]
fn is_js_object(value: Value) -> bool {
    matches!(
        unsafe { value.obj_tag() },
        Some(TAG_OBJECT | TAG_ARRAY | TAG_FUNCTION)
    )
}

pub fn js_to_int32(n: f64) -> i32 {
    if n.is_nan() || n.is_infinite() {
        return 0;
    }
    let mut m = n.trunc();
    let two32 = 4294967296.0f64;
    m = m % two32;
    if m < 0.0 {
        m += two32;
    }
    m as i64 as i32
}

#[inline]
pub(crate) fn int32_bitop(op: u8, x: i32, y: i32) -> Value {
    if op == 5 {
        return Value::from_i64(((x as u32) >> (y & 31)) as i64);
    }
    Value::from_i32(match op {
        0 => x & y,
        1 => x | y,
        2 => x ^ y,
        3 => x.wrapping_shl((y & 31) as u32),
        _ => x.wrapping_shr((y & 31) as u32),
    })
}

pub fn js_to_uint32(n: f64) -> u32 {
    js_to_int32(n) as u32
}

#[derive(Debug, Clone, Default)]
pub struct TimerStore {
    pub next_id: u64,
}

// ---------------------------------------------------------------------------
// timers (single-threaded event loop)
// ---------------------------------------------------------------------------

thread_local! {
    static TIMERS: RefCell<Vec<TimerEntry>> = RefCell::new(Vec::new());
}

pub fn push_timer(t: TimerEntry) {
    TIMERS.with(|m| m.borrow_mut().push(t));
}

/// Pop every timer whose deadline has passed. Intervals are re-armed.
pub fn take_due_timers(now: std::time::Instant) -> Vec<TimerEntry> {
    let mut due: Vec<TimerEntry> = Vec::new();
    let mut rearm: Vec<TimerEntry> = Vec::new();
    TIMERS.with(|m| {
        let mut v = m.borrow_mut();
        let mut keep: Vec<TimerEntry> = Vec::new();
        for t in v.drain(..) {
            if t.fire_at <= now {
                due.push(t.clone());
                if t.kind == TimerKind::Interval {
                    rearm.push(TimerEntry {
                        fire_at: now + std::time::Duration::from_millis(t.interval),
                        ..t
                    });
                }
            } else {
                keep.push(t);
            }
        }
        *v = keep;
    });
    for t in rearm {
        push_timer(t);
    }
    due
}

pub fn timer_count() -> usize {
    TIMERS.with(|m| m.borrow().len())
}

pub fn clear_timers() {
    TIMERS.with(|m| m.borrow_mut().clear());
}

// ---------------------------------------------------------------------------
// comparisons & operators
// ---------------------------------------------------------------------------

impl Vm {
    pub fn loose_eq(&mut self, a: Value, b: Value) -> VmResult<bool> {
        if a.is_null() && b.is_undefined() || a.is_undefined() && b.is_null() {
            return Ok(true);
        }
        let both_prim = !(a.is_ptr() && !a.is_function()) || !(b.is_ptr() && !b.is_function());
        if both_prim {
            if a.is_number() && b.is_string() {
                return Ok(a.as_f64() == self.to_number(b)?);
            }
            if a.is_string() && b.is_number() {
                return Ok(self.to_number(a)? == b.as_f64());
            }
            if a.is_bool() && !b.is_bool() {
                let n = if a.as_bool() { 1.0 } else { 0.0 };
                return Ok(n == self.to_number(b)?);
            }
            if b.is_bool() && !a.is_bool() {
                let n = if b.as_bool() { 1.0 } else { 0.0 };
                return Ok(self.to_number(a)? == n);
            }
            return Ok(self.strict_eq(a, b)?);
        }
        let pa = self.to_primitive(a, false)?;
        let pb = self.to_primitive(b, false)?;
        self.loose_eq(pa, pb)
    }

    pub fn strict_eq(&mut self, a: Value, b: Value) -> VmResult<bool> {
        if a.is_number() && b.is_number() {
            if a.is_nan() || b.is_nan() {
                return Ok(false);
            }
            return Ok(a.as_f64() == b.as_f64());
        }
        if a.is_string() && b.is_string() {
            let x = unsafe { a.as_string() };
            let y = unsafe { b.as_string() };
            return Ok(if x.ascii != 0 && y.ascii != 0 {
                unsafe { x.as_str() == y.as_str() }
            } else {
                x.code_units().eq(y.code_units())
            });
        }
        if a.is_ptr() && b.is_ptr() {
            return Ok(a.bits() == b.bits());
        }
        // `===` is strict: `null !== undefined`, so only bit identity counts.
        Ok(a.bits() == b.bits())
    }

    /// `<`, `>`, `<=`, `>=` with the string/numeric split.
    pub fn compare(&mut self, a: Value, b: Value, op: u8) -> VmResult<Value> {
        // Numeric fast path. Loop tests are number-vs-number millions of times;
        // the general path below pays `to_primitive` twice, two `to_number`
        // calls and a string branch. Semantics are identical: `to_primitive` is
        // the identity on numbers, `to_number` too, and NaN compares false —
        // which the general path also produces.
        if a.is_number() && b.is_number() {
            let x = a.as_f64();
            let y = b.as_f64();
            let r = if x.is_nan() || y.is_nan() {
                false
            } else {
                match op {
                    0 => x < y,
                    1 => x > y,
                    2 => x <= y,
                    _ => x >= y,
                }
            };
            return Ok(Value::from_bool(r));
        }
        let pa = self.to_primitive(a, true)?;
        let pb = self.to_primitive(b, true)?;
        let r = if pa.is_string() && pb.is_string() {
            let order = unsafe { pa.as_string() }
                .code_units()
                .cmp(unsafe { pb.as_string() }.code_units());
            match op {
                0 => order.is_lt(),
                1 => order.is_gt(),
                2 => !order.is_gt(),
                _ => !order.is_lt(),
            }
        } else {
            let x = self.to_number(pa)?;
            let y = self.to_number(pb)?;
            if x.is_nan() || y.is_nan() {
                false
            } else {
                match op {
                    0 => x < y,
                    1 => x > y,
                    2 => x <= y,
                    _ => x >= y,
                }
            }
        };
        Ok(Value::from_bool(r))
    }

    pub fn instance_of(&mut self, a: Value, ctor: Value) -> VmResult<bool> {
        if !ctor.is_function() {
            return Err(type_error(
                "Right-hand side of 'instanceof' is not callable",
            ));
        }
        let proto = unsafe { ctor.as_function().proto_obj };
        let mut cur = self.proto_of(a);
        let mut guard = 0;
        while cur.is_object() && guard < 1024 {
            if cur.bits() == proto.bits() {
                return Ok(true);
            }
            cur = self.proto_of(cur);
            guard += 1;
        }
        Ok(false)
    }

    pub fn arith(&mut self, op: ArithOp, a: Value, b: Value) -> VmResult<Value> {
        let _scope = if (a.is_ptr() && !a.is_string()) || (b.is_ptr() && !b.is_string()) {
            Some(self.arena().root_scope(&[a, b]))
        } else {
            None
        };
        match op {
            ArithOp::Add => {
                if let Some(v) = a.try_num_add(&b) {
                    return Ok(v);
                }
                let pa = if !a.is_ptr() || a.is_string() {
                    a
                } else {
                    self.to_primitive(a, false)?
                };
                let pb = if !b.is_ptr() || b.is_string() {
                    b
                } else {
                    self.to_primitive(b, false)?
                };
                if pa.is_string() || pb.is_string() {
                    if (pa.is_string() && unsafe { pa.as_string() }.ascii == 0)
                        || (pb.is_string() && unsafe { pb.as_string() }.ascii == 0)
                    {
                        let mut units: Vec<u16> = if pa.is_string() {
                            unsafe { pa.as_string() }.code_units().collect()
                        } else {
                            self.to_string(pa)?.encode_utf16().collect()
                        };
                        if pb.is_string() {
                            units.extend(unsafe { pb.as_string() }.code_units());
                        } else {
                            units.extend(self.to_string(pb)?.encode_utf16());
                        }
                        return Ok(self.new_string_utf16(&units));
                    }
                    // One buffer, no `format!`: the old
                    // `format!("{}{}", to_string(a), to_string(b))` allocated
                    // two temporary Strings plus the format buffer per `+`.
                    let owned_a;
                    let a_ref: &str = if pa.is_string() {
                        unsafe { pa.as_string().as_str() }
                    } else {
                        owned_a = self.to_string(pa)?;
                        &owned_a
                    };
                    let owned_b;
                    let b_ref: &str = if pb.is_string() {
                        unsafe { pb.as_string().as_str() }
                    } else {
                        owned_b = self.to_string(pb)?;
                        &owned_b
                    };
                    let mut buf = String::with_capacity(a_ref.len() + b_ref.len());
                    buf.push_str(a_ref);
                    buf.push_str(b_ref);
                    return Ok(self.new_string(&buf));
                }
                let x = self.to_number(pa)?;
                let y = self.to_number(pb)?;
                Ok(Value::from_f64(x + y))
            }
            other => {
                // Numeric fast path for +, -, *, /, % on two already-numeric
                // operands. `to_number` on a Smi is nearly free, but the f64
                // `%`/`**` it fed reached libm; the integer remainder path in
                // `try_num_binop` keeps `i % n` in registers.
                if let Some(v) = a.try_num_binop(&b, other) {
                    return Ok(v);
                }
                let x = self.to_number(a)?;
                let y = self.to_number(b)?;
                Ok(Value::from_f64(match other {
                    ArithOp::Sub => x - y,
                    ArithOp::Mul => x * y,
                    ArithOp::Div => x / y,
                    ArithOp::Mod => crate::value::js_mod(x, y),
                    ArithOp::Pow => x.powf(y),
                    ArithOp::Add => unreachable!(),
                }))
            }
        }
    }

    pub fn bitop(&mut self, op: u8, a: Value, b: Value) -> VmResult<Value> {
        let x = if a.is_smi() {
            a.as_i64() as i32
        } else {
            self.to_int32(a)?
        };
        let y = if b.is_smi() {
            b.as_i64() as i32
        } else {
            self.to_int32(b)?
        };
        Ok(int32_bitop(op, x, y))
    }
}

// ---------------------------------------------------------------------------
// the interpreter
// ---------------------------------------------------------------------------

impl Vm {
    /// Execute one instruction.
    /// Rebuild the dispatch cache from the current top frame.
    fn refresh_frame_cache(&mut self) {
        let fi = self.frames.len() - 1;
        let f = &self.frames[fi];
        let (reg_base, ip) = (f.reg_base, f.ip);
        let proto = f.proto_ref();
        let (code, consts, ics) = (
            proto.code.as_ptr(),
            proto.consts.as_ptr(),
            proto.ics.as_ptr(),
        );
        self.fc = FrameCache {
            len: fi + 1,
            fi,
            reg_base,
            code,
            consts,
            ics,
        };
        let _ = ip;
    }

    /// The top frame's *identity* changed. `frames.len()` alone cannot catch
    /// this — a previous call may have left the cache built for a different
    /// frame that happened to sit at the same depth — so a push always drops
    /// the cache instead of trusting the length comparison in `step`.
    #[inline]
    fn invalidate_frame_cache(&mut self) {
        self.fc.len = usize::MAX;
    }

    fn step(&mut self) -> VmResult<Step> {
        let _guest = self.arena().guest_scope();
        self.collect_if_due();
        // A push or pop of the frame stack is the only thing that can change
        // which frame is on top, so that single length comparison replaces the
        // per-instruction Arc walks entirely.
        if self.fc.len != self.frames.len() {
            self.refresh_frame_cache();
        }
        // The non-moving arena base is already owned by this VM. Keep it with
        // the frame pointers instead of asking thread-local storage per load.
        let arena_base = self.arena().base();
        // The instruction pointer stays in a register for as long as we stay
        // in this frame. It is published back to the frame only when the frame
        // stack changes (a call) or the frame is left (return/yield/error) —
        // previously every single instruction paid a bounds-checked
        // read-modify-write through `frames`, plus a `Result`/`Step` round trip
        // back to the caller for an answer nobody read.
        'frames: loop {
            let fi = self.fc.fi;
            let reg_base = self.fc.reg_base;
            let code_ptr = self.fc.code;
            let consts_ptr = self.fc.consts;
            let ics_ptr = self.fc.ics;
            let mut ip = self.frames[fi].ip;

            macro_rules! next {
                () => {{
                    ip += 1;
                    continue;
                }};
            }
            /// Publish the return address and restart on whatever frame is now on
            /// top — a call may have pushed one, so every cached frame pointer has
            /// to be rebuilt before the next instruction is fetched.
            macro_rules! frame_change {
                () => {{
                    self.frames[fi].ip = ip + 1;
                    if self.fc.len != self.frames.len() {
                        self.refresh_frame_cache();
                    }
                    self.collect_if_due();
                    continue 'frames;
                }};
            }
            macro_rules! jump {
                ($t:expr) => {{
                    let target = $t;
                    if target <= ip {
                        self.collect_if_due();
                    }
                    ip = target;
                    continue;
                }};
            }

            loop {
                let ins = unsafe { *code_ptr.add(ip) };
                let a = ins.a;
                let b = ins.b;
                let c = ins.c;
                match ins.op {
                    Op::Nop => next!(),
                    Op::Move => {
                        let v = self.ld(reg_base, b);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::LoadK => {
                        let k = unsafe { &*consts_ptr.add(b as usize) }.clone();
                        let v = self.const_value(&k)?;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::LoadUndefined => {
                        self.st(reg_base, a, Value::UNDEFINED);
                        next!()
                    }
                    Op::LoadNull => {
                        self.st(reg_base, a, Value::NULL);
                        next!()
                    }
                    Op::LoadTrue => {
                        self.st(reg_base, a, Value::TRUE);
                        next!()
                    }
                    Op::LoadFalse => {
                        self.st(reg_base, a, Value::FALSE);
                        next!()
                    }
                    Op::LoadThis => {
                        let v = self.frames[fi].this_val;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::LoadNewTarget => {
                        let v = self.frames[fi].new_target;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::NewObject => {
                        let v = self.new_object();
                        // `b` is a capacity hint: object literals count the properties
                        // they are about to store.
                        if b > 0 {
                            unsafe { v.as_object_mut() }.reserve(b as usize);
                        }
                        self.st(reg_base, a, v);
                        next!()
                    }
                    // Fused object literal: `a` = destination, `b` = const index of the
                    // static key list, `c` = register base holding the values in the
                    // same order. The whole literal — allocation, key list, shape and
                    // property bag — is built here, so a literal no longer pays one
                    // `DefProp` dispatch plus one `shape_for` transition (which was the
                    // hottest leaf in `gc_stress`) per property.
                    Op::NewObjectLit => {
                        let keys: &[PropKey] = match unsafe { &*consts_ptr.add(b as usize) } {
                            ConstVal::Keys(k) => k,
                            _ => &[],
                        };
                        let n = keys.len();
                        // The literal's shape depends only on its static key list, so
                        // it is walked once per call site and cached in the site's IC.
                        let cached = unsafe { (*ics_ptr.add(ip)).obj_shape };
                        let shape = if cached != 0 {
                            cached
                        } else {
                            let mut s = self.empty_obj_shape;
                            for k in keys {
                                s = self.arena().shape_for(s, *k);
                            }
                            unsafe { (*(ics_ptr.add(ip) as *mut Ic)).obj_shape = s };
                            s
                        };
                        let proto = Value::from_ptr_in(self.proto_object, arena_base);
                        let o = self.arena().alloc_object(shape, proto);
                        let v = Value::from_ptr_in(o, arena_base);
                        unsafe {
                            let ob = &mut *o;
                            ob.reserve(n);
                            for (i, k) in keys.iter().enumerate() {
                                ob.push_prop(*k, self.ld(reg_base, c + i as u32));
                            }
                            if n > DENSE_MAX {
                                ob.rebuild_index();
                            }
                        }
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::NewArray => {
                        let n = b as usize;
                        let v = self.new_array();
                        let base = a as usize + 1;
                        for i in 0..n {
                            let x = self.ld(reg_base, (base + i) as u32);
                            self.as_array_mut(v).push(x);
                        }
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::NewArrayLen => next!(),
                    // Load convention: `a` = destination register, `b` = source.
                    Op::LocalAddKJ => {
                        let cur = self.ld(reg_base, a);
                        // `i++` / `i += k` in a loop: the constant is a number, so the
                        // `ConstVal` clone and the `const_value` dispatch go away, and
                        // with them the `arith` call on the overwhelmingly common
                        // integer increment. Non-numeric constants and non-numeric
                        // accumulators take the unchanged `arith` path.
                        let cv = unsafe { &*consts_ptr.add(b as usize) };
                        let y = match cv {
                            ConstVal::Num(n) => Value::from_f64(*n),
                            other => {
                                let k = other.clone();
                                self.const_value(&k)?
                            }
                        };
                        let v = match cur.try_num_add(&y) {
                            Some(v) => v,
                            None => self.arith(ArithOp::Add, cur, y)?,
                        };
                        self.st(reg_base, a, v);
                        let top = c as usize;
                        let proto = self.frames[fi].proto_ref();
                        // Constant-bound loops already compile at JumpLocalCmp. For
                        // ordinary tests, enter after the increment has committed.
                        if proto
                            .code
                            .get(top)
                            .is_some_and(|x| x.op != Op::JumpLocalCmp)
                            && proto.local_cells.is_empty()
                        {
                            let ic = unsafe { &mut *(ics_ptr.add(ip) as *mut Ic) };
                            if ic.jit_state < 2 {
                                ic.hits = ic.hits.saturating_add(1);
                                if ic.hits >= 64 {
                                    let mut consts = |idx| match unsafe { &*consts_ptr.add(idx) } {
                                        ConstVal::Num(n) => Some(*n),
                                        _ => None,
                                    };
                                    crate::jit::bind_region_consts(consts_ptr);
                                    let region = unsafe {
                                        crate::jit::try_compile_region(
                                            &proto.code,
                                            top,
                                            proto.nslots as u32,
                                            &mut consts,
                                            ics_ptr,
                                            arena_base as usize,
                                        )
                                    };
                                    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                        eprintln!(
                                            "[jit] tail-trigger ip={} top={} region_ok={} head_op={:?}",
                                            ip,
                                            top,
                                            region.is_some(),
                                            proto.code.get(top).map(|x| x.op)
                                        );
                                    }
                                    if let Some(region) = region {
                                        ic.jit_code = region.code;
                                        ic.jit_exit = region.exit_ip as u32;
                                        ic.jit_state = 2;
                                        self.jit
                                            .insert((self.frames[fi].proto as usize, ip), region);
                                    } else {
                                        ic.jit_state = 3;
                                    }
                                }
                            }
                            if ic.jit_state == 2 {
                                let regs = unsafe { self.stack.as_mut_ptr().add(reg_base) };
                                crate::jit::bind_region_vm(self);
                                let status = unsafe {
                                    crate::jit::run_raw(ic.jit_code, regs, self.frames.len() as u32)
                                };
                                crate::jit::bind_region_vm(std::ptr::null_mut());
                                if status == 1 {
                                    #[cfg(test)]
                                    {
                                        self.completed_native_loops += 1;
                                    }
                                    jump!(ic.jit_exit as usize);
                                }
                                ic.jit_state = 3;
                                if status >= 2 {
                                    jump!((status - 2) as usize);
                                }
                            }
                        }
                        jump!(top);
                    }
                    Op::LocalAddK => {
                        let cur = self.ld(reg_base, a);
                        let cv = unsafe { &*consts_ptr.add(b as usize) };
                        let y = match cv {
                            ConstVal::Num(n) => Value::from_f64(*n),
                            other => {
                                let k = other.clone();
                                self.const_value(&k)?
                            }
                        };
                        let v = match cur.try_num_add(&y) {
                            Some(v) => v,
                            None => self.arith(ArithOp::Add, cur, y)?,
                        };
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::LocalAdd => {
                        let cur = self.ld(reg_base, a);
                        let rhs = self.ld(reg_base, b);
                        // Same `arith` path as `Op::Add`, so string concat, numeric
                        // fast paths and coercion stay byte-for-byte identical — the
                        // numeric case is just settled before the call.
                        let v = match cur.try_num_add(&rhs) {
                            Some(v) => v,
                            None => self.arith(ArithOp::Add, cur, rhs)?,
                        };
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::GetLocal => {
                        let v = self.ld(reg_base, b);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::SetLocal => {
                        let v = self.ld(reg_base, b);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::CellGet => {
                        let cellv = self.ld(reg_base, b);
                        let v = unsafe { (*cellv.as_cell_mut()).v };
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::CellSet => {
                        let cellv = self.ld(reg_base, a);
                        let v = self.ld(reg_base, b);
                        unsafe {
                            (*cellv.as_cell_mut()).v = v;
                        }
                        next!()
                    }
                    // `s += x` on a local accumulator: append into the frame's byte
                    // buffer instead of allocating a new string. This is the whole
                    // fix for the O(n^2) string build — 200k appends become one
                    // amortised Vec growth rather than 200k full copies.
                    Op::AppendStr => {
                        let v = self.ld(reg_base, b);
                        let fi2 = self.frames.len() - 1;
                        let current = self.ld(reg_base, a);
                        if (v.is_string() && unsafe { v.as_string() }.ascii == 0)
                            || (current.is_string() && unsafe { current.as_string() }.ascii == 0)
                        {
                            if let Some(buf) = self.frames[fi2]
                                .builders
                                .get_mut(a as usize)
                                .and_then(Option::take)
                            {
                                let value = self.new_string(
                                    std::str::from_utf8(&buf).expect("string accumulator UTF-8"),
                                );
                                self.st(reg_base, a, value);
                            }
                            let value = self.arith(ArithOp::Add, self.ld(reg_base, a), v)?;
                            self.st(reg_base, a, value);
                            next!();
                        }
                        let appended = {
                            // A string operand needs no intermediate copy:
                            // `as_str()` is a borrowed view into the arena, and
                            // `to_string` here allocated a whole String per
                            // append purely to hand it to `extend_from_slice`.
                            let s: String;
                            let bytes: &[u8] = if v.is_string() {
                                unsafe { v.as_string().as_str() }.as_bytes()
                            } else {
                                s = self.to_string(v).unwrap_or_default();
                                s.as_bytes()
                            };
                            let slot = a as usize;
                            let frame = &mut self.frames[fi2];
                            if slot >= frame.builders.len() {
                                frame.builders.resize(slot + 1, None);
                            }
                            let buf = frame.builders[slot].get_or_insert_with(|| {
                                // Seed from the current register value on first append.
                                let cur = self.stack[reg_base + slot];
                                match cur.as_str() {
                                    Some(sv) => sv.as_bytes().to_vec(),
                                    None => Vec::new(),
                                }
                            });
                            buf.extend_from_slice(bytes);
                            buf.len()
                        };
                        let _ = appended;
                        next!()
                    }
                    // Any read of an accumulated local materialises it once and drops
                    // the buffer, so later reads take the normal path.
                    Op::FlushStr => {
                        let slot = a as usize;
                        let fi2 = self.frames.len() - 1;
                        let flushed = self.frames[fi2].builders.get(slot).and_then(|b| b.clone());
                        if let Some(buf) = flushed {
                            let sv = String::from_utf8(buf).unwrap_or_default();
                            let v = self.new_string(&sv);
                            self.frames[fi2].builders[slot] = None;
                            self.st(reg_base, a, v);
                        }
                        next!()
                    }
                    Op::CreateCell => {
                        let cur = self.ld(reg_base, a);
                        if !cur.is_cell() {
                            let cp = self.arena().alloc_cell(cur);
                            self.st(reg_base, a, Value::from_ptr(cp));
                        }
                        next!()
                    }
                    Op::RebindCell => {
                        let v = self.ld(reg_base, b);
                        let cp = self.arena().alloc_cell(v);
                        self.st(reg_base, a, Value::from_ptr(cp));
                        next!()
                    }
                    Op::GetUpval => {
                        let cell = unsafe {
                            let fp = self.frames[fi].func;
                            let slot = (*fp).upvalues.as_ptr().add(b as usize);
                            *slot
                        };
                        let v = unsafe { (*cell).v };
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::SetUpval => {
                        let cell = unsafe {
                            let fp = self.frames[fi].func;
                            let slot = (*fp).upvalues.as_ptr().add(a as usize);
                            *slot
                        };
                        let v = self.ld(reg_base, b);
                        unsafe {
                            (*cell).v = v;
                        }
                        next!()
                    }
                    Op::BuildRest => {
                        let ab = self.frames[fi].arg_base;
                        let argc = self.frames[fi].argc;
                        let start = b as usize;
                        let v = self.new_array();
                        for i in start..argc as usize {
                            let x = self.stack[ab + i];
                            self.as_array_mut(v).push(x);
                        }
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::NewClosure => {
                        let k = unsafe { &*consts_ptr.add(b as usize) }.clone();
                        let idx = match k {
                            ConstVal::Fn(i) => i,
                            _ => 0,
                        };
                        let v = self.new_closure(idx);
                        let fp = match &unsafe { v.as_function() }.kind {
                            FnKind::Bytecode(proto) => proto.clone(),
                            _ => unreachable!(),
                        };
                        // Capture cells now, while this frame is still the closure's
                        // lexical parent. Rewiring on every call would be wrong for
                        // recursion: the enclosing frame of a self-call is the function
                        // itself, not the scope that created the closure.
                        self.capture_upvalues(&fp, reg_base, v);
                        unsafe {
                            let f = v.as_function_mut();
                            f.kind = FnKind::Bytecode(fp);
                            if let Some(n) = self.frames.get(fi) {
                                let name = (*n.func).name;
                                let _ = name;
                            }
                        }
                        // Give user functions a `prototype` object.
                        if !unused(v) {
                            let po = self.new_object();
                            unsafe {
                                (*v.as_function_mut()).proto_obj = po;
                                let pn = self.new_string("prototype");
                                let _ = pn;
                            }
                        }
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::GetGlobal => {
                        let k = unsafe { &*consts_ptr.add(b as usize) }.clone();
                        let sym = match k {
                            ConstVal::Str(s) => match s.as_key_str() {
                                Some(t) => self.arena().intern_sym(t),
                                None => 0,
                            },
                            _ => 0,
                        };
                        let v = unsafe { (*self.globals).get(PropKey::Sym(sym)) }
                            .unwrap_or(Value::UNDEFINED);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::SetGlobal => {
                        let k = unsafe { &*consts_ptr.add(a as usize) }.clone();
                        let sym = self.const_sym(&k);
                        let v = self.ld(reg_base, b);
                        self.set_prop(Value::from_ptr(self.globals), PropKey::Sym(sym), v)?;
                        next!()
                    }
                    Op::DefGlobal => {
                        let k = unsafe { &*consts_ptr.add(a as usize) }.clone();
                        let sym = self.const_sym(&k);
                        let v = self.ld(reg_base, b);
                        self.define_prop(Value::from_ptr(self.globals), PropKey::Sym(sym), v);
                        next!()
                    }
                    Op::GetProp => {
                        let obj = self.ld(reg_base, b);
                        // The key lives in the IC entry this instruction already owns.
                        // Re-deriving it via `frames[fi].proto.ics[ip]` walked two
                        // bounds-checked `Vec`s on every property read.
                        let mut ic = unsafe { &mut *(ics_ptr.add(ip) as *mut Ic) };
                        let key = ic.key;
                        // Monomorphic hit without the call. `get_prop_ic` is far too
                        // large to inline into the dispatch loop, so a cached `o.a`
                        // used to pay a call and a full prologue (null check, tag
                        // switch, array/string/function probes) before reaching the
                        // three loads the cache actually needs.
                        if unsafe { obj.obj_tag_in(arena_base) } == Some(TAG_OBJECT) {
                            let o = unsafe { &*obj.ptr_in::<JObject>(arena_base) };
                            if let Some(slot) = ic.slot_for(o.shape) {
                                if let Some((k, v)) = o.get_at(slot as usize) {
                                    if k == key {
                                        ic.hits += 1;
                                        self.st(reg_base, a, v);
                                        next!()
                                    }
                                }
                            }
                        }
                        let v = self.get_prop_ic(obj, key, Some(&mut ic))?;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    // Layout for all three: `a` = object, `b` = key constant index,
                    // `c` = value. `GetProp` differs only in that `a` is the destination.
                    Op::SetProp => {
                        let obj = self.ld(reg_base, a);
                        let ic = unsafe { &mut *(ics_ptr.add(ip) as *mut Ic) };
                        let key = ic.key;
                        let v = self.ld(reg_base, c);
                        // Per-site store cache: when this site last saw a
                        // receiver whose prototype and property epoch matched,
                        // no setter could intercept, so skip the scan. Both
                        // inputs are validated on every write below.
                        let proto = self.proto_of(obj);
                        let pb = if proto.is_ptr() { proto.bits() } else { 0 };
                        let ep = self.arena().property_epoch();
                        if ic.store_seen && ic.store_proto == pb && ic.store_epoch == ep {
                            self.set_prop_impl(obj, key, v, true)?;
                        } else {
                            self.set_prop_impl(obj, key, v, false)?;
                            // Only cache when nothing intercepted: a fired
                            // setter means this site can see accessors.
                            if self.set_scan_clear {
                                ic.store_proto = pb;
                                ic.store_epoch = ep;
                                ic.store_seen = true;
                            }
                        }
                        next!()
                    }
                    Op::DefProp => {
                        let obj = self.ld(reg_base, a);
                        let key = unsafe { (*ics_ptr.add(ip)).key };
                        let v = self.ld(reg_base, c);
                        // Nothing reads FLAG_ACCESSOR any more; the accessor scan is a
                        // plain prototype walk. Keep the flag out so it cannot drift
                        // out of sync with the walk.
                        self.define_prop(obj, key, v);
                        next!()
                    }
                    Op::DeleteProp => {
                        let obj = self.ld(reg_base, b);
                        let key = unsafe { (*ics_ptr.add(ip)).key };
                        let v = self.delete_prop(obj, key);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::DeleteIndex => {
                        let obj = self.ld(reg_base, b);
                        let kv = self.ld(reg_base, c);
                        let key = self.key_from_value(kv);
                        let v = self.delete_prop(obj, key);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::GetIndex => {
                        let obj = self.ld(reg_base, b);
                        let kv = self.ld(reg_base, c);
                        let v = self.get_index(obj, kv)?;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    // Layout matches the compiler: `a` = object, `b` = key, `c` = value.
                    Op::SetIndex => {
                        let obj = self.ld(reg_base, a);
                        let kv = self.ld(reg_base, b);
                        let val = self.ld(reg_base, c);
                        self.set_index(obj, kv, val)?;
                        next!()
                    }
                    Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Pow => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        let op = match ins.op {
                            Op::Add => ArithOp::Add,
                            Op::Sub => ArithOp::Sub,
                            Op::Mul => ArithOp::Mul,
                            Op::Div => ArithOp::Div,
                            Op::Mod => ArithOp::Mod,
                            _ => ArithOp::Pow,
                        };
                        // Two already-numeric operands never reach `arith`'s slow
                        // paths, so the numeric test is done here instead of paying a
                        // call and a `VmResult` for it on every `i - 1` / `x * 2`.
                        let fast = if ins.op == Op::Add {
                            x.try_num_add(&y)
                        } else {
                            x.try_num_binop(&y, op)
                        };
                        if let Some(v) = fast {
                            self.st(reg_base, a, v);
                            next!()
                        }
                        let v = self.arith(op, x, y)?;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::Neg => {
                        let x = self.ld(reg_base, b);
                        let n = self.to_number(x)?;
                        self.st(reg_base, a, Value::from_f64(-n));
                        next!()
                    }
                    Op::Not => {
                        let x = self.ld(reg_base, b);
                        self.st(reg_base, a, Value::from_bool(!x.to_bool()));
                        next!()
                    }
                    Op::BitNot => {
                        let x = self.to_int32(self.ld(reg_base, b))?;
                        self.st(reg_base, a, Value::from_i32(!x));
                        next!()
                    }
                    Op::TypeOf => {
                        let x = self.ld(reg_base, b);
                        let t = x.type_of();
                        let v = self.new_string(t);
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::In => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        let k = self.key_from_value(x);
                        let r = self.has_prop(y, k);
                        self.st(reg_base, a, Value::from_bool(r));
                        next!()
                    }
                    Op::InstanceOf => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        let r = self.instance_of(x, y)?;
                        self.st(reg_base, a, Value::from_bool(r));
                        next!()
                    }
                    Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        let op = match ins.op {
                            Op::BitAnd => 0u8,
                            Op::BitOr => 1,
                            Op::BitXor => 2,
                            Op::Shl => 3,
                            Op::Shr => 4,
                            _ => 5,
                        };
                        // `(i & 1)`, `n >> 2` — two Smis are the whole story, and
                        // `bitop` is a real call, so the integer case is settled here.
                        // The coercion branch below stays identical to `bitop`'s.
                        if x.is_smi() && y.is_smi() {
                            let xi = x.as_i64() as i32;
                            let yi = y.as_i64() as i32;
                            self.st(reg_base, a, int32_bitop(op, xi, yi));
                            next!();
                        }
                        let v = self.bitop(op, x, y)?;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::Eq => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        let r = self.loose_eq(x, y)?;
                        self.st(reg_base, a, Value::from_bool(r));
                        next!()
                    }
                    Op::StrictEq => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        // Smi encoding is canonical, so two integers are `===` exactly
                        // when their bits match. Everything else (strings by content,
                        // objects by identity) still goes through `strict_eq`.
                        let r = if x.is_smi() && y.is_smi() {
                            x.0 == y.0
                        } else {
                            self.strict_eq(x, y)?
                        };
                        self.st(reg_base, a, Value::from_bool(r));
                        next!()
                    }
                    // `dst = regs[b] <op> consts[c]`. One decode, one dispatch: the
                    // `LoadK` that used to materialise the constant is gone, and when
                    // the left operand is a local the `GetLocal` is gone too.
                    Op::BinOpK(sub) => {
                        let x = self.ld(reg_base, b);
                        let cv = unsafe { &*consts_ptr.add(c as usize) };
                        let y = match cv {
                            ConstVal::Num(n) => Value::from_f64(*n),
                            other => {
                                let k = other.clone();
                                self.const_value(&k)?
                            }
                        };
                        let v = if sub <= 4 {
                            let op = match sub {
                                0 => ArithOp::Add,
                                1 => ArithOp::Sub,
                                2 => ArithOp::Mul,
                                3 => ArithOp::Div,
                                _ => ArithOp::Mod,
                            };
                            // Two numeric operands never reach `arith`'s coercion
                            // paths, and `arith` is not inlined into the dispatch loop,
                            // so the check that always succeeds is done here instead.
                            match if sub == 0 {
                                x.try_num_add(&y)
                            } else {
                                x.try_num_binop(&y, op)
                            } {
                                Some(v) => v,
                                None => self.arith(op, x, y)?,
                            }
                        } else if x.is_number() && y.is_number() {
                            let xf = x.as_f64();
                            let yf = y.as_f64();
                            Value::from_bool(
                                !(xf.is_nan() || yf.is_nan())
                                    && match sub {
                                        5 => xf < yf,
                                        6 => xf > yf,
                                        7 => xf <= yf,
                                        _ => xf >= yf,
                                    },
                            )
                        } else {
                            self.compare(x, y, sub - 5)?
                        };
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::Lt | Op::Gt | Op::Le | Op::Ge => {
                        let x = self.ld(reg_base, b);
                        let y = self.ld(reg_base, c);
                        let op = match ins.op {
                            Op::Lt => 0u8,
                            Op::Gt => 1,
                            Op::Le => 2,
                            _ => 3,
                        };
                        // Numeric comparison inlined: `if (n < 2)` in a recursive
                        // function called `compare` on every execution, which showed up
                        // as the second-hottest leaf in that benchmark. Semantics match
                        // `compare`'s number branch exactly (NaN compares false).
                        if x.is_number() && y.is_number() {
                            let xf = x.as_f64();
                            let yf = y.as_f64();
                            let r = !(xf.is_nan() || yf.is_nan())
                                && match op {
                                    0 => xf < yf,
                                    1 => xf > yf,
                                    2 => xf <= yf,
                                    _ => xf >= yf,
                                };
                            self.st(reg_base, a, Value::from_bool(r));
                            next!();
                        }
                        let v = self.compare(x, y, op)?;
                        self.st(reg_base, a, v);
                        next!()
                    }
                    Op::Jump => jump!(c as usize),
                    Op::JumpLocalCmp => {
                        // Counted-loop JIT. The site counts its own executions; after
                        // enough back-edges the loop is compiled to native code and
                        // runs to completion in one call. Any guard failure
                        // deoptimizes to this very instruction, which the interpreter
                        // then evaluates normally.
                        {
                            // SAFETY: `ics_ptr` indexes this frame's own inline-cache
                            // vector, which has one entry per instruction.
                            let ic = unsafe { &mut *(ics_ptr.add(ip) as *mut Ic) };
                            match ic.jit_state {
                                2 => {
                                    // SAFETY: `reg_base + nslots` was resized on frame
                                    // entry, so the pointer stays inside the stack.
                                    let regs = unsafe { self.stack.as_mut_ptr().add(reg_base) };
                                    crate::jit::bind_region_vm(self);
                                    let status = unsafe {
                                        crate::jit::run_raw(
                                            ic.jit_code,
                                            regs,
                                            self.frames.len() as u32,
                                        )
                                    };
                                    crate::jit::bind_region_vm(std::ptr::null_mut());
                                    if status == 1 {
                                        #[cfg(test)]
                                        {
                                            self.completed_native_loops += 1;
                                        }
                                        jump!(ic.jit_exit as usize);
                                    }
                                    if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                        eprintln!(
                                            "[jit] deopt loop ip={} status={} op={:?}",
                                            ip,
                                            status,
                                            status
                                                .checked_sub(2)
                                                .and_then(|next| self.frames[fi]
                                                    .proto_ref()
                                                    .code
                                                    .get(next as usize))
                                                .map(|ins| ins.op)
                                        );
                                    }
                                    ic.jit_state = 3;
                                    if status >= 2 {
                                        jump!((status - 2) as usize);
                                    }
                                }
                                0 | 1 => {
                                    ic.hits = ic.hits.saturating_add(1);
                                    if ic.hits as u32 >= 64 {
                                        let plen = self.frames[fi].proto_ref().code.len();
                                        // The whole function, not just the loop from
                                        // here on: the compiler has to be able to see
                                        // whether anything after the loop still looks
                                        // at a register it is about to treat as a dead
                                        // object literal.
                                        let slice =
                                            unsafe { std::slice::from_raw_parts(code_ptr, plen) };
                                        let paddr = self.frames[fi].proto as usize;
                                        let region_slots =
                                            self.frames[fi].proto_ref().nslots as u32;
                                        let region_safe =
                                            self.frames[fi].proto_ref().local_cells.is_empty();
                                        let arena_base =
                                            unsafe { (*self.arena_ptr).base() as usize };
                                        let mut consts_num = |idx: usize| -> Option<f64> {
                                            let k = unsafe { &*consts_ptr.add(idx) }.clone();
                                            // `const_value` needs `&mut Vm`; the JIT
                                            // compile path owns `&mut self` here, so
                                            // resolve through a short reborrow.
                                            let vm: &mut Vm = self;
                                            let r = vm.const_value(&k);
                                            if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                                eprintln!("[jit] consts({}) -> {:?}", idx, &r);
                                            }
                                            match r {
                                                Ok(v) if v.is_number() => Some(v.as_f64()),
                                                _ => None,
                                            }
                                        };
                                        // The literal's static key list, straight out of
                                        // the constant pool. `Keys` is a boxed slice, so
                                        // borrowing it here is cheap and stable.
                                        let mut consts_keys = |idx: usize| -> Option<Vec<PropKey>> {
                                            match unsafe { &*consts_ptr.add(idx) } {
                                                ConstVal::Keys(k) => Some(k.to_vec()),
                                                _ => None,
                                            }
                                        };
                                        // SAFETY: `ics_ptr` is this proto's own inline-cache array.
                                        crate::jit::bind_region_consts(consts_ptr);
                                        match unsafe {
                                            crate::jit::try_compile(
                                                slice,
                                                ip,
                                                c as usize,
                                                &mut consts_num,
                                                &mut consts_keys,
                                                ics_ptr,
                                            )
                                        }
                                        .or_else(|| {
                                            if region_safe {
                                                unsafe {
                                                    crate::jit::try_compile_region(
                                                        slice,
                                                        ip,
                                                        region_slots,
                                                        &mut consts_num,
                                                        ics_ptr,
                                                        arena_base,
                                                    )
                                                }
                                            } else {
                                                None
                                            }
                                        }) {
                                            Some(jl) => {
                                                let exit = jl.exit_ip as u32;
                                                let code_p = jl.code;
                                                let nbytes = jl.len;
                                                self.jit.insert((paddr, ip), jl);
                                                ic.jit_code = code_p;
                                                ic.jit_exit = exit;
                                                ic.jit_state = 2;
                                                if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                                    eprintln!(
                                                        "[jit] compiled loop at ip={} ({} bytes)",
                                                        ip, nbytes
                                                    );
                                                }
                                            }
                                            None => {
                                                ic.jit_state = 3;
                                                if std::env::var_os("JHOL_JIT_DEBUG").is_some() {
                                                    eprintln!("[jit] ineligible loop at ip={}", ip);
                                                }
                                            }
                                        }
                                    } else {
                                        ic.jit_state = 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                        let x = self.ld(reg_base, a);
                        let code = (b >> 24) as u8;
                        let k = unsafe { &*consts_ptr.add((b & 0x00FF_FFFF) as usize) }.clone();
                        let y = self.const_value(&k)?;
                        // Numeric fast path, resolved here rather than inside
                        // `compare`: a loop test runs millions of times and the call
                        // alone cost more than the whole handler. Semantics match
                        // `compare` exactly for numbers — `to_primitive` and
                        // `to_number` are the identity, and NaN compares false — so
                        // the fused and unfused forms still cannot drift.
                        if x.is_number() && y.is_number() {
                            let xf = x.as_f64();
                            let yf = y.as_f64();
                            let taken = (xf.is_nan() || yf.is_nan())
                                || match code {
                                    0 => !(xf < yf),
                                    1 => !(xf > yf),
                                    2 => !(xf <= yf),
                                    _ => !(xf >= yf),
                                };
                            if taken {
                                jump!(c as usize);
                            }
                            next!()
                        }
                        let v = self.compare(x, y, code)?;
                        if !v.to_bool() {
                            jump!(c as usize);
                        }
                        next!()
                    }
                    Op::JumpIfFalse => {
                        let x = self.ld(reg_base, a);
                        if !x.to_bool() {
                            jump!(c as usize);
                        }
                        next!()
                    }
                    Op::JumpIfTrue => {
                        let x = self.ld(reg_base, a);
                        if x.to_bool() {
                            jump!(c as usize);
                        }
                        next!()
                    }
                    Op::JumpIfNilish => {
                        let x = self.ld(reg_base, a);
                        if x.is_nil() {
                            jump!(c as usize);
                        }
                        next!()
                    }
                    Op::JumpIfNotNilish => {
                        let x = self.ld(reg_base, a);
                        if !x.is_nil() {
                            jump!(c as usize);
                        }
                        next!()
                    }
                    Op::JumpIfNilishKeep => {
                        let x = self.ld(reg_base, a);
                        if x.is_nil() {
                            jump!(c as usize);
                        }
                        next!()
                    }
                    Op::Select => next!(),
                    Op::Call => {
                        let callee = self.ld(reg_base, b);
                        let arg_base = reg_base + b as usize + 1;
                        self.call_value(
                            callee,
                            Value::UNDEFINED,
                            arg_base,
                            c,
                            Some((reg_base, a)),
                        )?;
                        frame_change!()
                    }
                    Op::CallThis => {
                        let this_v = self.ld(reg_base, b);
                        let callee = self.ld(reg_base, b + 1);
                        let arg_base = reg_base + b as usize + 2;
                        self.call_value(callee, this_v, arg_base, c, Some((reg_base, a)))?;
                        frame_change!()
                    }
                    Op::CallMethod => {
                        let recv = self.ld(reg_base, b);
                        let mut ic = unsafe { &mut *(ics_ptr.add(ip) as *mut Ic) };
                        let key = ic.key;
                        let f = self.get_prop_ic(recv, key, Some(&mut ic))?;
                        let arg_base = reg_base + b as usize + 1;
                        self.call_value(f, recv, arg_base, c, Some((reg_base, a)))?;
                        frame_change!()
                    }
                    Op::Construct => {
                        let callee = self.ld(reg_base, b);
                        let arg_base = reg_base + b as usize + 1;
                        let v = self.construct(callee, arg_base, c)?;
                        self.st(reg_base, a, v);
                        frame_change!()
                    }
                    Op::ConstructSuper => {
                        let callee = self.ld(reg_base, b);
                        let arg_base = reg_base + b as usize + 1;
                        let v = self.construct_super(callee, arg_base, c)?;
                        self.stack[reg_base + THIS_SLOT as usize] = v;
                        // `frame.this_val` was cached at entry from the placeholder
                        // object; `super` replaces it, and every later `this` read in
                        // this frame has to see the new one.
                        self.frames[fi].this_val = v;
                        self.st(reg_base, a, v);
                        frame_change!()
                    }
                    Op::ConstructSuperSpread => {
                        // `super(...args)`: the parent constructor is resolved from
                        // `new.target` exactly as in `ConstructSuper`; only the argument
                        // window differs (spread values live in an array register).
                        let list = self.ld(reg_base, b);
                        let mut vals: Vec<Value> = Vec::new();
                        if list.is_array() {
                            let arr = unsafe { list.as_array() };
                            for i in 0..arr.len {
                                vals.push(arr.get(i));
                            }
                        }
                        let base = self.push_args(&vals);
                        let res = self.construct_super(Value::UNDEFINED, base, vals.len() as u32);
                        self.pop_to(base);
                        let v = res?;
                        self.stack[reg_base + THIS_SLOT as usize] = v;
                        self.frames[fi].this_val = v;
                        self.st(reg_base, a, v);
                        frame_change!()
                    }
                    Op::Return => {
                        let v = self.ld(reg_base, a);
                        let f = self.pop_frame();
                        match f.ret {
                            Some((cb, d)) => {
                                self.st(cb, d, v);
                                // Resume the caller *inside* this dispatch loop rather
                                // than returning to `run()` and re-entering. A return
                                // used to cost a `step()` round trip plus a second
                                // frame-cache rebuild on the way back in; now the whole
                                // call tree runs in one `step()` invocation, which is
                                // the hot path of every recursive call. `ret` is only
                                // ever `Some` for a frame that has a caller below it, so
                                // refreshing the cache here is always in range.
                                self.refresh_frame_cache();
                                continue 'frames;
                            }
                            None => {
                                self.last_result = v;
                                return Ok(Step::Done(v));
                            }
                        }
                    }
                    // Suspend. The frame is snapshotted into its generator state and
                    // popped, so the register file is left exactly as it was between
                    // frames; a later `next()` pushes the snapshot back and resumes.
                    Op::Yield => {
                        let v = self.ld(reg_base, a);
                        let gen = match self.frames[fi].gen {
                            Some(g) => g,
                            None => return Err(type_error("yield outside a generator")),
                        };
                        let gs = self.gens[gen].as_mut().unwrap();
                        gs.started = true;
                        gs.ip = ip + 1;
                        gs.resume_slot = Some(a);
                        gs.regs = self.stack[reg_base..reg_base + gs.proto.nslots].to_vec();
                        let f = self.frames.pop().unwrap();
                        // No truncate: the window is reused by the next call,
                        // which is what makes the zero-fill above skippable.
                        // Dead values left in the window stay reachable until
                        // overwritten, bounded by the deepest call depth.
                        return Ok(Step::Yield(v));
                    }
                    Op::Throw => {
                        let v = self.ld(reg_base, a);
                        return Err(crate::errors::Thrown::new(v).into());
                    }
                    Op::ThrowTdz => {
                        let k = unsafe { &*consts_ptr.add(a as usize) }.clone();
                        let name = self.const_value(&k)?;
                        let name = self.to_string(name)?;
                        return Err(crate::errors::reference_error(&format!(
                            "Cannot access '{}' before initialization",
                            name
                        )));
                    }
                    Op::SetProto => {
                        let o = self.ld(reg_base, a);
                        let p = self.ld(reg_base, b);
                        self.set_prototype(o, p)?;
                        next!()
                    }
                    // `b` is the register that receives the thrown value; `c` is the
                    // handler entry point, patched once the try body has been compiled.
                    Op::PushTry => {
                        let slot = reg_base + b as usize;
                        self.handlers.push(Handler {
                            frame_idx: fi,
                            catch_ip: c as usize,
                            slot,
                        });
                        next!()
                    }
                    Op::PopTry => {
                        // A callee must never remove its caller's handler.
                        if self.handlers.last().is_some_and(|h| h.frame_idx == fi) {
                            self.handlers.pop();
                        }
                        next!()
                    }
                    Op::CloseUpvalues => next!(),
                }
            } // inner instruction loop
        } // 'frames: every frame change restarts with fresh pointers
    }

    /// Element read with the array fast path (no property-key allocation).
    pub fn get_index(&mut self, obj: Value, key: Value) -> VmResult<Value> {
        // Dense in-bounds read. The old guard was `i < 4096`, an arbitrary
        // ceiling that sent every access past that index — 98% of a 200k
        // element array — through Value→key conversion and a full property
        // lookup. `i < len` is the real precondition: at or past the length the
        // generic path must run anyway, because that is where growth and the
        // sparse map live.
        if let Some(TAG_ARRAY) = unsafe { obj.obj_tag() } {
            if key.is_smi() {
                let i = key.as_i64();
                if i >= 0 && (i as u32) < unsafe { obj.as_array() }.len {
                    return Ok(unsafe { obj.as_array() }.get(i as u32));
                }
            }
        }
        let k = self.key_for_receiver(obj, key);
        self.get_prop(obj, k)
    }

    pub fn set_index(&mut self, obj: Value, key: Value, val: Value) -> VmResult<()> {
        // Same shape as `get_index`: in-bounds writes are a direct store, and
        // the `i < 4096` ceiling that used to gate this is gone — see above.
        if let Some(TAG_ARRAY) = unsafe { obj.obj_tag() } {
            if key.is_smi() {
                let i = key.as_i64();
                if i >= 0 && (i as u32) < unsafe { obj.as_array() }.len {
                    self.as_array_mut(obj).set(i as u32, val);
                    return Ok(());
                }
            }
        }
        let k = self.key_for_receiver(obj, key);
        self.set_prop(obj, k, val)
    }

    fn const_value(&mut self, k: &ConstVal) -> VmResult<Value> {
        match k {
            ConstVal::Undefined => Ok(Value::UNDEFINED),
            ConstVal::Empty => Ok(Value::EMPTY),
            ConstVal::Null => Ok(Value::NULL),
            ConstVal::True => Ok(Value::TRUE),
            ConstVal::False => Ok(Value::FALSE),
            ConstVal::Num(n) => Ok(Value::from_f64(*n)),
            ConstVal::BigInt(i) => Ok(Value::from_i64(*i)),
            ConstVal::Str(s) => Ok(*s),
            ConstVal::Fn(_) => Ok(Value::UNDEFINED),
            // Structural, not a value: `Op::NewObjectLit` reads it directly from
            // the const pool rather than materialising it.
            ConstVal::Keys(_) => Ok(Value::UNDEFINED),
        }
    }

    fn const_sym(&mut self, k: &ConstVal) -> u32 {
        match k {
            ConstVal::Str(s) => match s.as_key_str() {
                Some(t) => self.arena().intern_sym(t),
                None => 0,
            },
            _ => 0,
        }
    }

    pub fn construct(&mut self, callee: Value, arg_base: usize, argc: u32) -> VmResult<Value> {
        let scope = self.arena().root_scope(&[callee]);
        let values = self.stack[arg_base..arg_base + argc as usize].to_vec();
        self.pin_temporaries(&values);
        let result = self.construct_inner(callee, arg_base, argc);
        drop(scope);
        self.retain_outcome(&result);
        result
    }

    fn construct_inner(&mut self, callee: Value, arg_base: usize, argc: u32) -> VmResult<Value> {
        if !callee.is_function() {
            return Err(type_error("value is not a constructor"));
        }
        let f = unsafe { callee.as_function() };
        if f.hdr.has(FLAG_BOUND) {
            let target = f.this_val;
            let mut args = f.bound.clone();
            args.extend_from_slice(&self.stack[arg_base..arg_base + argc as usize]);
            let base = self.stack.len();
            self.stack.extend_from_slice(&args);
            let result = self.construct(target, base, args.len() as u32);
            self.stack.truncate(base);
            return result;
        }
        if matches!(&f.kind, FnKind::Bytecode(proto) if proto.is_arrow || proto.is_generator) {
            return Err(type_error("value is not a constructor"));
        }
        let is_native = matches!(f.kind, FnKind::Native(_));

        let _ = fallback_unused();
        let default_proto = if f.proto_obj.is_object() {
            f.proto_obj
        } else {
            Value::from_ptr(self.proto_object as *mut JObject)
        };
        let this = Value::from_ptr(self.arena().alloc_object(0, default_proto));
        if is_native {
            // Native constructors (`new Map()`, `new Error()`) are allowed: they
            // get a fresh `this` and may either mutate it or return an object.
            let mut args: Vec<Value> = Vec::with_capacity(argc as usize);
            for i in 0..argc {
                args.push(self.stack[arg_base + i as usize]);
            }
            let f = match f.kind {
                FnKind::Native(nf) => nf,
                _ => unreachable!(),
            };
            let r = f(self, this, &args)?;
            self.native_result = r;
            let out = if is_js_object(r) { r } else { this };
            return Ok(out);
        }
        let proto = match &unsafe { callee.as_function() }.kind {
            FnKind::Bytecode(p) => p.clone(),
            _ => unreachable!(),
        };
        let depth = self.frames.len();
        self.enter_frame(callee, &proto, this, arg_base, argc, callee, None)?;
        let v = self.run_until(depth)?;
        if is_js_object(v) {
            Ok(v)
        } else {
            Ok(this)
        }
    }

    /// `super(...args)` in a derived constructor.
    ///
    /// The parent constructor is derived at runtime from `new.target`:
    /// `new.target.prototype.__proto__.constructor`. That keeps the compiled
    /// code free of any superclass dependency, and makes a later reassignment
    /// of `Circle.prototype.__proto__` visible to `super`.
    fn construct_super(&mut self, _parent: Value, arg_base: usize, argc: u32) -> VmResult<Value> {
        let scope = self.arena().root_scope(&[_parent]);
        let values = self.stack[arg_base..arg_base + argc as usize].to_vec();
        self.pin_temporaries(&values);
        let result = self.construct_super_inner(_parent, arg_base, argc);
        drop(scope);
        self.retain_outcome(&result);
        result
    }

    fn construct_super_inner(
        &mut self,
        _parent: Value,
        arg_base: usize,
        argc: u32,
    ) -> VmResult<Value> {
        let fi = self.frames.len() - 1;
        let target = self.frames[fi].new_target;
        let own_func = self.frames[fi].func;
        let nt = if target.is_function() {
            target
        } else {
            Value::from_ptr(own_func)
        };
        let k = arena::active().intern_sym("prototype");
        let own_proto = self.get_prop(nt, PropKey::Sym(k))?;
        if !own_proto.is_object() {
            return Err(type_error("super called outside a derived constructor"));
        }
        // The super constructor is the *executing* function's [[Prototype]]
        // (ECMAScript GetSuperConstructor), not something derived from
        // `new.target`: in `F -> C -> B`, while C's constructor runs inside
        // `new F()`, `super()` must reach B. Resolving through
        // `new.target.prototype.__proto__.constructor` always yields C again and
        // recursed until the stack limit. The old lookup stays as a fallback for
        // constructors whose own [[Prototype]] was never bound.
        let own_fn = unsafe { Value::from_ptr(own_func).as_function() };
        let bound_parent = own_fn.proto;
        let ctor = if bound_parent.is_function() {
            bound_parent
        } else {
            let parent_proto = unsafe { own_proto.as_object() }.proto;
            let parent_proto = if parent_proto.is_object() {
                parent_proto
            } else {
                Value::from_ptr(self.proto_object)
            };
            let ck = arena::active().intern_sym("constructor");
            self.get_prop(parent_proto, PropKey::Sym(ck))?
        };
        crate::trace(&format!(
            "super nt={:?} own={:x} ctor_fn={}",
            nt.is_function(),
            own_proto.bits() & 0xFFFFFFFFFFF,
            ctor.is_function()
        ));
        if !ctor.is_function() {
            return Err(type_error("no super constructor"));
        }
        let proto = match &unsafe { ctor.as_function() }.kind {
            FnKind::Bytecode(p) => p.clone(),
            FnKind::Native(nf) => {
                // `class MyErr extends Error {}`, `extends Array`, `extends Map`:
                // the parent is a built-in. Construct it the way `construct`
                // does (fresh `this`; the native may return a different object,
                // as `Array` does), then re-parent the result onto the derived
                // class's prototype so `instanceof` and inherited methods see
                // the subclass.
                let nf = *nf;
                let this = Value::from_ptr(self.arena().alloc_object(0, own_proto));
                let mut args: Vec<Value> = Vec::with_capacity(argc as usize);
                for i in 0..argc {
                    args.push(self.stack[arg_base + i as usize]);
                }
                let r = nf(self, this, &args)?;
                self.native_result = r;
                let out = if is_js_object(r) { r } else { this };
                if out.is_array() {
                    unsafe { out.as_array_mut() }.proto = own_proto;
                } else if out.is_object() {
                    unsafe { out.as_object_mut() }.proto = own_proto;
                }
                return Ok(out);
            }
        };
        let this = Value::from_ptr(self.arena().alloc_object(0, own_proto));
        let depth = self.frames.len();
        self.enter_frame(ctor, &proto, this, arg_base, argc, nt, None)?;
        // A parent constructor may itself be derived and replace `this` through
        // its own `super()`; what it returns is the object to carry forward.
        let ret = self.run_until(depth)?;
        if is_js_object(ret) {
            Ok(ret)
        } else {
            Ok(this)
        }
    }

    /// Run until the script frame returns.
    pub fn run(&mut self) -> VmResult<Value> {
        let result = self.run_inner();
        self.retain_outcome(&result);
        result
    }

    fn run_inner(&mut self) -> VmResult<Value> {
        loop {
            if self.frames.is_empty() {
                return Ok(self.last_result);
            }
            match self.step() {
                Ok(Step::Continue) => {}
                Ok(Step::Done(v)) => return Ok(v),
                Ok(Step::Yield(v)) => return Ok(v),
                Err(e) => {
                    if self.handle_exception(e) {
                        continue;
                    }
                    return Err(self.take_error());
                }
            }
        }
    }

    /// Drive the interpreter until fewer than `depth` frames remain. This is how
    /// a builtin invokes a JS callback: it pushes the frame, runs to completion,
    /// and the frame's return value is left in `last_result`.
    pub fn run_until(&mut self, depth: usize) -> VmResult<Value> {
        let result = self.run_until_inner(depth);
        self.retain_outcome(&result);
        result
    }

    fn run_until_inner(&mut self, depth: usize) -> VmResult<Value> {
        loop {
            if self.frames.len() <= depth {
                return Ok(self.last_result);
            }
            match self.step() {
                Ok(Step::Continue) => {}
                Ok(Step::Done(v)) => return Ok(v),
                Ok(Step::Yield(v)) => return Ok(v),
                Err(e) => {
                    if !self
                        .handlers
                        .last()
                        .is_some_and(|handler| handler.frame_idx >= depth)
                    {
                        while self.frames.len() > depth {
                            self.pop_frame();
                        }
                        self.invalidate_frame_cache();
                        return Err(e);
                    }
                    if self.handle_exception(e) {
                        continue;
                    }
                    return Err(self.take_error());
                }
            }
        }
    }

    /// Runtime faults are raised as `"TypeError: message"` strings. JavaScript
    /// expects an object with `name` and `message`, so a caught value is
    /// upgraded once, at the catch, and left alone if it is not one of ours.
    fn error_object_from_thrown(&mut self, v: Value) -> Value {
        if !v.is_string() {
            return v;
        }
        let s = match self.to_string(v) {
            Ok(s) => s,
            Err(_) => return v,
        };
        for name in [
            "TypeError",
            "RangeError",
            "SyntaxError",
            "ReferenceError",
            "EvalError",
            "URIError",
        ] {
            let prefix = format!("{name}: ");
            if let Some(msg) = s.strip_prefix(&prefix) {
                return crate::builtins::make_error_value(self, name, msg);
            }
        }
        v
    }

    fn handle_exception(&mut self, e: VmError) -> bool {
        // Drop handlers that no longer have a live frame.
        while let Some(h) = self.handlers.last() {
            if h.frame_idx >= self.frames.len() {
                self.handlers.pop();
                continue;
            }
            let (fi, catch_ip, slot) = (h.frame_idx, h.catch_ip, h.slot);
            self.handlers.pop();
            let thrown = match e {
                VmError::Exception(t) => self.error_object_from_thrown(t.value),
                VmError::Syntax(m) => crate::builtins::make_error_value(self, "SyntaxError", &m),
                VmError::Internal(m) => {
                    crate::builtins::make_error_value(self, "InternalError", &m)
                }
            };
            self.frames.truncate(fi + 1);
            // Unwind to the handler frame's own register window, not to its
            // argument base: the registers (including the exception slot) have
            // to stay live for the handler to run.
            // Discard only what sits *above* the handler frame's register
            // window. Truncating to the window start instead (and resizing
            // back) would blank registers the body still needs.
            let start = self.frames[fi].stack_base;
            let end = start + self.frames[fi].proto_ref().nslots;
            if self.stack.len() > end {
                self.stack.truncate(end);
            } else if self.stack.len() < end {
                self.stack.resize(end, Value::UNDEFINED);
            }
            let _window = start;
            if slot < self.stack.len() {
                self.stack[slot] = thrown;
            } else {
                self.stack.resize(slot + 1, Value::UNDEFINED);
                self.stack[slot] = thrown;
            }
            self.frames[fi].ip = catch_ip;
            self.invalidate_frame_cache();
            return true;
        }
        self.pending = Some(e);
        false
    }

    fn take_error(&mut self) -> VmError {
        self.pending
            .take()
            .unwrap_or_else(|| VmError::Internal("uncaught exception".into()))
    }
}

fn fallback_unused() {}

fn unused(_v: Value) -> bool {
    false
}

// ---------------------------------------------------------------------------
// engine bootstrap
// ---------------------------------------------------------------------------

impl Vm {
    fn bare() -> Self {
        arena::active().attach_vm();
        Vm {
            stack: Vec::with_capacity(1024),
            frames: Vec::with_capacity(64),
            arena_ptr: arena::active_ptr(),
            jit: std::collections::HashMap::new(),
            #[cfg(test)]
            completed_native_loops: 0,
            set_scan_clear: true,
            setter_scan: None,
            set_clear: [(0, 0, 0, 0); 8],
            set_clear_used: 0,
            fc: FrameCache {
                len: usize::MAX,
                fi: 0,
                reg_base: 0,
                code: std::ptr::null(),
                consts: std::ptr::null(),
                ics: std::ptr::null(),
            },
            handlers: Vec::new(),
            globals: std::ptr::null_mut(),
            module_dirs: Vec::new(),
            protos: Vec::new(),
            roots: Vec::new(),
            last_result: Value::UNDEFINED,
            pending: None,
            native_result: Value::UNDEFINED,
            gens: Vec::new(),
            gen_owners: HashMap::new(),
            free_gens: Vec::new(),
            exit_code: 0,
            timers: Vec::new(),
            timer_seq: 0,
            proto_strings: HashMap::new(),
            proto_string: std::ptr::null_mut(),
            proto_object: std::ptr::null_mut(),
            proto_function: std::ptr::null_mut(),
            proto_array: std::ptr::null_mut(),
            proto_number: std::ptr::null_mut(),
            proto_number_val: Value::UNDEFINED,
            digit_atoms: [Value::UNDEFINED; 10],
            proto_string_val: Value::UNDEFINED,
            proto_array_val: Value::UNDEFINED,
            proto_bool: std::ptr::null_mut(),
            empty_array_shape: 0,
            empty_obj_shape: 0,
            obj_named_fn_fn: None,
            global_ic: HashMap::new(),
            depth_guard: 0,
        }
    }
}

/// What one instruction produced.
#[derive(Debug)]
#[allow(dead_code)]
enum Step {
    Continue,
    Done(Value),
    Yield(Value),
}

/// A suspended generator: its bytecode plus a snapshot of its register file.
#[derive(Debug)]
pub struct GenState {
    proto: Arc<FnProto>,
    /// The generator function closure itself. Resuming must re-enter *this*
    /// function (its upvalue cells live on the closure), not the generator
    /// object. Using the object here read a plain JObject as a JFunction and
    /// crashed on the first `GetUpval`.
    func: Value,
    regs: Vec<Value>,
    ip: usize,
    started: bool,
    done: bool,
    this_val: Value,
    /// Register the paused `yield` wrote its value into. On the next `next(v)`
    /// that slot receives `v`, which is how `const x = yield 1` resumes with a
    /// sent value. `None` until the generator actually suspends at a yield.
    resume_slot: Option<u32>,
}

// ---------------------------------------------------------------------------
// generators, module entry, public engine API
// ---------------------------------------------------------------------------

/// Marker an uncaught internal error carries when it came from `process.exit`.
/// The bridge turns it into an exit code instead of a diagnostic.
pub const EXIT_MARKER: &str = "__jhol_exit:";

pub fn exit_marker(code: i32) -> String {
    format!("{}{}", EXIT_MARKER, code)
}

/// Parse an exit code out of an exit marker, if that is what it is.
pub fn exit_code_of(msg: &str) -> Option<i32> {
    msg.strip_prefix(EXIT_MARKER)
        .and_then(|rest| rest.parse::<i32>().ok())
}

impl Vm {
    /// Call `callee` with `this` and no arguments, returning its value.
    ///
    /// A native callee leaves no frame behind, so its result arrives through
    /// `native_result`; a bytecode callee's value comes from `last_result` once
    /// its frame has returned.
    pub fn call0(&mut self, callee: Value, this: Value) -> VmResult<Value> {
        self.call_direct_args(callee, this, &[])
    }

    /// Calling a `function*` produces a suspended generator
    /// can snapshot it.
    pub fn call_generator(&mut self, callee: Value, this: Value, regs: Vec<Value>) -> Value {
        self.new_generator_object(callee, regs, this)
    }

    /// Calling a `function*` produces a suspended generator: the body has not
    /// run at all, and its first `next()` starts it.
    #[allow(clippy::too_many_arguments)]
    fn make_generator(
        &mut self,
        callee: Value,
        proto: &FnProto,
        this: Value,
        arg_base: usize,
        argc: u32,
        ret: Option<(usize, u32)>,
    ) -> VmResult<()> {
        let nslots = proto.nslots;
        let mut regs = vec![Value::UNDEFINED; nslots];
        let is_arrow = proto.is_arrow;
        if !is_arrow {
            regs[THIS_SLOT as usize] = this;
            regs[NEWTARGET_SLOT as usize] = Value::UNDEFINED;
        }
        let param_base = if is_arrow { 0 } else { 2 };
        let copy = (argc as usize).min(proto.nparams);
        for i in 0..copy {
            regs[param_base + i] = self.stack[arg_base + i];
        }
        let obj = self.new_generator_object(callee, regs, this);
        match ret {
            Some((cb, d)) => self.st(cb, d, obj),
            None => self.native_result = obj,
        }
        Ok(())
    }

    pub fn new_generator_object(&mut self, callee: Value, regs: Vec<Value>, this: Value) -> Value {
        let obj = self.new_object();
        let proto = match &unsafe { callee.as_function() }.kind {
            FnKind::Bytecode(p) => p.clone(),
            _ => unreachable!(),
        };
        let state = GenState {
            proto,
            func: callee,
            regs,
            ip: 0,
            started: false,
            done: false,
            this_val: this,
            resume_slot: None,
        };
        let idx = if let Some(index) = self.free_gens.pop() {
            self.gens[index] = Some(state);
            index
        } else {
            self.gens.push(Some(state));
            self.gens.len() - 1
        };
        self.arena().note_external_allocation(
            self.gens[idx].as_ref().unwrap().regs.capacity() * std::mem::size_of::<Value>(),
        );
        self.gen_owners.insert(obj.bits(), idx);
        unsafe { obj.as_object_mut() }.hdr.flags |= FLAG_GENERATOR;
        let gk = arena::active().intern_sym("#gen");
        self.define_prop(obj, PropKey::Sym(gk), Value::from_i64(idx as i64));
        let nsym = arena::active().intern_sym("next");
        let nf = self.new_function_native("next", crate::builtins::n_gen_next, 0);
        self.define_prop(obj, PropKey::Sym(nsym), nf);
        let isym = arena::active().intern_sym(ITER_SYM);
        let it = self.new_function_native(
            crate::builtins::ITER_SYM_KEY,
            crate::builtins::n_gen_self,
            0,
        );
        self.define_prop(obj, PropKey::Sym(isym), it);
        obj
    }

    pub fn resume(&mut self, gen: Value, sent: Value) -> VmResult<Value> {
        let scope = self.arena().root_scope(&[gen, sent]);
        let result = self.resume_inner(gen, sent);
        drop(scope);
        self.retain_outcome(&result);
        result
    }

    fn resume_inner(&mut self, gen: Value, sent: Value) -> VmResult<Value> {
        let Some(&idx) = self.gen_owners.get(&gen.bits()) else {
            return Err(type_error("not a generator"));
        };
        if self.gens[idx].as_ref().unwrap().done {
            return self.gen_result(gen, Value::UNDEFINED, true);
        }
        let proto = self.gens[idx].as_ref().unwrap().proto.clone();
        let nslots = proto.nslots;
        let reg_base = self.stack.len();
        let ip = if self.gens[idx].as_ref().unwrap().started {
            self.gens[idx].as_ref().unwrap().ip
        } else {
            self.gens[idx].as_mut().unwrap().started = true;
            0
        };
        let mut snapshot = self.gens[idx].as_ref().unwrap().regs.clone();
        if let Some(slot) = self.gens[idx].as_mut().unwrap().resume_slot.take() {
            if let Some(s) = snapshot.get_mut(slot as usize) {
                *s = sent;
            }
        }
        let reg_depth = self.frames.len();
        self.stack.resize(reg_base + nslots, Value::UNDEFINED);
        self.stack[reg_base..reg_base + nslots].copy_from_slice(&snapshot);
        if !proto.is_arrow {
            self.stack[reg_base + THIS_SLOT as usize] = self.gens[idx].as_ref().unwrap().this_val;
        }
        self.frames.push(Frame {
            func: unsafe { self.gens[idx].as_ref().unwrap().func.as_function_mut() },
            proto: std::sync::Arc::as_ptr(&proto),
            reg_base,
            arg_base: reg_base,
            stack_base: reg_base,
            return_stack_base: reg_base,
            argc: 0,
            ip,
            this_val: self.gens[idx].as_ref().unwrap().this_val,
            new_target: Value::UNDEFINED,
            super_ctor: Value::UNDEFINED,
            ret: None,
            gen: Some(idx),
            // Empty, not `vec![None; nslots]`: the array only holds
            // string-accumulator buffers, which most frames never use, and
            // allocating it per call was one heap allocation per function
            // invocation. The append/flush paths grow it on demand.
            builders: Vec::new(),
            _scratch: 0,
        });
        self.invalidate_frame_cache();
        let v = self.run_until(reg_depth)?;
        // `v` is the yielded value, or the generator's `return` value once it
        // completes; both belong in the result object.
        self.gen_result(gen, v, self.gens[idx].as_ref().unwrap().done)
    }

    fn gen_result(&mut self, gen: Value, value: Value, done: bool) -> VmResult<Value> {
        let res = self.new_object();
        let vk = arena::active().intern_sym("value");
        let dk = arena::active().intern_sym("done");
        self.define_prop(res, PropKey::Sym(vk), value);
        self.define_prop(res, PropKey::Sym(dk), Value::from_bool(done));
        let _ = gen;
        Ok(res)
    }

    /// Run a compiled module as the top-level script frame.
    pub fn run_module(&mut self, protos: Vec<Arc<FnProto>>, entry: u32) -> VmResult<Value> {
        let module_base = self.protos.len();
        self.protos.extend(protos);
        let proto = self.protos[module_base + entry as usize].clone();
        let nslots = proto.nslots.max(2);
        let f = JFunction::bytecode(Value::UNDEFINED, proto.clone(), Value::UNDEFINED);
        let fp = self.arena().alloc_function(f);
        let _func = Value::from_ptr(fp);
        unsafe {
            (*fp).proto = Value::from_ptr(self.proto_function_ptr() as *mut JFunction);
            (*fp).hdr.flags |= FLAG_CTOR;
            (*fp).module_base = module_base;
            (*fp).upvalues = Vec::new();
        }
        self.stack.clear();
        self.frames.clear();
        self.handlers.clear();
        self.stack.resize(nslots, Value::UNDEFINED);
        self.stack[THIS_SLOT as usize] = Value::from_ptr(self.globals);
        self.stack[NEWTARGET_SLOT as usize] = Value::UNDEFINED;
        self.frames.push(Frame {
            func: fp,
            proto: std::sync::Arc::as_ptr(&proto),
            reg_base: 0,
            arg_base: 0,
            stack_base: 0,
            return_stack_base: 0,
            argc: 0,
            ip: 0,
            this_val: Value::from_ptr(self.globals),
            new_target: Value::UNDEFINED,
            super_ctor: Value::UNDEFINED,
            ret: None,
            gen: None,
            // Empty, not `vec![None; nslots]`: the array only holds
            // string-accumulator buffers, which most frames never use, and
            // allocating it per call was one heap allocation per function
            // invocation. The append/flush paths grow it on demand.
            builders: Vec::new(),
            _scratch: 0,
        });
        self.invalidate_frame_cache();
        self.run()
    }

    pub fn proto_object_ptr(&self) -> *mut JObject {
        self.proto_object
    }
    pub fn globals_ptr(&self) -> *mut JObject {
        self.globals
    }

    // ---- accessors used by the builtin installer ----
    pub fn set_protos(
        &mut self,
        globals: Value,
        obj_proto: Value,
        fn_proto: Value,
        arr_proto: Value,
        str_proto: Value,
    ) {
        self.globals = unsafe { globals.as_object_mut() };
        self.proto_object = unsafe { obj_proto.as_object_mut() };
        self.proto_function = unsafe { fn_proto.as_function_mut() };
        self.proto_array = unsafe { arr_proto.as_object_mut() };
        self.proto_string = unsafe { str_proto.as_object_mut() };
        self.proto_array_val = Value::from_ptr(self.proto_array);
        self.proto_string_val = Value::from_ptr(self.proto_string);
    }

    pub fn proto_function_ptr(&self) -> *mut JFunction {
        self.proto_function
    }

    pub fn error_proto(&self) -> Value {
        Value::from_ptr(self.proto_object as *mut JObject)
    }

    pub fn set_number_proto(&mut self, p: Value) {
        self.proto_number = unsafe { p.as_object_mut() };
        self.proto_number_val = Value::from_ptr(self.proto_number);
    }
    pub fn set_bool_proto(&mut self, p: Value) {
        self.proto_bool = unsafe { p.as_object_mut() };
    }
    pub fn set_number_proto_last(&mut self) {}
    pub fn next_timer_id(&mut self) -> u64 {
        self.timer_seq += 1;
        self.timer_seq
    }
    pub fn push_args(&mut self, args: &[Value]) -> usize {
        let base = self.stack.len();
        self.stack.extend_from_slice(args);
        base
    }
    pub fn pop_to(&mut self, base: usize) {
        self.stack.truncate(base);
    }
    pub fn construct_at(&mut self, callee: Value, base: usize, argc: u32) -> VmResult<Value> {
        self.construct(callee, base, argc)
    }
}

pub fn cancel_timer(id: u64) {
    TIMERS.with(|m| m.borrow_mut().retain(|t| t.id != id));
}

// ---------------------------------------------------------------------------
// the public engine
impl Engine {
    pub fn new() -> Self {
        clear_timers();
        let arena = Arena::new();
        arena.install();
        Engine {
            vm: Vm::bare(),
            started: false,
        }
    }

    pub fn ensure_started(&mut self) {
        if !self.started {
            self.started = true;
            crate::builtins::install(&mut self.vm);
        }
    }

    /// Compile and run `src`. Returns the completed value of the last expression.
    pub fn eval_source(&mut self, src: &str, name: &str) -> VmResult<Value> {
        self.ensure_started();
        let scope = self.vm.arena().root_scope(&[]);
        let prog = crate::parser::parse_program(src)
            .map_err(|m| VmError::Syntax(format!("{} ({})", m, name)))?;
        let module = crate::compiler::compile(&prog, name)
            .map_err(|m| VmError::Syntax(format!("{} ({})", m, name)))?;
        let out = self.vm.run_module(module.protos, module.entry);
        self.vm.roots.clear();
        drop(scope);
        self.vm.retain_outcome(&out);
        self.vm.collect_if_due();
        out
    }

    pub fn run_file(&mut self, path: &std::path::Path) -> VmResult<i32> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| VmError::Internal(format!("cannot read {}: {}", path.display(), e)))?;
        let name = path.display().to_string();
        // The entry script is a module too: relative `require` calls resolve
        // against its directory for the whole run (including timers fired
        // later — the entry stays on the stack until the event loop drains).
        let dir = path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into());
        self.vm.module_dirs.push(dir);
        let out = self.eval_source(&src, &name);
        if out.is_err() {
            self.vm.module_dirs.pop();
            let e = out.unwrap_err();
            self.run_event_loop()?;
            return Err(e);
        }
        self.run_event_loop()?;
        self.vm.module_dirs.pop();
        Ok(self.vm.exit_code)
    }

    /// Drain pending timers, in deadline order, until none remain.
    pub(crate) fn run_event_loop(&mut self) -> VmResult<()> {
        let mut guard = 0usize;
        loop {
            let now = std::time::Instant::now();
            let due = take_due_timers(now);
            if due.is_empty() {
                let next = TIMERS.with(|timers| timers.borrow().iter().map(|t| t.fire_at).min());
                let Some(deadline) = next else {
                    return Ok(());
                };
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(now)
                        .min(std::time::Duration::from_millis(50)),
                );
                continue;
            }
            let _scope = self.vm.arena().root_scope(&[]);
            for timer in &due {
                self.vm.pin_temporaries(&[timer.cb]);
                self.vm.pin_temporaries(&timer.args);
            }
            for t in due {
                if !t.cb.is_function() {
                    continue;
                }
                let base = self.vm.push_args(&t.args);
                let r = self
                    .vm
                    .call_value_at(t.cb, Value::UNDEFINED, base, t.args.len() as u32);
                self.vm.pop_to(base);
                r?;
            }
            guard += 1;
            if guard > 10_000_000 {
                return Ok(());
            }
        }
    }
}

impl Vm {
    pub fn call_value_at(
        &mut self,
        callee: Value,
        this: Value,
        base: usize,
        argc: u32,
    ) -> VmResult<()> {
        let args = self.stack[base..base + argc as usize].to_vec();
        self.call_direct_args(callee, this, &args).map(|_| ())
    }
}
