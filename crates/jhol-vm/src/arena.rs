//! One flat 16 TiB arena + a non-moving mark-sweep collector.
//!
//! # Why an arena instead of `Box` per object
//!
//! Boxing every object individually (a) costs a `malloc` per allocation, (b)
//! makes every heap reference a real pointer whose move behaviour we would have
//! to track, and (c) prevents `Value` from being a single word. Instead the
//! engine reserves one huge address range once, commits pages on demand, and
//! bump-allocates inside it. An offset below 2^44 is all a `Value` ever needs.
//!
//! # Why mark-sweep and not copying
//!
//! A semispace copier is faster for the young generation, but it *moves*
//! objects, which invalidates every `&'static JObject` handed out by
//! [`crate::value::Value::as_object`] and every `*mut JCell` held by a closure.
//! The whole interpreter is written in terms of those stable references, so
//! non-moving mark-sweep is the design the rest of the engine is built on.
//! Freeing a block keeps its header (`size` stays valid) so sweeping is a
//! single linear walk with no auxiliary metadata.
//!
//! # Block protocol
//!
//! ```text
//! tag != 0, mark == LIVE      live object
//! tag != 0, mark == QUEUED    live, tracing in progress
//! tag != 0, mark == DEAD      garbage, about to be freed
//! tag == 0, FREELIST flag     already sitting in a size-class free list
//! ```
//! Splitting a recycled block writes a `tag == 0` tail header, which keeps the
//! linear sweep valid forever.
//!
//! # Shape interning
//!
//! Property-addition histories intern into `u32` shape ids through one
//! transition table keyed `(shape, PropKey) -> shape`. Two objects that gained
//! the same keys in the same order share a shape, which is exactly what the
//! interpreter's inline caches validate against.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::objects::*;
use crate::value::{Value, OFFSET_MAX};

/// Largest size that goes into a size-class free list (512 B => 64 classes).
const CLASS_MAX: usize = 512;
const NCLASSES: usize = 65;

static ALLOCS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The arena for this thread. One `Engine` per thread, and every engine
    /// installs its own, so tests and callers cannot trip over each other.
    static ARENA: Cell<*mut Arena> = const { Cell::new(std::ptr::null_mut()) };
}

/// Start of this thread's arena reservation — the base every `Value` offset is
/// relative to. This is the *reservation*, not the address of the `Arena`
/// struct; mixing the two up makes every dereference miss.
#[inline]
pub fn arena_base() -> *mut u8 {
    let p = ARENA.with(|c| c.get());
    if p.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { (*p).base }
}

#[inline]
pub fn has_arena() -> bool {
    ARENA.with(|c| !c.get().is_null())
}

/// Raw pointer to this thread's arena, or null when none is installed.
///
/// `active()` goes through `thread_local!(...).with(...)`, which costs ~8ns per
/// call on this platform — and every allocation and property store needs the
/// arena. Callers that run per instruction cache this once instead.
#[inline]
pub fn active_ptr() -> *mut Arena {
    ARENA.with(|c| c.get())
}

/// This thread's arena. Panics if no engine was started on this thread.
#[inline]
pub fn active() -> &'static mut Arena {
    let p = ARENA.with(|c| c.get());
    debug_assert!(!p.is_null(), "JholVM: arena not installed");
    unsafe { &mut *p }
}

#[inline]
fn class_of(size: usize) -> usize {
    let s = size.div_ceil(8);
    if s <= CLASS_MAX / 8 {
        s
    } else {
        NCLASSES
    }
}

pub struct Arena {
    base: *mut u8,
    reserved: usize,
    bump: usize,
    free: Vec<Vec<u32>>,
    symbols: HashMap<Box<str>, u32>,
    symbol_names: Vec<Box<str>>,
    transitions: HashMap<u128, u32>,
    /// Direct-mapped caches for the two lookups every property store performs.
    /// Object literals replay the same `(shape, key)` and accessor-key
    /// sequences on every iteration, so a few dozen slots capture nearly all of
    /// them and remove a hash probe from the hot path. Entries are only valid
    /// where the matching bit in `*_used` is set, so a real key can never alias
    /// an empty slot.
    trans_ic: [(u128, u32); 32],
    trans_ic_used: u32,
    acc_ic: [(u64, u32); 32],
    acc_ic_used: u32,
    /// Next hidden-class id. Shapes are interned in `transitions` by
    /// `(parent shape, key)`, so identity is structural and nothing else needs
    /// to be stored per shape.
    next_shape: u32,
    accessor_map: HashMap<u64, u32>,
    atoms: HashMap<Box<str>, Value>,
    atom_roots: Vec<Value>,
    live_bytes: usize,
    peak_bytes: usize,
    collections: u64,
    total_alloc: u64,
    total_freed: u64,
    heap_limit: usize,
    gc_disabled: bool,
}

unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

/// Reservation sizes to try, largest first.
///
/// The value offset space is 44 bits (16 TiB), so a reservation that large is
/// *possible* — but reserving it eagerly wastes address space in a process that
/// hosts several engines (tests, embedded runtimes). The default is therefore a
/// few gigabytes, which is plenty for a package-manager script and leaves room
/// for the rest of the process. Set `JHOL_VM_ARENA_BYTES` for explicit control.
const RESERVE_CANDIDATES: [usize; 6] = [
    64 * 1024 * 1024 * 1024, // 64 GiB
    8 * 1024 * 1024 * 1024,  // 8 GiB
    1 * 1024 * 1024 * 1024,  // 1 GiB
    256 * 1024 * 1024,       // 256 MiB
    64 * 1024 * 1024,        // 64 MiB
    4 * 1024 * 1024,         // 4 MiB
];

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

impl Arena {
    pub fn new() -> Self {
        let override_bytes = std::env::var("JHOL_VM_ARENA_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok());
        let mut base = std::ptr::null_mut();
        let mut reserved = 0usize;
        let mut candidates: Vec<usize> = override_bytes.into_iter().collect();
        candidates.extend_from_slice(&RESERVE_CANDIDATES);
        for want in candidates {
            if let Some(p) = reserve(want) {
                base = p;
                reserved = want;
                break;
            }
        }
        let heap_limit = std::env::var("JHOL_VM_HEAP_LIMIT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(384 * 1024 * 1024);
        Arena {
            base,
            reserved,
            bump: 0,
            free: (0..=NCLASSES).map(|_| Vec::new()).collect(),
            symbols: HashMap::new(),
            symbol_names: Vec::new(),
            transitions: HashMap::new(),
            trans_ic: [(0, 0); 32],
            trans_ic_used: 0,
            acc_ic: [(0, 0); 32],
            acc_ic_used: 0,
            next_shape: 1,
            accessor_map: HashMap::new(),
            atoms: HashMap::new(),
            atom_roots: Vec::new(),
            live_bytes: 0,
            peak_bytes: 0,
            collections: 0,
            total_alloc: 0,
            total_freed: 0,
            heap_limit,
            gc_disabled: std::env::var("JHOL_VM_NO_GC").is_ok(),
        }
    }

    #[inline]
    pub fn base(&self) -> *mut u8 {
        self.base
    }
    #[inline]
    pub fn offset(&self) -> usize {
        self.bump
    }
    #[inline]
    pub fn live_bytes(&self) -> usize {
        self.live_bytes
    }
    #[inline]
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }
    #[inline]
    pub fn reserved(&self) -> usize {
        self.reserved
    }

    pub fn stats(&self) -> String {
        format!(
            "heap: {}B live / {}B peak / {}B reserved, bump={}B, collections={}, alloc={}B, freed={}B, symbols={}, shapes={}",
            self.live_bytes,
            self.peak_bytes,
            self.reserved,
            self.bump,
            self.collections,
            self.total_alloc,
            self.total_freed,
            self.symbols.len(),
            self.next_shape - 1
        )
    }

    /// Install as this thread's arena. Called once at engine startup.
    ///
    /// A thread hosts one arena at a time, so any previous reservation is
    /// unmapped and dropped first: otherwise a process that builds several
    /// engines leaks 16 TiB of address space each time.
    /// Give up this thread's arena: unmap the reservation and drop it. Called
    /// when the `Engine` that installed it goes away.
    pub fn uninstall() {
        ARENA.with(|c| {
            let p = c.get();
            if p.is_null() {
                return;
            }
            c.set(std::ptr::null_mut());
            unsafe {
                let a = &mut *p;
                unmap(a.base, a.reserved);
                drop(Box::from_raw(p));
            }
        });
    }

    pub fn install(self) -> &'static mut Arena {
        ARENA.with(|c| {
            let old = c.get();
            if !old.is_null() {
                unsafe {
                    let doomed = &mut *old;
                    unmap(doomed.base, doomed.reserved);
                    drop(Box::from_raw(old));
                }
            }
        });
        let ptr = Box::into_raw(Box::new(self));
        ARENA.with(|c| c.set(ptr));
        unsafe { &mut *ptr }
    }

    // ---------------- allocation ----------------

    fn take_block(&mut self, need: usize) -> Option<u32> {
        let cls = class_of(need);
        if cls == NCLASSES {
            let mut best: Option<(usize, usize)> = None;
            for (i, &off) in self.free[NCLASSES].iter().enumerate() {
                let sz = unsafe { (*self.block(off as usize)).size as usize };
                if sz >= need && (best.is_none() || sz < best.unwrap().1) {
                    best = Some((i, sz));
                }
            }
            return best.map(|(i, _)| self.free[NCLASSES].swap_remove(i));
        }
        // Every block in class `cls` is exactly 8*cls bytes, so the first hit
        // is always usable.
        self.free[cls].pop()
    }

    #[inline]
    unsafe fn block(&self, off: usize) -> *mut ObjHeader {
        self.base.add(off) as *mut ObjHeader
    }

    /// Hand out `need` bytes (rounded up to 8) and return its arena offset.
    pub fn alloc_raw(&mut self, need: usize) -> u32 {
        let need = need.div_ceil(8) * 8;
        if let Some(off) = self.take_block(need) {
            let actual = unsafe { (*self.block(off as usize)).size as usize };
            if actual > need {
                let rem = actual - need;
                unsafe {
                    std::ptr::write(
                        self.block(off as usize + need),
                        ObjHeader::new(0, rem as u32, FLAG_FREELIST),
                    );
                }
                self.free[class_of(rem)].push((off as usize + need) as u32);
            }
            self.account(need);
            return off;
        }
        let off = self.bump;
        let end = off + need;
        if end > self.reserved {
            self.oom();
        }
        self.bump = end;
        self.account(need);
        off as u32
    }

    fn account(&mut self, need: usize) {
        self.live_bytes += need;
        self.total_alloc += need as u64;
        if self.live_bytes > self.peak_bytes {
            self.peak_bytes = self.live_bytes;
        }
        ALLOCS.fetch_add(1, Ordering::Relaxed);
    }

    fn oom(&self) -> ! {
        panic!(
            "JholVM: arena exhausted ({}B live, {}B reserved)",
            self.live_bytes, self.reserved
        );
    }

    /// Allocate a `#[repr(C)]` object and return the raw pointer.
    pub fn alloc_obj<T: HasHeader>(&mut self, make: impl FnOnce() -> T) -> *mut T {
        let need = std::mem::size_of::<T>().div_ceil(8) * 8;
        let off = self.alloc_raw(need);
        unsafe {
            let p = self.block(off as usize) as *mut T;
            std::ptr::write(p, make());
            (*p).hdr_mut().size = need as u32;
            p
        }
    }

    /// Strings keep their bytes inline after the (8-aligned) header.
    pub fn alloc_string(&mut self, s: &str) -> *mut JString {
        if s.is_ascii() { self.alloc_string_data(s, None) }
        else { self.alloc_string_data(s, Some(&s.encode_utf16().collect::<Vec<_>>())) }
    }

    pub fn alloc_string_utf16(&mut self, units: &[u16]) -> *mut JString {
        if units.iter().all(|&c| c < 128) {
            let bytes: Vec<u8> = units.iter().map(|&c| c as u8).collect();
            self.alloc_string_data(unsafe { std::str::from_utf8_unchecked(&bytes) }, None)
        } else {
            self.alloc_string_data(&String::from_utf16_lossy(units), Some(units))
        }
    }

    fn alloc_string_data(&mut self, s: &str, units: Option<&[u16]>) -> *mut JString {
        let units_off = (s.len() + 1) & !1;
        let payload = units.map_or(s.len(), |u| units_off + u.len() * 2);
        let need = (32 + payload).div_ceil(8) * 8;
        let off = self.alloc_raw(need);
        unsafe {
            let p = self.block(off as usize) as *mut JString;
            std::ptr::write(p, JString {
                hdr: ObjHeader::new(TAG_STRING, need as u32, 0),
                len: s.len() as u32,
                hash: hash_bytes(s.as_bytes()),
                is_atom: 0,
                ascii: units.is_none() as u8,
                pad: [0; 2],
                utf16_len: units.map_or(s.len(), |u| u.len()) as u32,
            });
            let data = (p as *mut u8).add(32);
            std::ptr::copy_nonoverlapping(s.as_ptr(), data, s.len());
            if let Some(u) = units {
                std::ptr::copy_nonoverlapping(u.as_ptr(), data.add(units_off).cast::<u16>(), u.len());
            }
            p
        }
    }

    pub fn alloc_object(&mut self, shape: u32, proto: Value) -> *mut JObject {
        self.alloc_obj(|| JObject::new(shape, proto))
    }

    pub fn alloc_array(&mut self, shape: u32, proto: Value) -> *mut JArray {
        self.alloc_obj(|| JArray::new(shape, proto))
    }

    pub fn alloc_cell(&mut self, v: Value) -> *mut JCell {
        self.alloc_obj(|| JCell { hdr: ObjHeader::new(TAG_CELL, 0, 0), v })
    }

    pub fn alloc_function(&mut self, f: JFunction) -> *mut JFunction {
        self.alloc_obj(move || f)
    }

    // ---------------- interning ----------------

    pub fn intern_sym(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.symbols.get(name) {
            return id;
        }
        let id = self.symbol_names.len() as u32;
        let b: Box<str> = name.into();
        self.symbol_names.push(b.clone());
        self.symbols.insert(b, id);
        id
    }

    pub(crate) fn intern_atom_utf16(&mut self, units: &[u16]) -> Value {
        if let Ok(s)=String::from_utf16(units) { return self.intern_atom(&s); }
        let p=self.alloc_string_utf16(units);
        unsafe { (*p).is_atom=1; }
        let v=Value::from_ptr(p);self.atom_roots.push(v);v
    }

    pub fn sym_name(&self, id: u32) -> &str {
        &self.symbol_names[id as usize]
    }

    /// Intern a string as an atom: identity-stable and permanently rooted, so
    /// comparing two atoms is a word compare.
    pub fn intern_atom(&mut self, s: &str) -> Value {
        if let Some(&v) = self.atoms.get(s) {
            return v;
        }
        let p = self.alloc_string(s);
        unsafe { (*p).is_atom = 1 };
        let v = Value::from_ptr(p);
        self.atom_roots.push(v);
        self.atoms.insert(s.into(), v);
        v
    }

    pub fn shape_for(&mut self, parent: u32, key: PropKey) -> u32 {
        let k = transition_key(parent, key);
        // Mix both halves into the slot: `key` ranks live in the low bits and
        // `parent` in the high ones, and both need to spread the table.
        let slot = ((k ^ (k >> 64)) as usize) & 31;
        let bit = 1u32 << slot;
        if self.trans_ic_used & bit != 0 {
            let (ek, ev) = self.trans_ic[slot];
            if ek == k {
                return ev;
            }
        }
        if let Some(&s) = self.transitions.get(&k) {
            self.trans_ic[slot] = (k, s);
            self.trans_ic_used |= bit;
            return s;
        }
        // A shape is identified by its transition path, so the id alone is the
        // whole representation: no key list is materialised or copied.
        let id = self.next_shape;
        self.next_shape += 1;
        self.transitions.insert(k, id);
        self.trans_ic[slot] = (k, id);
        self.trans_ic_used |= bit;
        id
    }

    /// Symbol for the hidden `#get:<name>` / `#set:<name>` accessor slot.
    pub fn accessor_sym(&mut self, base: u32, prefix: &str) -> u32 {
        // The cache key must distinguish "#get:" from "#set:" — encoding only
        // the length would alias them (both are five bytes long).
        let k = (base as u64) | ((prefix.len() as u64) << 32) | ((hash_bytes(prefix.as_bytes()) as u64) << 40);
        let slot = ((k ^ (k >> 32)) as usize) & 31;
        let bit = 1u32 << slot;
        if self.acc_ic_used & bit != 0 {
            let (ek, ev) = self.acc_ic[slot];
            if ek == k {
                return ev;
            }
        }
        if let Some(&m) = self.accessor_map.get(&k) {
            self.acc_ic[slot] = (k, m);
            self.acc_ic_used |= bit;
            return m;
        }
        let name = format!("{}{}", prefix, self.sym_name(base));
        let id = self.intern_sym(&name);
        self.accessor_map.insert(k, id);
        self.acc_ic[slot] = (k, id);
        self.acc_ic_used |= bit;
        id
    }

    // ---------------- collection ----------------

    /// Mark-sweep. `roots` are slices of live `Value`s (register frames, the
    /// global object set, …). Atoms are always rooted by the arena itself.
    pub fn collect(&mut self, roots: &[&[Value]]) {
        if self.gc_disabled {
            return;
        }
        self.collections += 1;
        let mut work: Vec<*mut ObjHeader> = Vec::new();
        for r in roots {
            for v in *r {
                self.mark_value(*v, &mut work);
            }
        }
        for v in self.atom_roots.clone() {
            self.mark_value(v, &mut work);
        }
        while let Some(h) = work.pop() {
            if unsafe { (*h).mark } == MARK_QUEUED {
                unsafe { (*h).mark = MARK_LIVE };
                let off = (h as usize) - (self.base as usize);
                self.trace(off, &mut work);
            }
        }
        self.sweep();
    }

    fn mark_value(&mut self, v: Value, work: &mut Vec<*mut ObjHeader>) {
        if !v.is_ptr() {
            return;
        }
        let off = (v.bits() & OFFSET_MAX) as usize;
        let h = unsafe { self.block(off) };
        if unsafe { (*h).mark } == MARK_DEAD {
            unsafe { (*h).mark = MARK_QUEUED };
            work.push(h);
        }
    }

    fn mark_cell(&self, c: *mut JCell, work: &mut Vec<*mut ObjHeader>) {
        if c.is_null() {
            return;
        }
        unsafe {
            if (*c).hdr.mark == MARK_DEAD {
                (*c).hdr.mark = MARK_QUEUED;
                work.push(c as *mut ObjHeader);
            }
        }
    }

    fn trace(&mut self, off: usize, work: &mut Vec<*mut ObjHeader>) {
        // Slots are read straight out of the object and pushed as pointers; no
        // intermediate `Vec<Value>` is built per object. Tracing a large graph
        // allocated one vector per node, which made the collector itself the
        // allocation hotspot under churn.
        let tag = unsafe { (*self.block(off)).tag };
        unsafe {
            match tag {
                TAG_OBJECT => {
                    let o = &*(self.block(off) as *const JObject);
                    self.mark_value(o.proto, work);
                    for (_, x) in o.iter() {
                        self.mark_value(*x, work);
                    }
                }
                TAG_ARRAY => {
                    let a = &*(self.block(off) as *const JArray);
                    self.mark_value(a.proto, work);
                    for x in a.data.iter() {
                        self.mark_value(*x, work);
                    }
                    if let Some(m) = &a.sparse {
                        for val in m.values() {
                            self.mark_value(*val, work);
                        }
                    }
                }
                TAG_FUNCTION => {
                    let f = &*(self.block(off) as *const JFunction);
                    self.mark_value(f.name, work);
                    self.mark_value(f.this_val, work);
                    self.mark_value(f.proto_obj, work);
                    for b in f.bound.iter() {
                        self.mark_value(*b, work);
                    }
                    for &c in f.upvalues.iter() {
                        self.mark_cell(c, work);
                    }
                }
                TAG_CELL => {
                    let c = &*(self.block(off) as *const JCell);
                    self.mark_value(c.v, work);
                }
                _ => {}
            }
        }
    }

    fn sweep(&mut self) {
        let mut off = 0usize;
        let end = self.bump;
        let mut freed = 0usize;
        let mut alive = 0usize;
        while off < end {
            let h = unsafe { self.block(off) };
            let sz = unsafe { (*h).size as usize };
            if sz == 0 {
                break;
            }
            if unsafe { (*h).mark } == MARK_LIVE {
                alive += sz;
                unsafe { (*h).mark = MARK_DEAD };
            } else if unsafe { (*h).tag } != 0 {
                unsafe {
                    match (*h).tag {
                        TAG_STRING => std::ptr::drop_in_place(h as *mut JString),
                        TAG_OBJECT => std::ptr::drop_in_place(h as *mut JObject),
                        TAG_ARRAY => std::ptr::drop_in_place(h as *mut JArray),
                        TAG_FUNCTION => std::ptr::drop_in_place(h as *mut JFunction),
                        TAG_CELL => std::ptr::drop_in_place(h as *mut JCell),
                        _ => {}
                    }
                    (*h).tag = 0;
                    (*h).flags = FLAG_FREELIST;
                    (*h).mark = MARK_DEAD;
                }
                self.free[class_of(sz)].push(off as u32);
                freed += sz;
            }
            off += sz;
        }
        self.live_bytes = alive;
        self.total_freed += freed as u64;
    }

    pub fn maybe_collect(&mut self, roots: &[&[Value]]) {
        if self.live_bytes > self.heap_limit {
            self.collect(roots);
        }
    }
}

/// Header accessor so `Arena::alloc_obj` can stamp the block size after the
/// constructor has run.
trait HasHeader {
    fn hdr_mut(&mut self) -> &mut ObjHeader;
}
macro_rules! impl_has_header {
    ($($t:ty),*) => {$(
        impl HasHeader for $t {
            #[inline]
            fn hdr_mut(&mut self) -> &mut ObjHeader { &mut self.hdr }
        }
    )*};
}
impl_has_header!(JObject, JArray, JFunction, JCell, JString);

fn transition_key(parent: u32, key: PropKey) -> u128 {
    ((parent as u128) << 64) | key.shape_rank() as u128
}

pub fn hash_bytes(b: &[u8]) -> u32 {
    // FNV-1a: dependency-free, only used for string hash sanity checks.
    let mut h: u32 = 0x811c_9dc5;
    for &c in b {
        h ^= c as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

// ---------------- OS memory ----------------

/// Reserve the whole offset space up front as writable, swap-less anonymous
/// memory.
///
/// The obvious alternative — `PROT_NONE` plus `mprotect` per extent — is not
/// portable: on macOS and several BSDs, `mprotect` refuses to add write
/// permission to an anonymous `PROT_NONE` mapping. `MAP_NORESERVE` gives the
/// same "pages exist only when touched" behaviour everywhere and needs no
/// second syscall on the allocation path.
fn reserve(bytes: usize) -> Option<*mut u8> {
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON | NORESERVE,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            None
        } else {
            Some(p as *mut u8)
        }
    }
}

/// `MAP_NORESERVE` where it exists, otherwise nothing (the mapping is still
/// lazily populated, only the swap accounting differs).
#[cfg(target_os = "linux")]
const NORESERVE: i32 = libc::MAP_NORESERVE;
#[cfg(not(target_os = "linux"))]
const NORESERVE: i32 = 0;

#[allow(dead_code)]
fn commit(base: *mut u8, from: usize, to: usize) {
    let _ = (base, from, to);
}

/// Release a reservation back to the OS.
fn unmap(base: *mut u8, len: usize) {
    if base.is_null() || len == 0 {
        return;
    }
    unsafe {
        libc::munmap(base.cast(), len);
    }
}
