//! NaN-boxed 64-bit values.
//!
//! ## Layout
//!
//! ```text
//!  63            48 47     44 43                                  0
//! +----------------+---------+-------------------------------------+
//! |     0x7FF8     | subtag  | payload / pointer-offset / smi       |
//! +----------------+---------+-------------------------------------+
//! ```
//!
//! A `u64` whose top 16 bits are exactly `0x7FF8` is a **boxed** value; any
//! other `u64` is a native `f64` bit pattern. `0x7FF8...` is a quiet-NaN, so
//! every finite double and both infinities keep their raw bits and the "is this
//! a double?" test is a single 16-bit compare. NaNs produced by arithmetic are
//! canonicalised to `NAN_VALUE`, the one NaN that is *not* used as a box.
//!
//! The 44-bit offset space is served by a single flat arena that reserves 16 TiB
//! of address space up front and commits pages on demand, so a heap pointer
//! always fits and never needs a handle dereference. `subtag` splits the
//! payload space between pointers, Smis (±8.7e12) and singletons.
//!
//! This is deliberately different from V8 (tagged pointers with a
//! pointer-compression heap) and from QuickJS (union + refcounted tagged
//! pointers): one word, one branch to dispatch, no separate Smi range stolen
//! from the double space.

use std::cmp::Ordering;

use crate::arena;
use crate::objects::{JArray, JCell, JFunction, JObject, JString, ObjHeader, TAG_ARRAY, TAG_CELL, TAG_FUNCTION, TAG_OBJECT, TAG_STRING};
use crate::errors::{type_error, VmError, VmResult};

/// The quiet-NaN prefix shared by every boxed value.
pub const BOX_TAG: u64 = 0x7FF8_0000_0000_0000;
const SUB_SHIFT: u32 = 44;
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const SMI_MASK: u64 = 0x0000_0FFF_FFFF_FFFF;
pub const OFFSET_MAX: u64 = 0x0000_0FFF_FFFF_FFFF;

const SUB_PTR: u64 = 0;
const SUB_SMI: u64 = 1;
const SUB_SINGLETON: u64 = 2;

const SING_UNDEFINED: u64 = 0;
const SING_NULL: u64 = 1;
const SING_TRUE: u64 = 2;
const SING_FALSE: u64 = 3;
const SING_EMPTY: u64 = 4;

const HEADER_OFFSET: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value(pub u64);

impl Value {
    pub const UNDEFINED: Value = Value(BOX_TAG | (SUB_SINGLETON << SUB_SHIFT) | SING_UNDEFINED);
    pub const NULL: Value = Value(BOX_TAG | (SUB_SINGLETON << SUB_SHIFT) | SING_NULL);
    pub const TRUE: Value = Value(BOX_TAG | (SUB_SINGLETON << SUB_SHIFT) | SING_TRUE);
    pub const FALSE: Value = Value(BOX_TAG | (SUB_SINGLETON << SUB_SHIFT) | SING_FALSE);
    /// Not a JS value: an absent array slot. Never escapes to user code.
    pub const EMPTY: Value = Value(BOX_TAG | (SUB_SINGLETON << SUB_SHIFT) | SING_EMPTY);
    /// The single canonical NaN double.
    pub const NAN: Value = Value(0x7FF0_0000_0000_0001);

    #[inline]
    pub const fn from_bits(b: u64) -> Value {
        Value(b)
    }

    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    // ---------- construction ----------

    #[inline]
    pub fn from_f64(f: f64) -> Value {
        if f.is_nan() {
            Value::NAN
        } else if !(f == 0.0 && f.is_sign_negative()) && f.fract() == 0.0 && f >= -(1i64 << 43) as f64 && f < (1i64 << 43) as f64 {
            // Integral values inside the Smi range are stored boxed, so every
            // integer has one representation. Before this, `let i = 0` produced
            // a double-repr zero (raw bits), which made the JIT's Smi entry
            // guard fail on the very first test, and meant the same number
            // could carry two different bit patterns depending on provenance.
            Value::from_i64(f as i64)
        } else {
            Value(f.to_bits())
        }
    }

    #[inline]
    pub fn from_i32(i: i32) -> Value {
        Value::from_i64(i as i64)
    }

    #[inline]
    pub fn from_i64(i: i64) -> Value {
        debug_assert!(i >= -(1i64 << 43) && i < (1i64 << 43), "int out of Smi range");
        Value(BOX_TAG | (SUB_SMI << SUB_SHIFT) | ((i as u64) & SMI_MASK))
    }

    #[inline]
    pub fn from_usize(i: usize) -> Value {
        Value::from_i64(i as i64)
    }

    /// Allocate a fresh arena string.
    pub fn from_str_owned(s: impl Into<String>) -> Value {
        let s: String = s.into();
        match arena::has_arena() {
            true => Value::from_ptr(arena::active().alloc_string(&s)),
            false => Value::UNDEFINED,
        }
    }

    /// Interned (identity-stable) string.
    pub fn str_atom(s: &str) -> Value {
        arena::active().intern_atom(s)
    }

    pub fn str_static(s: &'static str) -> Value {
        Value::from_str_owned(s)
    }

    #[inline]
    pub fn from_bool(b: bool) -> Value {
        if b {
            Value::TRUE
        } else {
            Value::FALSE
        }
    }

    #[inline]
    pub fn from_ptr<T>(p: *mut T) -> Value {
        let base = crate::arena::arena_base() as u64;
        let off = (p as u64).wrapping_sub(base);
        debug_assert!(off < (1u64 << 44), "heap object outside the 44-bit arena offset space");
        Value(BOX_TAG | (SUB_PTR << SUB_SHIFT) | (off & OFFSET_MAX))
    }

    // ---------- classification ----------

    #[inline]
    pub fn is_boxed(&self) -> bool {
        self.0 >> 48 == 0x7FF8
    }

    #[inline]
    pub fn is_double(&self) -> bool {
        !self.is_boxed()
    }

    #[inline]
    pub fn is_smi(&self) -> bool {
        self.is_boxed() && (self.0 >> SUB_SHIFT) & 0xF == SUB_SMI
    }

    #[inline]
    pub fn is_ptr(&self) -> bool {
        self.is_boxed() && (self.0 >> SUB_SHIFT) & 0xF == SUB_PTR
    }

    #[inline]
    pub fn is_undefined(&self) -> bool {
        *self == Value::UNDEFINED
    }
    #[inline]
    pub fn is_null(&self) -> bool {
        *self == Value::NULL
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        *self == Value::EMPTY
    }
    #[inline]
    pub fn is_nil(&self) -> bool {
        self.is_undefined() || self.is_null()
    }

    #[inline]
    pub fn is_bool(&self) -> bool {
        *self == Value::TRUE || *self == Value::FALSE
    }

    #[inline]
    pub fn as_bool(&self) -> bool {
        *self == Value::TRUE
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        self.is_smi() || self.is_double()
    }

    #[inline]
    pub fn is_nan(&self) -> bool {
        *self == Value::NAN
    }

    #[inline]
    pub fn is_string(&self) -> bool {
        unsafe { self.obj_tag() == Some(TAG_STRING) }
    }

    #[inline]
    pub fn is_object(&self) -> bool {
        unsafe { self.obj_tag() == Some(TAG_OBJECT) }
    }

    #[inline]
    pub fn is_array(&self) -> bool {
        unsafe { self.obj_tag() == Some(TAG_ARRAY) }
    }

    #[inline]
    pub fn is_function(&self) -> bool {
        unsafe { self.obj_tag() == Some(TAG_FUNCTION) }
    }

    #[inline]
    pub fn is_cell(&self) -> bool {
        unsafe { self.obj_tag() == Some(TAG_CELL) }
    }

    /// Cheap "could be a heap object with identity" test.
    #[inline]
    pub fn is_ref(&self) -> bool {
        self.is_ptr()
    }

    #[inline]
    pub unsafe fn obj_tag(&self) -> Option<u32> {
        if !self.is_ptr() {
            return None;
        }
        let base = crate::arena::arena_base();
        if base.is_null() {
            return None;
        }
        let off = (self.0 & OFFSET_MAX) as usize;
        let hdr = base.add(off) as *const ObjHeader;
        Some((*hdr).tag)
    }

    // ---------- accessors ----------

    #[inline]
    pub fn as_f64(&self) -> f64 {
        if self.is_smi() {
            self.as_i64() as f64
        } else if self.is_double() {
            f64::from_bits(self.0)
        } else {
            f64::NAN
        }
    }

    #[inline]
    pub fn as_i64(&self) -> i64 {
        if self.is_smi() {
            (((self.0 as i64) << 20) >> 20) as i64
        } else if self.is_double() {
            f64::from_bits(self.0) as i64
        } else {
            0
        }
    }

    #[inline]
    pub fn as_u32(&self) -> u32 {
        let f = self.as_f64();
        if f < 0.0 {
            0
        } else if f > 4294967295.0 {
            4294967295
        } else {
            f as u32
        }
    }

    #[inline]
    pub fn as_usize(&self) -> usize {
        let f = self.as_f64();
        if f < 0.0 {
            0
        } else {
            f as usize
        }
    }

    #[inline]
    pub unsafe fn as_string(&self) -> &'static JString {
        &*((crate::arena::arena_base() as *mut JString).byte_add((self.0 & OFFSET_MAX) as usize)
            as *const JString)
    }

    #[inline]
    pub unsafe fn as_object(&self) -> &'static JObject {
        &*((crate::arena::arena_base() as *mut JObject).byte_add((self.0 & OFFSET_MAX) as usize)
            as *const JObject)
    }

    #[inline]
    pub unsafe fn as_object_mut(&self) -> &'static mut JObject {
        &mut *((crate::arena::arena_base() as *mut JObject).byte_add((self.0 & OFFSET_MAX) as usize)
            as *mut JObject)
    }

    #[inline]
    pub unsafe fn as_array(&self) -> &'static JArray {
        &*((crate::arena::arena_base() as *mut JArray).byte_add((self.0 & OFFSET_MAX) as usize)
            as *const JArray)
    }

    #[inline]
    pub unsafe fn as_array_mut(&self) -> &'static mut JArray {
        &mut *((crate::arena::arena_base() as *mut JArray).byte_add((self.0 & OFFSET_MAX) as usize)
            as *mut JArray)
    }

    #[inline]
    pub unsafe fn as_function(&self) -> &'static JFunction {
        &*((crate::arena::arena_base() as *mut JFunction).byte_add((self.0 & OFFSET_MAX) as usize)
            as *const JFunction)
    }

    #[inline]
    pub unsafe fn as_function_mut(&self) -> &'static mut JFunction {
        &mut *((crate::arena::arena_base() as *mut JFunction).byte_add((self.0 & OFFSET_MAX) as usize)
            as *mut JFunction)
    }

    #[inline]
    pub unsafe fn as_cell_mut(&self) -> &'static mut JCell {
        &mut *((crate::arena::arena_base() as *mut JCell).byte_add((self.0 & OFFSET_MAX) as usize)
            as *mut JCell)
    }

    pub fn as_str(&self) -> Option<&'static str> {
        if self.is_string() {
            Some(unsafe { self.as_string().as_str() })
        } else {
            None
        }
    }

    /// Best-effort identifier text: strings and numbers (as produced by
    /// `String(n)`) both work when the caller only needs a stable key.
    pub fn as_key_str(&self) -> Option<&'static str> {
        if self.is_string() {
            Some(unsafe { self.as_string().as_str() })
        } else {
            None
        }
    }

    // ---------- truthiness ----------

    #[inline]
    pub fn to_bool(&self) -> bool {
        if self.is_smi() {
            self.as_i64() != 0
        } else if self.is_double() {
            let f = f64::from_bits(self.0);
            f != 0.0 && !f.is_nan()
        } else if self.is_bool() {
            self.as_bool()
        } else if self.is_string() {
            // The empty string is falsy in JS.
            unsafe { !self.as_string().as_str().is_empty() }
        } else {
            !self.is_nil()
        }
    }

    /// `typeof`, fixed string results.
    pub fn type_of(&self) -> &'static str {
        if self.is_smi() || self.is_double() {
            "number"
        } else if self.is_string() {
            "string"
        } else if self.is_ptr() {
            let tag = unsafe { self.obj_tag() };
            match tag {
                Some(TAG_FUNCTION) => "function",
                _ => {
                    if self.is_null() || self.is_undefined() {
                        "undefined"
                    } else {
                        "object"
                    }
                }
            }
        } else if self.is_undefined() {
            "undefined"
        } else if self.is_bool() {
            "boolean"
        } else {
            "object"
        }
    }

    // ---------- comparisons ----------

    /// `===`. Pointer identity for references.
    pub fn strict_eq(&self, other: &Value) -> bool {
        if self.is_number() && other.is_number() { return self.as_f64() == other.as_f64(); }
        if self.is_string() && other.is_string() {
            return unsafe { self.as_string() }.code_units().eq(unsafe { other.as_string() }.code_units());
        }
        self.0 == other.0
    }

    /// `Object.is`-style: signed zeros differ; NaNs compare equal.
    pub fn same_value(&self, other: &Value) -> bool {
        if self.is_number() && other.is_number() {
            return (self.is_nan() && other.is_nan()) || self.as_f64().to_bits() == other.as_f64().to_bits();
        }
        self.strict_eq(other)
    }

    /// Numeric ordering, returning `None` when either side is `NaN`.
    pub fn num_cmp(&self, other: &Value) -> Option<Ordering> {
        if self.is_smi() && other.is_smi() {
            return Some(self.as_i64().cmp(&other.as_i64()));
        }
        let a = self.as_f64();
        let b = other.as_f64();
        a.partial_cmp(&b)
    }

    // ---------- arithmetic fast paths ----------

    /// Numeric `+` when both sides are numbers, `None` when the slow path
    /// (string concatenation / ToPrimitive) is required.
    #[inline]
    pub fn try_num_add(&self, other: &Value) -> Option<Value> {
        if self.is_smi() && other.is_smi() {
            let a = self.as_i64();
            let b = other.as_i64();
            let r = a.wrapping_add(b);
            if r >= -(1i64 << 43) && r < (1i64 << 43) {
                return Some(Value::from_i64(r));
            }
            return Some(Value::from_f64(a as f64 + b as f64));
        }
        if self.is_number() && other.is_number() {
            return Some(Value::from_f64(self.as_f64() + other.as_f64()));
        }
        None
    }

    #[inline]
    pub fn try_num_binop(&self, other: &Value, op: ArithOp) -> Option<Value> {
        if self.is_smi() && other.is_smi() {
            let a = self.as_i64();
            let b = other.as_i64();
            return match op {
                ArithOp::Sub => Some(Value::from_f64((a - b) as f64)),
                ArithOp::Div => Some(Value::from_f64(a as f64 / b as f64)),
                // Integer remainder: for two integers `%` and `fmod` agree on
                // sign and magnitude, and this avoids a libm `fmod` call per
                // iteration (`i % 1000` in a loop was the single hottest leaf
                // in the array/string/sort benchmarks). Zero still has to go
                // through the f64 path so it yields NaN, not a panic.
                ArithOp::Mod if b != 0 => Some(if a < 0 && a % b == 0 { Value::from_f64(-0.0) } else { Value::from_i64(a % b) }),
                ArithOp::Mul => {
                    if let Some(r) = a.checked_mul(b) {
                        if r >= -(1i64 << 43) && r < (1i64 << 43) {
                            return Some(if r == 0 && (a < 0 || b < 0) { Value::from_f64(-0.0) } else { Value::from_i64(r) });
                        }
                    }
                    Some(Value::from_f64((a as f64) * (b as f64)))
                }
                _ => None,
            };
        }
        if self.is_number() && other.is_number() {
            let a = self.as_f64();
            let b = other.as_f64();
            return Some(Value::from_f64(match op {
                ArithOp::Add => a + b,
                ArithOp::Sub => a - b,
                ArithOp::Mul => a * b,
                ArithOp::Div => a / b,
                ArithOp::Mod => js_mod(a, b),
                ArithOp::Pow => js_pow(a, b),
            }));
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

/// JS `%` keeps the sign of the dividend.
#[inline]
pub fn js_mod(a: f64, b: f64) -> f64 {
    let r = a % b;
    r
}

/// JS `**` follows IEEE `pow` (so `(-1) ** 0.5` is NaN, not a throw).
#[inline]
pub fn js_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}

/// Numeric ordering that JS `<` uses, with `NaN` handled as "not less than".
pub fn js_less(a: f64, b: f64) -> bool {
    a < b
}

pub fn throw_not_func() -> VmError {
    type_error("value is not a function")
}

pub fn ok<T>(v: T) -> VmResult<T> {
    Ok(v)
}

/// Formatting helper used by `String(x)` / `console.log`.
pub fn f64_to_js_string(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-Infinity".to_string() } else { "Infinity".to_string() };
    }
    if f == 0.0 {
        return "0".to_string();
    }
    if f.fract() == 0.0 && f.abs() < 1e21 {
        let i = f as i64;
        // Large integers must not go through i64.
        if (i as f64) == f {
            return i.to_string();
        }
    }
    let mut s = format!("{}", f);
    // Rust prints `3` for 3.0; JS prints `3`. Handled above for the integral
    // case; here only the exponent form can leak an `e` without a sign.
    if s.contains('e') && !s.contains("e-") && !s.contains("e+") {
        s = s.replacen('e', "e+", 1);
    }
    s
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_smi() {
            write!(f, "Smi({})", self.as_i64())
        } else if self.is_double() {
            write!(f, "Num({})", f64::from_bits(self.0))
        } else if self.is_string() {
            write!(f, "Str({:?})", self.as_str().unwrap_or("<bad>"))
        } else if self.is_function() {
            write!(f, "Fn@{:x}", self.0 & OFFSET_MAX)
        } else if self.is_array() {
            write!(f, "Array@{:x}", self.0 & OFFSET_MAX)
        } else if self.is_object() {
            write!(f, "Object@{:x}", self.0 & OFFSET_MAX)
        } else {
            write!(f, "Value({:#x})", self.0)
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_smi() {
            write!(f, "{}", self.as_i64())
        } else if self.is_double() {
            write!(f, "{}", f64_to_js_string(f64::from_bits(self.0)))
        } else if self.is_string() {
            write!(f, "{}", self.as_str().unwrap_or(""))
        } else if self.is_bool() {
            write!(f, "{}", self.as_bool())
        } else if self.is_undefined() {
            write!(f, "undefined")
        } else if self.is_null() {
            write!(f, "null")
        } else if self.is_function() {
            write!(f, "function")
        } else {
            write!(f, "[object Object]")
        }
    }
}
