//! Heap object layouts for the JholVM arena.
//!
//! Every heap object is `#[repr(C)]`, begins with an [`ObjHeader`], and is
//! allocated inside one flat arena (see `crate::arena`). Because the collector
//! is a **non-moving** mark-sweep, raw pointers to objects stay valid for the
//! object's entire lifetime, which is what lets `Value` carry untagged 44-bit
//! offsets instead of handles and lets the interpreter dereference without a
//! handle-table lookup.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::FnProto;
use crate::errors::{VmError, VmResult};
use crate::value::Value;

pub const TAG_STRING: u32 = 1;
pub const TAG_OBJECT: u32 = 2;
pub const TAG_ARRAY: u32 = 3;
pub const TAG_FUNCTION: u32 = 4;
pub const TAG_CELL: u32 = 5;

pub const FLAG_FROZEN: u32 = 1;
pub const FLAG_SEALED: u32 = 2;
pub const FLAG_STRICT: u32 = 4;
pub const FLAG_CTOR: u32 = 8;
pub const FLAG_ARROW: u32 = 16;
pub const FLAG_BOUND: u32 = 32;
/// Set on a recycled block that currently sits in an arena free list. Blocks
/// carrying this bit are skipped by the linear sweep so nothing is queued twice.
pub const FLAG_FREELIST: u32 = 0x8000_0000;
/// Set on an object that carries (or whose prototype chain carries) an
/// accessor. `set_prop` checks this one bit instead of walking the prototype
/// chain on every store, which is what made `{a:1,b:2,c:3}` literal-heavy code
/// spend its time in the collector's symbol table.
pub const FLAG_ACCESSOR: u32 = 0x4000_0000;
/// Built-in prototypes (`Object.prototype`, `Array.prototype`, …) hold the
/// engine's own methods. In JavaScript those are non-enumerable, so `for-in`
/// must not yield them; the object model has no per-property descriptors yet,
/// so the prototype as a whole is marked and enumeration skips its keys.
pub const FLAG_HIDDEN_KEYS: u32 = 0x2000_0000;
pub const MARK_DEAD: u8 = 0;
/// Marked and queued for tracing.
pub const MARK_QUEUED: u8 = 1;
/// Marked and traced: definitely live until the next sweep.
pub const MARK_LIVE: u8 = 2;

/// Function-shaped native callback.
pub type NativeFn = fn(&mut crate::vm::Vm, Value, &[Value]) -> VmResult<Value>;

/// Every heap object is 8-aligned and begins with this header.
///
/// `#[repr(C, align(8))]` pins the alignment so that objects placed at arena
/// offsets that are multiples of 8 always give their inner `Vec`/`Value` fields
/// correct alignment without per-object fixups.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct ObjHeader {
    pub tag: u32,
    /// Total object size in bytes, rounded up to the arena's 8-byte granularity.
    pub size: u32,
    pub flags: u32,
    pub mark: u8,
    pub pad: [u8; 3],
}

impl ObjHeader {
    #[inline]
    pub const fn new(tag: u32, size: u32, flags: u32) -> Self {
        ObjHeader { tag, size, flags, mark: 0, pad: [0; 3] }
    }
    #[inline]
    pub fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }
}

/// Property key. Identifiers are interned to a dense `u32` symbol; array indices
/// keep their numeric form so that `arr[3]` never pays a string allocation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PropKey {
    Sym(u32),
    Idx(u32),
}

impl PropKey {
    #[inline]
    pub fn discriminant(&self) -> u64 {
        match self {
            PropKey::Idx(i) => 0x1_0000_0000 | (*i as u64),
            PropKey::Sym(s) => 0x2_0000_0000 | (*s as u64),
        }
    }
    /// Ordering used by the shape-signature: indices first (ascending), then
    /// symbols. This is deliberately *not* the JS enumeration order; enumeration
    /// order comes from the insertion-ordered `keys` vector.
    #[inline]
    pub fn shape_rank(&self) -> u64 {
        self.discriminant()
    }
}

/// Strings live inline after the header (flexible-array style), which keeps one
/// allocation per string and avoids a second indirection on every read.
#[repr(C)]
pub struct JString {
    pub hdr: ObjHeader,
    pub len: u32,
    pub hash: u32,
    pub is_atom: u8,
    pub ascii: u8,
    pub pad: [u8; 2],
    pub utf16_len: u32,
}

impl JString {
    #[inline]
    pub fn code_unit(&self, index: usize) -> Option<u16> {
        if index >= self.utf16_len as usize { return None; }
        unsafe {
            let data = (self as *const JString).cast::<u8>().add(32);
            Some(if self.ascii != 0 {
                *data.add(index) as u16
            } else {
                let offset = (self.len as usize + 1) & !1;
                *data.add(offset).cast::<u16>().add(index)
            })
        }
    }

    pub fn code_units(&self) -> impl ExactSizeIterator<Item = u16> + '_ {
        (0..self.utf16_len as usize).map(|i| self.code_unit(i).unwrap())
    }

    #[inline]
    pub unsafe fn as_str(&self) -> &str {
        let base = (self as *const JString).cast::<u8>().add(32);
        let len = self.len as usize;
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(base, len))
    }
}

/// Properties stored inline before the bag spills to the heap.
///
/// Objects are the hottest allocation in the suite — `gc_stress` builds two
/// million four-property literals — and a `Vec` per object meant a `malloc`
/// per object (plus a leak, because the arena recycles blocks with
/// `ptr::write`, which never drops the old bag). Four slots cover every object
/// literal in the benchmark set and the overwhelming majority of literals in
/// real code, so those objects now allocate nothing at all; anything wider
/// spills to a heap `Vec` exactly as before.
pub const INLINE_PROPS: usize = 4;

/// Ordinary object.
///
/// Properties are stored in **insertion order** — inline while short, in a
/// spilled `Vec` once they outgrow [`INLINE_PROPS`]. Lookup is a linear scan
/// while short (the overwhelmingly common case) and upgrades to a hash index
/// once the object grows. `shape` is an interned fingerprint of the key
/// sequence, which is what makes inline caches cheap.
#[repr(C)]
pub struct JObject {
    pub hdr: ObjHeader,
    pub shape: u32,
    pub proto: Value,
    /// Used entries in `small`; meaningless once `spill` is set.
    pub nprops: u32,
    pub extensible: bool,
    /// Heap bag, present only once the object grows past `INLINE_PROPS`.
    /// Boxed so the common object pays one word, not a whole `Vec` header.
    pub spill: Option<Box<Vec<(PropKey, Value)>>>,
    /// Insertion-ordered inline bag: a property's key and value share one
    /// cache line with their neighbours, so a cached read's key check and
    /// value load touch a single line.
    pub small: [(PropKey, Value); INLINE_PROPS],
    /// Key -> slot index, built only for wide objects (> `DENSE_MAX`).
    pub index: Option<Box<HashMap<PropKey, u32>>>,
}

pub const DENSE_MAX: usize = 12;

impl JObject {
    #[inline]
    pub fn new(shape: u32, proto: Value) -> Self {
        JObject {
            hdr: ObjHeader::new(TAG_OBJECT, 0, 0),
            shape,
            proto,
            nprops: 0,
            extensible: true,
            spill: None,
            small: [(PropKey::Sym(0), Value::UNDEFINED); INLINE_PROPS],
            index: None,
        }
    }

    /// The insertion-ordered property slice, inline or spilled.
    #[inline]
    pub fn props(&self) -> &[(PropKey, Value)] {
        match &self.spill {
            Some(v) => v.as_slice(),
            None => &self.small[..self.nprops as usize],
        }
    }

    /// Read a property by slot, copying it out — the shape of every inline
    /// cache site, which only needs the key check and the value.
    #[inline]
    pub fn get_at(&self, i: usize) -> Option<(PropKey, Value)> {
        self.props().get(i).copied()
    }

    /// Iterate the bag in insertion order (used by the collector's tracer).
    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, (PropKey, Value)> {
        self.props().iter()
    }

    #[inline]
    pub fn len(&self) -> usize {
        match &self.spill {
            Some(v) => v.len(),
            None => self.nprops as usize,
        }
    }

    #[inline]
    pub fn get(&self, key: PropKey) -> Option<Value> {
        let p = self.props();
        if p.len() > DENSE_MAX {
            let idx = self.index.as_ref().and_then(|m| m.get(&key).copied());
            return match idx {
                Some(i) => p.get(i as usize).map(|e| e.1),
                None => None,
            };
        }
        let mut i = 0usize;
        while i < p.len() {
            if p[i].0 == key {
                return Some(p[i].1);
            }
            i += 1;
        }
        None
    }

    /// Pre-size the bag. An object literal knows up front how many properties
    /// it is about to store: small literals stay inline and allocate nothing,
    /// wide ones get their single heap allocation here instead of one per push.
    pub fn reserve(&mut self, n: usize) {
        if let Some(v) = &mut self.spill {
            v.reserve(n);
        } else if n > INLINE_PROPS {
            self.spill = Some(Box::new(Vec::with_capacity(n)));
        }
    }

    /// Append without the `find_index` probe — the object-literal path, which
    /// knows the key sequence has no duplicates.
    pub fn push_prop(&mut self, key: PropKey, val: Value) {
        if let Some(v) = &mut self.spill {
            v.push((key, val));
            return;
        }
        let n = self.nprops as usize;
        if n < INLINE_PROPS {
            self.small[n] = (key, val);
            self.nprops = (n + 1) as u32;
        } else {
            // First key past the inline bag: spill, keeping insertion order.
            let mut v: Vec<(PropKey, Value)> = Vec::with_capacity(INLINE_PROPS * 2);
            v.extend_from_slice(&self.small[..n]);
            v.push((key, val));
            self.spill = Some(Box::new(v));
        }
    }

    #[inline]
    fn set_at(&mut self, i: usize, val: Value) {
        match &mut self.spill {
            Some(v) => v[i].1 = val,
            None => self.small[i].1 = val,
        }
    }

    /// Insert or overwrite. Returns `false` when the object is non-extensible
    /// and the key is missing (silently ignored, as in sloppy mode).
    pub fn set(&mut self, key: PropKey, val: Value) -> bool {
        if let Some(i) = self.find_index(key) {
            // A frozen/sealed object refuses to update existing properties.
            if !self.extensible {
                return false;
            }
            self.set_at(i, val);
            return true;
        }
        if !self.extensible {
            return false;
        }
        self.push_prop(key, val);
        if self.len() > DENSE_MAX {
            self.rebuild_index();
        }
        true
    }

    #[inline]
    pub fn find_index(&self, key: PropKey) -> Option<usize> {
        let p = self.props();
        if p.len() > DENSE_MAX {
            return self.index.as_ref().and_then(|m| m.get(&key).copied()).map(|i| i as usize);
        }
        (0..p.len()).find(|&i| p[i].0 == key)
    }

    pub fn rebuild_index(&mut self) {
        let mut m: HashMap<PropKey, u32> = HashMap::with_capacity(self.len() * 2);
        for (i, p) in self.props().iter().enumerate() {
            m.insert(p.0, i as u32);
        }
        self.index = Some(Box::new(m));
    }

    pub fn delete(&mut self, key: PropKey) -> bool {
        if let Some(i) = self.find_index(key) {
            match &mut self.spill {
                Some(v) => {
                    v.remove(i);
                }
                None => {
                    let n = self.nprops as usize;
                    self.small.copy_within(i + 1..n, i);
                    self.nprops -= 1;
                }
            }
            if self.len() > DENSE_MAX {
                self.rebuild_index();
            } else {
                self.index = None;
            }
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.nprops = 0;
        self.spill = None;
        self.index = None;
    }

    pub fn own_keys(&self) -> Vec<PropKey> {
        self.props().iter().map(|p| p.0).collect()
    }
}

/// Array. Elements live in a dense `Vec` (holes are `Value::EMPTY`), with a
/// sparse `HashMap` only for indices far past `len`.
#[repr(C)]
pub struct JArray {
    pub hdr: ObjHeader,
    pub len: u32,
    pub shape: u32,
    pub proto: Value,
    pub data: Vec<Value>,
    pub sparse: Option<HashMap<u32, Value>>,
    pub extensible: bool,
}

/// Objects this far out on a JS array go to the sparse side table instead of
/// being materialised as holes.
pub const SPARSE_FROM: u32 = 4096;

impl JArray {
    pub fn new(shape: u32, proto: Value) -> Self {
        JArray {
            hdr: ObjHeader::new(TAG_ARRAY, 0, 0),
            len: 0,
            shape,
            proto,
            data: Vec::new(),
            sparse: None,
            extensible: true,
        }
    }

    #[inline]
    pub fn get(&self, i: u32) -> Value {
        let i = i as usize;
        if i < self.data.len() {
            let v = self.data[i];
            if v.is_empty() {
                Value::UNDEFINED
            } else {
                v
            }
        } else if let Some(m) = &self.sparse {
            m.get(&(i as u32)).copied().unwrap_or(Value::UNDEFINED)
        } else {
            Value::UNDEFINED
        }
    }

    /// `None` means "not a present element" (used by `in`, `delete`, stores).
    #[inline]
    pub fn get_raw(&self, i: u32) -> Option<Value> {
        let i = i as usize;
        if i < self.data.len() {
            let v = self.data[i];
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        } else {
            self.sparse.as_ref().and_then(|m| m.get(&(i as u32)).copied())
        }
    }

    #[inline]
    pub fn set(&mut self, i: u32, v: Value) {
        if i >= SPARSE_FROM {
            self.sparse.get_or_insert_with(HashMap::new).insert(i, v);
            if i >= self.len {
                self.len = i + 1;
            }
            return;
        }
        let i = i as usize;
        if i >= self.data.len() {
            // The slot at `i` must exist too, so grow past it.
            self.data.resize(i + 1, Value::EMPTY);
        }
        self.data[i] = v;
        if i as u32 >= self.len {
            self.len = (i + 1) as u32;
        }
    }

    #[inline]
    pub fn push(&mut self, v: Value) {
        let n = self.data.len();
        if n >= self.len as usize {
            self.data.push(v);
        } else {
            self.data[n] = v;
        }
        self.len = (self.data.len()) as u32;
    }

    pub fn set_len(&mut self, n: u32) {
        self.len = n;
        if (n as usize) < self.data.len() {
            self.data.truncate(n as usize);
        }
    }

    pub fn own_keys(&self) -> Vec<u32> {
        let mut v = Vec::with_capacity(self.data.len());
        for i in 0..self.data.len() as u32 {
            v.push(i);
        }
        v
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.sparse = None;
        self.len = 0;
    }
}

/// Mutable box for a captured variable. Capturing a local always routes through
/// a cell, so closures never alias a frame slot directly — that keeps frames
/// plain `Value` arrays and removes the whole class of upvalue lifetime bugs.
#[repr(C)]
pub struct JCell {
    pub hdr: ObjHeader,
    pub v: Value,
}

pub enum FnKind {
    Native(NativeFn),
    Bytecode(Arc<FnProto>),
}

/// Function object (also used for constructors, arrows, methods and bound
/// functions).
#[repr(C)]
pub struct JFunction {
    pub hdr: ObjHeader,
    pub kind: FnKind,
    pub name: Value,
    pub upvalues: Vec<*mut JCell>,
    pub this_val: Value,
    pub bound: Vec<Value>,
    /// The function's own `__proto__` (Function.prototype, or the parent class
    /// constructor when the class uses `extends`).
    pub proto: Value,
    /// The `.prototype` property object.
    pub proto_obj: Value,
    /// Functions carry their own property bag (`name`, `length`, `prototype`,
    /// anything the user assigns) with the same store shape as `JObject`, so
    /// one property path serves both.
    pub shape: u32,
    pub keys: Vec<PropKey>,
    pub vals: Vec<Value>,
}

impl JFunction {
    pub fn native(name: Value, f: NativeFn, proto_obj: Value) -> Self {
        JFunction {
            hdr: ObjHeader::new(TAG_FUNCTION, 0, 0),
            kind: FnKind::Native(f),
            name,
            upvalues: Vec::new(),
            this_val: Value::UNDEFINED,
            bound: Vec::new(),
            proto: Value::UNDEFINED,
            proto_obj,
            shape: 0,
            keys: Vec::new(),
            vals: Vec::new(),
        }
    }

    pub fn bytecode(name: Value, proto: Arc<FnProto>, proto_obj: Value) -> Self {
        JFunction {
            hdr: ObjHeader::new(TAG_FUNCTION, 0, 0),
            kind: FnKind::Bytecode(proto),
            name,
            upvalues: Vec::new(),
            this_val: Value::UNDEFINED,
            bound: Vec::new(),
            proto: Value::UNDEFINED,
            proto_obj,
            shape: 0,
            keys: Vec::new(),
            vals: Vec::new(),
        }
    }

    #[inline]
    pub fn is_native(&self) -> bool {
        matches!(self.kind, FnKind::Native(_))
    }
    #[inline]
    pub fn is_arrow(&self) -> bool {
        self.hdr.has(FLAG_ARROW)
    }
    #[inline]
    pub fn is_strict(&self) -> bool {
        self.hdr.has(FLAG_STRICT)
    }
    #[inline]
    pub fn is_ctor(&self) -> bool {
        self.hdr.has(FLAG_CTOR)
    }

    #[inline]
    /// Functions have no index side-table, so a plain scan is always used here
    /// (a builtin object can easily exceed `DENSE_MAX` properties).
    pub fn get(&self, key: PropKey) -> Option<Value> {
        (0..self.keys.len()).find(|&i| self.keys[i] == key).map(|i| self.vals[i])
    }

    #[inline]
    pub fn find_index(&self, key: PropKey) -> Option<usize> {
        (0..self.keys.len()).find(|&i| self.keys[i] == key)
    }

    pub fn set(&mut self, key: PropKey, val: Value) {
        if let Some(i) = self.find_index(key) {
            self.vals[i] = val;
            return;
        }
        self.keys.push(key);
        self.vals.push(val);
    }

    pub fn delete(&mut self, key: PropKey) -> bool {
        if let Some(i) = self.find_index(key) {
            self.keys.remove(i);
            self.vals.remove(i);
            true
        } else {
            false
        }
    }

    pub fn arity(&self) -> u32 {
        match &self.kind {
            FnKind::Native(_) => 0,
            FnKind::Bytecode(p) => p.nparams as u32,
        }
    }
}

pub fn is_callable(v: Value) -> bool {
    v.is_function()
}
