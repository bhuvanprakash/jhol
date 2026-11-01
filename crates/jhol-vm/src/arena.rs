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

/// Deterministic non-cryptographic 64-bit hasher (FX-style) for the VM's
/// process-local maps. Profiling showed the default SipHash as the single
/// hottest frame — every interned-key property write hashed string bytes
/// through it. These tables are not attacker-facing; distribution just needs
/// to be good, not collision-proof, and `HashMap` stays correct either way.
#[derive(Clone, Default)]
pub struct FastH(u64);

pub type FMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FastH>>;

impl std::hash::Hasher for FastH {
    fn finish(&self) -> u64 {
        // Murmur3 fmix64 finalizer: without full avalanche, sequential keys
        // (`key0`…`keyN`, small integers) land in strided buckets and wide
        // maps degrade toward linear scans.
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
        h ^= h >> 33;
        h
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h =
            (self.0 ^ (bytes.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)).rotate_left(31);
        let mut i = 0;
        while i + 8 <= bytes.len() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[i..i + 8]);
            h = (h ^ u64::from_le_bytes(b).wrapping_mul(0xff51_afd7_ed55_8ccd)).rotate_left(31);
            i += 8;
        }
        if i < bytes.len() {
            let mut b = [0u8; 8];
            b[..bytes.len() - i].copy_from_slice(&bytes[i..]);
            h = (h ^ u64::from_le_bytes(b).wrapping_mul(0xc4ce_b9fe_1a85_ec53)).rotate_left(31);
        }
        self.0 = h;
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0 ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15)).rotate_left(31);
    }
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }
    fn write_u128(&mut self, n: u128) {
        self.write_u64(n as u64);
        self.write_u64((n >> 64) as u64);
    }
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}
#[cfg(feature = "stats")]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::objects::*;
use crate::value::{Value, OFFSET_MAX};

/// Largest size that goes into a size-class free list (512 B => 64 classes).
const CLASS_MAX: usize = 512;
const NCLASSES: usize = 65;

#[cfg(feature = "stats")]
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
    #[cfg(windows)]
    committed: usize,
    free: Vec<Vec<usize>>,
    symbols: FMap<Box<str>, u32>,
    symbol_names: Vec<Box<str>>,
    transitions: FMap<u128, u32>,
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
    accessor_map: FMap<u64, u32>,
    atoms: FMap<Box<str>, Value>,
    atom_roots: Vec<Value>,
    live_bytes: usize,
    peak_bytes: usize,
    collections: u64,
    property_epoch: u64,
    collection_remaining: usize,
    live_external_bytes: usize,
    external_pressure: usize,
    root_mode: RootMode,
    temporary_roots: Vec<Value>,
    host_roots: Vec<Value>,
    total_alloc: u64,
    total_freed: u64,
    heap_limit: usize,
    gc_disabled: bool,
}

impl Drop for Arena {
    fn drop(&mut self) {
        // Unmapping bytes alone leaks the Rust-owned vectors, maps and function
        // prototypes inside live objects. Finalize them before releasing pages.
        let mut off = 0;
        while off < self.bump {
            let header = unsafe { self.block(off) };
            let size = unsafe { (*header).size as usize };
            if size < std::mem::size_of::<ObjHeader>() || size > self.bump - off {
                break;
            }
            unsafe { drop_block(header) };
            off += size;
        }
        unmap(self.base, self.reserved);
    }
}

#[inline]
unsafe fn drop_block(header: *mut ObjHeader) {
    match (*header).tag {
        TAG_STRING => std::ptr::drop_in_place(header as *mut JString),
        TAG_OBJECT => std::ptr::drop_in_place(header as *mut JObject),
        TAG_ARRAY => std::ptr::drop_in_place(header as *mut JArray),
        TAG_FUNCTION => std::ptr::drop_in_place(header as *mut JFunction),
        TAG_CELL => std::ptr::drop_in_place(header as *mut JCell),
        _ => {}
    }
}

/// Owned container capacity, excluding bytes already inside the arena block.
/// Hash tables use a conservative bucket/control-byte allowance.
#[inline]
unsafe fn external_bytes(header: *const ObjHeader) -> usize {
    use std::mem::size_of;
    match (*header).tag {
        TAG_OBJECT => {
            let o = &*header.cast::<JObject>();
            o.spill.as_ref().map_or(0, |v| {
                size_of::<Vec<(PropKey, Value)>>() + v.capacity() * size_of::<(PropKey, Value)>()
            }) + o.index.as_ref().map_or(0, |m| {
                size_of::<FMap<PropKey, u32>>() + m.capacity() * 2 * size_of::<(PropKey, u32)>()
            })
        }
        TAG_ARRAY => {
            let a = &*header.cast::<JArray>();
            a.data.capacity() * size_of::<Value>()
                + a.sparse
                    .as_ref()
                    .map_or(0, |m| m.capacity() * 2 * size_of::<(u32, Value)>())
                + a.props.as_ref().map_or(0, |p| {
                    size_of::<Vec<(PropKey, Value)>>()
                        + p.capacity() * size_of::<(PropKey, Value)>()
                })
        }
        TAG_FUNCTION => {
            let f = &*header.cast::<JFunction>();
            f.keys.capacity() * size_of::<PropKey>()
                + f.vals.capacity() * size_of::<Value>()
                + f.bound.capacity() * size_of::<Value>()
                + f.upvalues.capacity() * size_of::<*mut JCell>()
        }
        _ => 0,
    }
}

#[inline]
pub(crate) fn note_external_allocation(bytes: usize) {
    if bytes == 0 {
        return;
    }
    let arena = active_ptr();
    if !arena.is_null() {
        unsafe { &mut *arena }.note_external_allocation(bytes);
    }
}

unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

/// Stack-owned roots for Rust code that can re-enter JavaScript. No reference
/// into a VM vector is held across re-entry; the installed arena stays fixed.
#[derive(Clone, Copy)]
enum RootMode {
    None,
    Host,
    Temporary,
}

pub(crate) struct RootScope {
    arena: *mut Arena,
    start: usize,
    mode: RootMode,
}

impl Drop for RootScope {
    #[inline]
    fn drop(&mut self) {
        let arena = unsafe { &mut *self.arena };
        arena.temporary_roots.truncate(self.start);
        arena.root_mode = self.mode;
    }
}

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
            #[cfg(windows)]
            committed: 0,
            free: (0..=NCLASSES).map(|_| Vec::new()).collect(),
            symbols: HashMap::default(),
            symbol_names: Vec::new(),
            transitions: HashMap::default(),
            trans_ic: [(0, 0); 32],
            trans_ic_used: 0,
            acc_ic: [(0, 0); 32],
            acc_ic_used: 0,
            next_shape: 1,
            accessor_map: HashMap::default(),
            atoms: HashMap::default(),
            atom_roots: Vec::new(),
            live_bytes: 0,
            peak_bytes: 0,
            collections: 0,
            property_epoch: 0,
            collection_remaining: if std::env::var("JHOL_VM_NO_GC").is_ok() {
                usize::MAX
            } else {
                heap_limit
            },
            live_external_bytes: 0,
            external_pressure: 0,
            root_mode: RootMode::None,
            temporary_roots: Vec::new(),
            host_roots: Vec::new(),
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

    #[inline]
    pub(crate) fn property_epoch(&self) -> u64 {
        self.property_epoch
    }

    pub(crate) fn invalidate_property_caches(&mut self) {
        self.property_epoch = self.property_epoch.wrapping_add(1);
    }

    pub(crate) fn attach_vm(&mut self) {
        self.root_mode = RootMode::Host;
    }

    #[inline]
    pub(crate) fn collection_due(&self) -> bool {
        self.collection_remaining == 0
    }

    #[inline]
    pub(crate) fn note_external_allocation(&mut self, bytes: usize) {
        if bytes != 0 {
            self.external_pressure = self.external_pressure.saturating_add(bytes);
            self.collection_remaining = self.collection_remaining.saturating_sub(bytes);
        }
    }

    pub fn collection_count(&self) -> u64 {
        self.collections
    }

    #[cfg(test)]
    pub(crate) fn set_collection_budget(&mut self, bytes: usize) {
        self.heap_limit = bytes;
        self.collection_remaining = if self.gc_disabled { usize::MAX } else { bytes };
    }

    #[inline]
    pub(crate) fn retain_value(&mut self, value: Value) {
        if !matches!(self.root_mode, RootMode::None) && value.is_ptr() {
            self.retain_value_slow(value);
        }
    }

    #[cold]
    fn retain_value_slow(&mut self, value: Value) {
        match self.root_mode {
            RootMode::Temporary => self.temporary_roots.push(value),
            RootMode::Host => self.host_roots.push(value),
            RootMode::None => {}
        }
    }

    #[inline]
    pub(crate) fn pin_temporaries(&mut self, values: &[Value]) {
        if matches!(self.root_mode, RootMode::Temporary) {
            self.temporary_roots
                .extend(values.iter().copied().filter(Value::is_ptr));
        }
    }

    #[inline]
    pub(crate) fn root_scope(&mut self, values: &[Value]) -> RootScope {
        let scope = RootScope {
            arena: self,
            start: self.temporary_roots.len(),
            mode: self.root_mode,
        };
        self.root_mode = RootMode::Temporary;
        self.pin_temporaries(values);
        scope
    }

    #[inline]
    pub(crate) fn guest_scope(&mut self) -> RootScope {
        let scope = RootScope {
            arena: self,
            start: self.temporary_roots.len(),
            mode: self.root_mode,
        };
        self.root_mode = RootMode::None;
        scope
    }

    pub fn stats(&self) -> String {
        format!(
            "heap: {}B live / {}B peak / {}B reserved, bump={}B, collections={}, alloc={}B, freed={}B, external_live={}B, external_growth={}B, symbols={}, shapes={}",
            self.live_bytes,
            self.peak_bytes,
            self.reserved,
            self.bump,
            self.collections,
            self.total_alloc,
            self.total_freed,
            self.live_external_bytes,
            self.external_pressure,
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
                drop(Box::from_raw(p));
            }
        });
    }

    pub fn install(self) -> &'static mut Arena {
        ARENA.with(|c| {
            let old = c.get();
            if !old.is_null() {
                unsafe {
                    drop(Box::from_raw(old));
                }
            }
        });
        let ptr = Box::into_raw(Box::new(self));
        ARENA.with(|c| c.set(ptr));
        unsafe { &mut *ptr }
    }

    // ---------------- allocation ----------------

    #[inline]
    fn take_block(&mut self, need: usize) -> Option<usize> {
        let cls = class_of(need);
        if cls == NCLASSES {
            let mut best: Option<(usize, usize)> = None;
            for (i, &off) in self.free[NCLASSES].iter().enumerate() {
                let sz = unsafe { (*self.block(off)).size as usize };
                // A remainder must hold a complete header for the linear sweep.
                if (sz == need || sz >= need + std::mem::size_of::<ObjHeader>())
                    && (best.is_none() || sz < best.unwrap().1)
                {
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
    #[inline]
    pub fn alloc_raw(&mut self, need: usize) -> usize {
        let need = need
            .max(std::mem::size_of::<ObjHeader>())
            .checked_add(7)
            .map(|size| size & !7)
            .filter(|&size| size <= u32::MAX as usize)
            .unwrap_or_else(|| self.oom());
        if let Some(off) = self.take_block(need) {
            let actual = unsafe { (*self.block(off)).size as usize };
            if actual > need {
                let rem = actual - need;
                unsafe {
                    std::ptr::write(
                        self.block(off + need),
                        ObjHeader::new(0, rem as u32, FLAG_FREELIST),
                    );
                }
                self.free[class_of(rem)].push(off + need);
            }
            self.account(need);
            return off;
        }
        let off = self.bump;
        let end = off.checked_add(need).unwrap_or_else(|| self.oom());
        if end > self.reserved || end - 1 > OFFSET_MAX as usize {
            self.oom();
        }
        #[cfg(windows)]
        if end > self.committed {
            // Commit in 64 KiB chunks, bounded by the reservation.
            let to = (end.saturating_add(65535) & !65535).min(self.reserved);
            if !commit(self.base, self.committed, to) {
                self.oom();
            }
            self.committed = to;
        }
        self.bump = end;
        self.account(need);
        off
    }

    #[inline]
    fn account(&mut self, need: usize) {
        self.live_bytes += need;
        // Charge allocations once; hot loop/call polls become one comparison.
        self.collection_remaining = self.collection_remaining.saturating_sub(need);
        self.total_alloc += need as u64;
        if self.live_bytes > self.peak_bytes {
            self.peak_bytes = self.live_bytes;
        }
        #[cfg(feature = "stats")]
        ALLOCS.fetch_add(1, Ordering::Relaxed);
    }

    fn oom(&self) -> ! {
        panic!(
            "JholVM: arena exhausted ({}B live, {}B reserved)",
            self.live_bytes, self.reserved
        );
    }

    /// Allocate a `#[repr(C)]` object and return the raw pointer.
    #[inline]
    pub fn alloc_obj<T: HasHeader>(&mut self, make: impl FnOnce() -> T) -> *mut T {
        let need = std::mem::size_of::<T>().div_ceil(8) * 8;
        let value = make();
        let off = self.alloc_raw(need);
        unsafe {
            let p = self.block(off) as *mut T;
            std::ptr::write(p, value);
            (*p).hdr_mut().size = need as u32;
            self.note_external_allocation(external_bytes(p.cast()));
            self.retain_value(Value::from_ptr_in(p, self.base));
            p
        }
    }

    /// Strings keep their bytes inline after the (8-aligned) header.
    pub fn alloc_string(&mut self, s: &str) -> *mut JString {
        if s.is_ascii() {
            self.alloc_string_data(s, None)
        } else {
            self.alloc_string_data(s, Some(&s.encode_utf16().collect::<Vec<_>>()))
        }
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
            let p = self.block(off) as *mut JString;
            std::ptr::write(
                p,
                JString {
                    hdr: ObjHeader::new(TAG_STRING, need as u32, 0),
                    len: s.len() as u32,
                    hash: hash_bytes(s.as_bytes()),
                    is_atom: 0,
                    ascii: units.is_none() as u8,
                    pad: [0; 2],
                    utf16_len: units.map_or(s.len(), |u| u.len()) as u32,
                },
            );
            let data = (p as *mut u8).add(32);
            std::ptr::copy_nonoverlapping(s.as_ptr(), data, s.len());
            if let Some(u) = units {
                std::ptr::copy_nonoverlapping(
                    u.as_ptr(),
                    data.add(units_off).cast::<u16>(),
                    u.len(),
                );
            }
            self.retain_value(Value::from_ptr_in(p, self.base));
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
        self.alloc_obj(|| JCell {
            hdr: ObjHeader::new(TAG_CELL, 0, 0),
            v,
        })
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
        if let Ok(s) = String::from_utf16(units) {
            return self.intern_atom(&s);
        }
        let p = self.alloc_string_utf16(units);
        unsafe {
            (*p).is_atom = 1;
        }
        let v = Value::from_ptr(p);
        self.atom_roots.push(v);
        v
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
        let k = (base as u64)
            | ((prefix.len() as u64) << 32)
            | ((hash_bytes(prefix.as_bytes()) as u64) << 40);
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
        self.collect_with(roots, |_, _, _| 0);
    }

    pub(crate) fn collect_with(
        &mut self,
        roots: &[&[Value]],
        mut external: impl FnMut(&Arena, usize, &mut Vec<*mut ObjHeader>) -> usize,
    ) {
        if self.gc_disabled {
            return;
        }
        self.collections += 1;
        self.live_external_bytes = 0;
        // Freed prototype addresses may be reused with a different chain.
        self.invalidate_property_caches();
        let mut work: Vec<*mut ObjHeader> = Vec::new();
        for r in roots {
            for v in *r {
                self.mark_value(*v, &mut work);
            }
        }
        for &value in self.temporary_roots.iter().chain(self.host_roots.iter()) {
            self.mark_value(value, &mut work);
        }
        for v in self.atom_roots.clone() {
            self.mark_value(v, &mut work);
        }
        while let Some(h) = work.pop() {
            if unsafe { (*h).mark } == MARK_QUEUED {
                unsafe { (*h).mark = MARK_LIVE };
                let off = (h as usize) - (self.base as usize);
                self.trace(off, &mut work);
                if unsafe { (*h).flags & FLAG_GENERATOR != 0 } {
                    self.live_external_bytes = self
                        .live_external_bytes
                        .saturating_add(external(self, off, &mut work));
                }
            }
        }
        self.sweep();
        self.external_pressure = 0;
        let live = self.live_bytes.saturating_add(self.live_external_bytes);
        self.collection_remaining = self.heap_limit.max(live / 2).max(64 * 1024);
    }

    pub(crate) fn mark_value(&self, v: Value, work: &mut Vec<*mut ObjHeader>) {
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
                    if let Some(p) = &a.props {
                        for (_, val) in p.iter() {
                            self.mark_value(*val, work);
                        }
                    }
                }
                TAG_FUNCTION => {
                    let f = &*(self.block(off) as *const JFunction);
                    self.mark_value(f.name, work);
                    self.mark_value(f.this_val, work);
                    self.mark_value(f.bound_this, work);
                    self.mark_value(f.proto, work);
                    self.mark_value(f.proto_obj, work);
                    for &value in &f.vals {
                        self.mark_value(value, work);
                    }
                    if let FnKind::Bytecode(proto) = &f.kind {
                        for constant in &proto.consts {
                            if let crate::bytecode::ConstVal::Str(value) = constant {
                                self.mark_value(*value, work);
                            }
                        }
                    }
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
                self.live_external_bytes = self
                    .live_external_bytes
                    .saturating_add(unsafe { external_bytes(h) });
                unsafe { (*h).mark = MARK_DEAD };
            } else if unsafe { (*h).tag } != 0 {
                unsafe {
                    drop_block(h);
                    (*h).tag = 0;
                    (*h).flags = FLAG_FREELIST;
                    (*h).mark = MARK_DEAD;
                }
                self.free[class_of(sz)].push(off);
                freed += sz;
            }
            off += sz;
        }
        self.live_bytes = alive;
        self.total_freed += freed as u64;
    }

    pub fn maybe_collect(&mut self, roots: &[&[Value]]) {
        if self.collection_due() {
            self.collect(roots);
        }
    }
}

/// Header accessor so `Arena::alloc_obj` can stamp the block size after the
/// constructor has run.
pub trait HasHeader {
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
#[cfg(unix)]
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
#[cfg(all(unix, not(target_os = "linux")))]
const NORESERVE: i32 = 0;

#[allow(dead_code)]
#[cfg(unix)]
fn commit(base: *mut u8, from: usize, to: usize) -> bool {
    let _ = (base, from, to);
    true
}

/// Release a reservation back to the OS.
#[cfg(unix)]
fn unmap(base: *mut u8, len: usize) {
    if base.is_null() || len == 0 {
        return;
    }
    unsafe {
        libc::munmap(base.cast(), len);
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[test]
    fn offsets_above_four_gib_do_not_wrap_or_alias() {
        let mut arena = Arena::new();
        let high = 1usize << 32;
        assert!(
            arena.reserved > high + 65536,
            "test needs a sparse reservation above 4 GiB"
        );
        arena.bump = high;
        #[cfg(windows)]
        {
            assert!(commit(arena.base, high, high + 65536));
            arena.committed = high + 65536;
        }
        let size = std::mem::size_of::<JCell>();
        let offset = arena.alloc_raw(size);
        assert_eq!(
            offset, high,
            "arena offsets must retain all 44 payload bits"
        );
        unsafe {
            arena
                .block(offset)
                .write(ObjHeader::new(TAG_CELL, size as u32, 0));
        }
        arena.free[class_of(size)].push(offset);
        assert_eq!(
            arena.alloc_raw(size),
            high,
            "recycled offsets must not truncate either"
        );
        // This sparse-offset probe deliberately left the preceding 4 GiB
        // untouched; it is not a valid linear heap for finalization.
        arena.bump = 0;
    }

    #[test]
    fn oversized_blocks_fail_before_mutating_the_arena() {
        let mut arena = Arena::new();
        for need in [usize::MAX, u32::MAX as usize + 1] {
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || arena.alloc_raw(need)
            ))
            .is_err());
            assert_eq!(arena.bump, 0);
            assert_eq!(arena.live_bytes, 0);
        }
    }

    #[test]
    fn recycled_tail_always_fits_a_complete_header() {
        let mut arena = Arena::new();
        let size = 1024;
        let offset = arena.alloc_raw(size);
        unsafe {
            arena
                .block(offset)
                .write(ObjHeader::new(TAG_CELL, size as u32, 0));
        }
        arena.free[class_of(size)].push(offset);
        assert_ne!(
            arena.alloc_raw(size - 8),
            offset,
            "an eight-byte tail cannot hold its header"
        );
        assert_eq!(
            arena.alloc_raw(size),
            offset,
            "the unsplit block stays reusable"
        );
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn VirtualAlloc(
        address: *mut std::ffi::c_void,
        bytes: usize,
        allocation_type: u32,
        protection: u32,
    ) -> *mut std::ffi::c_void;
    fn VirtualFree(address: *mut std::ffi::c_void, bytes: usize, free_type: u32) -> i32;
}

#[cfg(windows)]
fn reserve(bytes: usize) -> Option<*mut u8> {
    // MEM_RESERVE reserves address space without charging for untouched pages.
    let p = unsafe { VirtualAlloc(std::ptr::null_mut(), bytes, 0x2000, 0x04) };
    if p.is_null() {
        None
    } else {
        Some(p.cast())
    }
}

#[cfg(windows)]
fn commit(base: *mut u8, from: usize, to: usize) -> bool {
    if from >= to {
        return true;
    }
    // MEM_COMMIT + PAGE_READWRITE; the OS rounds this range to page boundaries.
    !unsafe { VirtualAlloc(base.add(from).cast(), to - from, 0x1000, 0x04) }.is_null()
}

#[cfg(windows)]
fn unmap(base: *mut u8, len: usize) {
    if !base.is_null() && len != 0 {
        // MEM_RELEASE requires the original reservation address and size zero.
        unsafe {
            VirtualFree(base.cast(), 0, 0x8000);
        }
    }
}
