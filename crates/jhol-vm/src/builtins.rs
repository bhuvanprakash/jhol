//! JS-visible builtins: the global object graph and everything on it.
//!
//! Native functions are plain `fn` pointers (no captures), which is what lets a
//! builtin live in an arena cell that outlives any Rust frame. Where the engine
//! needs a runtime helper that JS never names directly (`__jhol_apply`,
//! `__jhol_enum_keys`, …) it is installed under a prefixed global.

use crate::arena::{self};
use crate::errors::{syntax_error, type_error, VmResult};
use crate::objects::*;
use crate::value::{f64_to_js_string, ArithOp, Value};
use crate::vm::{push_timer, TimerEntry, TimerKind, Vm};

macro_rules! native {
    ($name:ident, |$vm:ident, $this:ident, $args:ident| $body:expr) => {
        pub fn $name($vm: &mut Vm, $this: Value, $args: &[Value]) -> VmResult<Value> {
            let _ = &$vm;
            $body
        }
    };
}

thread_local! {
    /// Shared `RegExp.prototype` for regex objects created by the literal path
    /// and by the constructor.
    static REGEX_PROTO: std::cell::Cell<Option<Value>> = const { std::cell::Cell::new(None) };
    /// Compiled patterns, keyed by (source, flags). Recompiling on every call
    /// would make `replace(/x/g, …)` in a loop quadratic.
    static REGEX_CACHE: std::cell::RefCell<std::collections::HashMap<(String, String), std::rc::Rc<crate::regex::Regex>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

thread_local! {
    /// Prototype per error name, filled while the globals are built. Runtime
    /// errors are raised from deep inside the VM with no access to the globals,
    /// so they need this handle to produce a real Error object.
    static ERROR_PROTOS: std::cell::RefCell<std::collections::HashMap<&'static str, Value>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Build an `Error`-like object with `name` and `message`.
///
/// Runtime faults used to be thrown as the bare string `"TypeError: …"`, so
/// `catch (e) { e.name }` was `undefined` and `e instanceof TypeError` was
/// `false`. Every `catch` converts them now, which is why this lives here: the
/// VM has no access to the globals at throw time.
pub fn make_error_value(vm: &mut Vm, name: &str, msg: &str) -> Value {
    let o = vm.new_object();
    let proto = ERROR_PROTOS.with(|m| m.borrow().get(name).copied());
    if let Some(p) = proto {
        let _ = vm.set_prototype(o, p);
    }
    set_obj_str(vm, o, "name", name);
    set_obj_str(vm, o, "message", msg);
    o
}

fn compiled_regex(source: &str, flags: &str) -> Result<std::rc::Rc<crate::regex::Regex>, String> {
    REGEX_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if let Some(r) = cache.get(&(source.to_string(), flags.to_string())) {
            return Ok(r.clone());
        }
        let re = std::rc::Rc::new(crate::regex::Regex::compile(source, flags)?);
        // Bound the cache; patterns come from user code.
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert((source.to_string(), flags.to_string()), re.clone());
        Ok(re)
    })
}

/// A regex object is a plain object carrying `source` and `flags` plus this
/// marker. Keeping it inside the existing value model means no new `Value`
/// variant and no GC root for a Rust struct.
fn as_regexp(vm: &mut Vm, v: Value) -> Option<(String, String)> {
    if !v.is_object() {
        return None;
    }
    let mk = key(vm, "__jhol_regex");
    let marker = vm.get_prop(v, mk).ok()?;
    if !marker.as_bool() {
        return None;
    }
    let ks = key(vm, "source");
    let kf = key(vm, "flags");
    let src = vm.get_prop(v, ks).ok()?;
    let flg = vm.get_prop(v, kf).ok()?;
    Some((to_str(vm, src).ok()?, to_str(vm, flg).ok()?))
}

fn make_regexp(vm: &mut Vm, source: &str, flags: &str) -> VmResult<Value> {
    // Validate eagerly: constructing an invalid regex must throw, like node.
    compiled_regex(source, flags).map_err(|e| {
        type_error(&format!(
            "Invalid regular expression: /{source}/{flags}: {e}"
        ))
    })?;
    let proto = REGEX_PROTO.with(|p| p.get()).unwrap_or(Value::UNDEFINED);
    let o = vm.new_object();
    vm.set_prototype(o, proto)?;
    set_obj_str(vm, o, "source", source);
    set_obj_str(vm, o, "flags", flags);
    let g = flags.contains('g');
    let kmarker = key(vm, "__jhol_regex");
    let klast = key(vm, "lastIndex");
    for (name, on) in [
        ("global", g),
        ("ignoreCase", flags.contains('i')),
        ("multiline", flags.contains('m')),
        ("sticky", flags.contains('y')),
        ("dotAll", flags.contains('s')),
        ("unicode", flags.contains('u')),
        ("hasIndices", false),
    ] {
        let kn = key(vm, name);
        vm.set_prop(o, kn, Value::from_bool(on))?;
    }
    vm.set_prop(o, kmarker, Value::from_bool(true))?;
    vm.set_prop(o, klast, Value::from_i32(0))?;
    Ok(o)
}

#[inline]
fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::UNDEFINED)
}

#[allow(dead_code)]
#[inline]
fn iarg(v: Value) -> i64 {
    v.as_i64()
}

fn key(vm: &mut Vm, s: &str) -> PropKey {
    let v = vm.new_string(s);
    vm.key_from_value(v)
}

fn keyv(vm: &mut Vm, v: Value) -> PropKey {
    vm.key_from_value(v)
}

#[allow(dead_code)]
fn as_array<'a>(_vm: &'a mut Vm, v: Value) -> VmResult<&'a mut JArray> {
    if v.is_array() {
        Ok(unsafe { v.as_array_mut() })
    } else {
        Err(type_error("expected an array"))
    }
}

/// Read `this` as an array, throwing a JS-visible TypeError when it is not one.
fn this_array(_vm: &mut Vm, this: Value) -> VmResult<&'static mut JArray> {
    if this.is_array() {
        Ok(unsafe { this.as_array_mut() })
    } else {
        Err(type_error("Array method called on a non-array"))
    }
}

/// Build a real iterator object (with `next()`) over any array or iterable.
/// Returning a bare array from `@@iterator` left `for (const x of someSet)`
/// calling `next` on an object that had none.
fn make_iter_from(vm: &mut Vm, v: Value) -> Value {
    let inner = vm.new_array();
    if v.is_array() {
        let a = unsafe { v.as_array() };
        for i in 0..a.len {
            unsafe { inner.as_array_mut() }.push(a.get(i));
        }
    } else if v.is_string() {
        let s = unsafe { v.as_string().as_str().to_string() };
        for c in s.chars() {
            let cs = vm.new_string(&c.to_string());
            unsafe { inner.as_array_mut() }.push(cs);
        }
    } else if v.is_object() {
        // Map/Set store their contents in a private array; read that directly
        // rather than calling back into `@@iterator`, which would recurse.
        let ks2 = key(vm, "#items");
        let kp2 = key(vm, "#pairs");
        let src = vm.get_prop(v, ks2).unwrap_or(Value::UNDEFINED);
        let src = if src.is_array() {
            src
        } else {
            vm.get_prop(v, kp2).unwrap_or(Value::UNDEFINED)
        };
        if src.is_array() {
            let a = unsafe { src.as_array() };
            for i in 0..a.len {
                let x = a.get(i);
                unsafe { inner.as_array_mut() }.push(x);
            }
        }
    }
    let it = vm.new_object();
    let ks = key(vm, "#src");
    let ki = key(vm, "#i");
    let kn = key(vm, "next");
    vm.set_prop(it, ks, inner).unwrap();
    vm.set_prop(it, ki, Value::from_i32(0)).unwrap();
    let nf = vm.new_function_native("next", n_iter_next, 0);
    vm.set_prop(it, kn, nf).unwrap();
    it
}

fn this_str(vm: &mut Vm, this: Value) -> VmResult<String> {
    if this.is_string() {
        Ok(unsafe { this.as_string().as_str().to_string() })
    } else {
        Ok(vm.to_string(this)?.to_string())
    }
}

fn string_units(vm: &mut Vm, value: Value) -> VmResult<Vec<u16>> {
    if value.is_string() {
        Ok(unsafe { value.as_string() }.code_units().collect())
    } else {
        Ok(vm.to_string(value)?.encode_utf16().collect())
    }
}

fn string_index(vm: &mut Vm, value: Value) -> VmResult<i64> {
    let n = vm.to_number(value)?;
    Ok(if n.is_nan() { 0 } else { n.trunc() as i64 })
}

fn to_str(vm: &mut Vm, v: Value) -> VmResult<String> {
    if let Some(s) = v.as_str() {
        return Ok(s.to_string());
    }
    vm.to_string(v)
}

fn js_str(s: &str) -> String {
    s.to_string()
}

// ---------------------------------------------------------------------------
// installation
// ---------------------------------------------------------------------------

fn obj_proto_ref(vm: &Vm) -> Value {
    Value::from_ptr(vm.proto_object_ptr())
}

pub fn install(vm: &mut Vm) {
    let a = arena::active();

    let globals = Value::from_ptr(a.alloc_object(0, Value::NULL));
    let obj_proto = Value::from_ptr(a.alloc_object(0, Value::NULL));
    // Built-in prototypes carry the engine's own methods, which are
    // non-enumerable in JavaScript. Mark them so `for-in` never yields
    // `toString`, `hasOwnProperty` and friends.
    for p in [obj_proto] {
        unsafe { p.as_object_mut() }.hdr.flags |= FLAG_HIDDEN_KEYS;
    }
    let fn_proto =
        Value::from_ptr(a.alloc_function(JFunction::native(Value::UNDEFINED, n_noop, Value::NULL)));
    unsafe { fn_proto.as_function_mut() }.proto_obj = obj_proto;
    // An object, not an array: methods live on the prototype property bag.
    let arr_proto = Value::from_ptr(a.alloc_object(0, obj_proto));
    unsafe { arr_proto.as_object_mut() }.hdr.flags |= FLAG_HIDDEN_KEYS;
    let str_proto = Value::from_ptr(a.alloc_object(0, obj_proto));
    unsafe { str_proto.as_object_mut() }.hdr.flags |= FLAG_HIDDEN_KEYS;
    vm.set_protos(globals, obj_proto, fn_proto, arr_proto, str_proto);

    // ---- Object.prototype ----
    method(vm, obj_proto, "hasOwnProperty", n_hasown);
    method(vm, obj_proto, "toString", n_obj_to_string);
    method(vm, obj_proto, "valueOf", n_obj_value_of);
    method(vm, obj_proto, "isPrototypeOf", n_is_prototype_of);
    method(vm, obj_proto, "propertyIsEnumerable", n_is_enumerable);

    // `__proto__` is an accessor on Object.prototype, not a data property, so
    // it reads and writes the internal prototype slot.
    proto_size_getter(vm, obj_proto, "__proto__", n_proto_get);
    proto_size_setter(vm, obj_proto, "__proto__", n_proto_set);

    // ---- Function.prototype ----
    method(vm, fn_proto, "call", n_fn_call);
    method(vm, fn_proto, "apply", n_fn_apply);
    method(vm, fn_proto, "bind", n_fn_bind);
    method(vm, fn_proto, "toString", n_fn_to_string);

    // ---- Array.prototype ----
    unsafe { method(vm, arr_proto, "push", n_push).as_function_mut() }
        .hdr
        .flags |= FLAG_NATIVE_LEAF;
    method(vm, arr_proto, "pop", n_pop);
    method(vm, arr_proto, "shift", n_shift);
    method(vm, arr_proto, "unshift", n_unshift);
    method(vm, arr_proto, "slice", n_slice);
    method(vm, arr_proto, "splice", n_splice);
    method(vm, arr_proto, "concat", n_concat);
    method(vm, arr_proto, "join", n_join);
    method(vm, arr_proto, "indexOf", n_index_of);
    method(vm, arr_proto, "lastIndexOf", n_last_index_of);
    method(vm, arr_proto, "includes", n_includes);
    method(vm, arr_proto, "find", n_find);
    method(vm, arr_proto, "findIndex", n_find_index);
    method(vm, arr_proto, "findLast", n_find_last);
    method(vm, arr_proto, "filter", n_filter);
    method(vm, arr_proto, "map", n_map);
    method(vm, arr_proto, "forEach", n_for_each);
    method(vm, arr_proto, "reduce", n_reduce);
    method(vm, arr_proto, "reduceRight", n_reduce_right);
    method(vm, arr_proto, "some", n_some);
    method(vm, arr_proto, "every", n_every);
    method(vm, arr_proto, "sort", n_sort);
    method(vm, arr_proto, "reverse", n_reverse);
    method(vm, arr_proto, "fill", n_fill);
    method(vm, arr_proto, "flat", n_flat);
    method(vm, arr_proto, "flatMap", n_flat_map);
    method(vm, arr_proto, "at", n_at);
    method(vm, arr_proto, "join", n_join);
    // `for (const [i, v] of arr.entries())`. Pairs are ordinary arrays: `for…of`
    // already iterates arrays, so no iterator object is needed here.
    method(vm, arr_proto, "entries", n_array_entries);
    method(vm, arr_proto, "keys", n_array_keys);
    method(vm, arr_proto, "values", n_array_values);
    method(vm, arr_proto, "toString", n_array_to_string);
    method(vm, arr_proto, ITER_SYM_KEY, |vm, this, _args| {
        Ok(make_iter_from(vm, this))
    });

    // ---- String.prototype ----
    method(vm, str_proto, "charAt", n_char_at);
    unsafe { method(vm, str_proto, "charCodeAt", n_char_code_at).as_function_mut() }
        .hdr
        .flags |= FLAG_NATIVE_PRIMITIVE_LEAF;
    method(vm, str_proto, "codePointAt", n_code_point_at);
    method(vm, str_proto, "at", n_str_at);
    method(vm, str_proto, "indexOf", n_str_index_of);
    method(vm, str_proto, "lastIndexOf", n_str_last_index_of);
    method(vm, str_proto, "includes", n_str_includes);
    method(vm, str_proto, "startsWith", n_starts_with);
    method(vm, str_proto, "endsWith", n_ends_with);
    method(vm, str_proto, "slice", n_str_slice);
    method(vm, str_proto, "substring", n_substring);
    method(vm, str_proto, "substr", n_substr);
    method(vm, str_proto, "toUpperCase", n_to_upper);
    method(vm, str_proto, "toLowerCase", n_to_lower);
    method(vm, str_proto, "toLocaleUpperCase", n_to_upper);
    method(vm, str_proto, "toLocaleLowerCase", n_to_lower);
    method(vm, str_proto, "trim", n_trim);
    method(vm, str_proto, "trimStart", n_trim_start);
    method(vm, str_proto, "trimEnd", n_trim_end);
    method(vm, str_proto, "split", n_split);
    method(vm, str_proto, "replace", n_replace);
    method(vm, str_proto, "match", n_match);
    method(vm, str_proto, "search", n_search);
    method(vm, str_proto, "replaceAll", n_replace_all);
    method(vm, str_proto, "repeat", n_repeat);
    method(vm, str_proto, "padStart", n_pad_start);
    method(vm, str_proto, "padEnd", n_pad_end);
    method(vm, str_proto, "concat", n_concat_str);
    method(vm, str_proto, "toString", n_str_to_string);
    method(vm, str_proto, "valueOf", n_str_to_string);
    method(vm, str_proto, "normalize", n_str_normalize);
    method(vm, str_proto, "localeCompare", n_locale_compare);
    method(vm, str_proto, ITER_SYM_KEY, |vm, this, _args| {
        Ok(make_iter_from(vm, this))
    });

    // ---- constructors ----
    let obj_ctor = vm.new_function_native("Object", n_object_ctor, 1);
    link(vm, obj_ctor, obj_proto);
    set_fn(vm, obj_ctor, "keys", n_object_keys);
    set_fn(vm, obj_ctor, "values", n_object_values);
    set_fn(vm, obj_ctor, "entries", n_object_entries);
    set_fn(vm, obj_ctor, "assign", n_object_assign);
    set_fn(vm, obj_ctor, "freeze", n_object_freeze);
    set_fn(vm, obj_ctor, "isFrozen", n_object_is_frozen);
    set_fn(vm, obj_ctor, "seal", n_object_seal);
    set_fn(vm, obj_ctor, "isSealed", n_object_is_sealed);
    set_fn(vm, obj_ctor, "is", n_object_is);
    set_fn(
        vm,
        obj_ctor,
        "preventExtensions",
        n_object_prevent_extensions,
    );
    set_fn(vm, obj_ctor, "isExtensible", n_object_is_extensible);
    set_fn(vm, obj_ctor, "getPrototypeOf", n_get_proto_of);
    set_fn(vm, obj_ctor, "setPrototypeOf", n_set_proto_of);
    set_fn(vm, obj_ctor, "create", n_object_create);
    set_fn(vm, obj_ctor, "defineProperty", n_define_property);
    set_fn(vm, obj_ctor, "defineProperties", n_define_properties);
    set_fn(vm, obj_ctor, "getOwnPropertyNames", n_own_names);
    set_fn(vm, obj_ctor, "getOwnPropertyDescriptor", n_own_descriptor);
    set_fn(vm, obj_ctor, "fromEntries", n_from_entries);
    set_fn(vm, obj_ctor, "isPrototypeOf", n_is_prototype_of);
    globals_def(vm, "Object", obj_ctor);

    let arr_ctor = vm.new_function_native("Array", n_array_ctor, 1);
    link(vm, arr_ctor, arr_proto);
    set_fn(vm, arr_ctor, "isArray", n_is_array);
    set_fn(vm, arr_ctor, "from", n_array_from);
    set_fn(vm, arr_ctor, "of", n_array_of);
    globals_def(vm, "Array", arr_ctor);

    let str_ctor = vm.new_function_native("String", n_string_ctor, 1);
    link(vm, str_ctor, str_proto);
    set_fn(vm, str_ctor, "raw", n_string_raw);
    set_fn(vm, str_ctor, "fromCharCode", n_from_char_code);
    set_fn(vm, str_ctor, "fromCodePoint", n_from_code_point);
    globals_def(vm, "String", str_ctor);

    let num_ctor = vm.new_function_native("Number", n_number_ctor, 1);
    let num_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto_ref(vm)));
    unsafe { num_proto.as_object_mut() }.hdr.flags |= FLAG_HIDDEN_KEYS;
    link(vm, num_ctor, num_proto);
    method(vm, num_proto, "toFixed", n_to_fixed);
    method(vm, num_proto, "toPrecision", n_to_precision);
    unsafe { method(vm, num_proto, "toString", n_number_to_string).as_function_mut() }
        .hdr
        .flags |= FLAG_NATIVE_LEAF;
    method(vm, num_proto, "valueOf", n_number_value_of);
    set_fn(vm, num_ctor, "isInteger", n_number_is_integer);
    set_fn(vm, num_ctor, "isFinite", n_number_is_finite);
    set_fn(vm, num_ctor, "isNaN", n_number_is_nan);
    set_fn(vm, num_ctor, "isSafeInteger", n_number_is_safe_integer);
    // `Number.parseInt === parseInt` in JS: share one function value rather
    // than installing two distinct natives. Same for parseFloat.
    let parse_float_fn = vm.new_function_native("parseFloat", n_parse_float, 1);
    let parse_int_fn = vm.new_function_native("parseInt", n_parse_int, 2);
    let kpf = key(vm, "parseFloat");
    let _ = vm.set_prop(num_ctor, kpf, parse_float_fn);
    let kpi = key(vm, "parseInt");
    let _ = vm.set_prop(num_ctor, kpi, parse_int_fn);
    let k = key(vm, "MAX_SAFE_INTEGER");
    vm.set_prop(num_ctor, k, Value::from_f64(9007199254740991.0))
        .unwrap();
    let k = key(vm, "MIN_SAFE_INTEGER");
    vm.set_prop(num_ctor, k, Value::from_f64(-9007199254740991.0))
        .unwrap();
    let k = key(vm, "MAX_VALUE");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::MAX)).unwrap();
    let k = key(vm, "MIN_VALUE");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::MIN_POSITIVE))
        .unwrap();
    let k = key(vm, "EPSILON");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::EPSILON))
        .unwrap();
    let k = key(vm, "POSITIVE_INFINITY");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::INFINITY))
        .unwrap();
    let k = key(vm, "NEGATIVE_INFINITY");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::NEG_INFINITY))
        .unwrap();
    let k = key(vm, "NaN");
    vm.set_prop(num_ctor, k, Value::NAN).unwrap();
    globals_def(vm, "Number", num_ctor);
    vm.set_number_proto(num_proto);

    let bool_ctor = vm.new_function_native("Boolean", n_bool_ctor, 1);
    let bool_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto_ref(vm)));
    link(vm, bool_ctor, bool_proto);
    vm.set_bool_proto(bool_proto);
    method(vm, bool_proto, "toString", n_bool_to_string);
    method(vm, bool_proto, "valueOf", n_bool_value_of);
    globals_def(vm, "Boolean", bool_ctor);

    // ---- Promise / microtasks ----
    let promise_proto = vm.new_object();
    let promise_ctor = vm.new_function_native("Promise", n_promise_ctor, 1);
    link(vm, promise_ctor, obj_proto);
    PROMISE_PROTO.with(|c| c.set(promise_proto.bits()));
    let proto_key = key(vm, "prototype");
    vm.set_prop(promise_ctor, proto_key, promise_proto).unwrap();
    method(vm, promise_proto, "then", n_promise_then);
    method(vm, promise_proto, "catch", n_promise_catch);
    set_fn(vm, promise_ctor, "resolve", n_promise_resolve_static);
    set_fn(vm, promise_ctor, "reject", n_promise_reject_static);
    globals_def(vm, "Promise", promise_ctor);
    gcd(vm, "queueMicrotask", n_queue_microtask);

    let sym_obj = vm.new_object();
    let iter_key = vm.new_string(crate::vm::ITER_SYM);
    let k = key(vm, "iterator");
    vm.set_prop(sym_obj, k, iter_key).unwrap();
    let sym_ctor = vm.new_function_native("Symbol", n_symbol_ctor, 0);
    link(vm, sym_ctor, obj_proto);
    globals_def(vm, "Symbol", sym_ctor);

    // ---- Math ----
    let math = vm.new_object();
    let mut nums: Vec<(&str, f64)> = vec![
        ("PI", std::f64::consts::PI),
        ("E", std::f64::consts::E),
        ("LN2", std::f64::consts::LN_2),
        ("LN10", std::f64::consts::LN_10),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("SQRT2", std::f64::consts::SQRT_2),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
    ];
    for (n, v) in nums.drain(..) {
        let __k = key(vm, n);
        vm.set_prop(math, __k, Value::from_f64(v)).unwrap();
    }
    m(vm, math, "abs", n_math_abs);
    m(vm, math, "ceil", n_math_ceil);
    m(vm, math, "floor", n_math_floor);
    m(vm, math, "round", n_math_round);
    m(vm, math, "trunc", n_math_trunc);
    m(vm, math, "sign", n_math_sign);
    m(vm, math, "sqrt", n_math_sqrt);
    m(vm, math, "cbrt", n_math_cbrt);
    m(vm, math, "pow", n_math_pow);
    m(vm, math, "exp", n_math_exp);
    m(vm, math, "log", n_math_log);
    m(vm, math, "log2", n_math_log2);
    m(vm, math, "log10", n_math_log10);
    m(vm, math, "sin", n_math_sin);
    m(vm, math, "cos", n_math_cos);
    m(vm, math, "tan", n_math_tan);
    m(vm, math, "asin", n_math_asin);
    m(vm, math, "acos", n_math_acos);
    m(vm, math, "atan", n_math_atan);
    m(vm, math, "atan2", n_math_atan2);
    m(vm, math, "sinh", n_math_sinh);
    m(vm, math, "cosh", n_math_cosh);
    m(vm, math, "tanh", n_math_tanh);
    m(vm, math, "hypot", n_math_hypot);
    m(vm, math, "min", n_math_min);
    m(vm, math, "max", n_math_max);
    m(vm, math, "random", n_math_random);
    m(vm, math, "imul", n_math_imul);
    m(vm, math, "clz32", n_math_clz32);
    m(vm, math, "fround", n_math_fround);
    globals_def(vm, "Math", math);

    // ---- JSON ----
    let json = vm.new_object();
    m(vm, json, "parse", n_json_parse);
    m(vm, json, "stringify", n_json_stringify);
    globals_def(vm, "JSON", json);

    // ---- console / process ----
    let console = vm.new_object();
    m(vm, console, "log", n_console_log);
    m(vm, console, "info", n_console_log);
    m(vm, console, "debug", n_console_log);
    m(vm, console, "error", n_console_error);
    m(vm, console, "warn", n_console_warn);
    m(vm, console, "trace", n_console_log);
    globals_def(vm, "console", console);

    let proc = vm.new_object();
    set_obj_str(vm, proc, "platform", std::env::consts::OS);
    set_obj_str(vm, proc, "version", crate::JHOLVM_VERSION);
    set_obj_str(vm, proc, "arch", std::env::consts::ARCH);
    let argv = vm.new_array();
    let a0 = vm.new_string("jhol");
    unsafe { argv.as_array_mut().push(a0) };
    for a in std::env::args().skip(1) {
        let s = vm.new_string(&a);
        unsafe { argv.as_array_mut().push(s) };
    }
    let k = key(vm, "argv");
    vm.set_prop(proc, k, argv).unwrap();
    let env = vm.new_object();
    for (k, v) in std::env::vars() {
        let kn = key(vm, &k);
        let vn = vm.new_string(&v);
        vm.set_prop(env, kn, vn).unwrap();
    }
    let k = key(vm, "env");
    vm.set_prop(proc, k, env).unwrap();
    m(vm, proc, "exit", n_process_exit);
    m(vm, proc, "hrtime", n_process_hrtime);
    m(vm, proc, "cwd", n_process_cwd);
    globals_def(vm, "process", proc);

    globals_def(vm, "globalThis", globals);
    globals_def(vm, "undefined", Value::UNDEFINED);

    // ---- global functions ----
    globals_def(vm, "parseInt", parse_int_fn);
    globals_def(vm, "parseFloat", parse_float_fn);
    gcd(vm, "isNaN", n_global_is_nan);
    gcd(vm, "isFinite", n_global_is_finite);
    gcd(vm, "setTimeout", n_set_timeout);
    gcd(vm, "setInterval", n_set_interval);
    gcd(vm, "clearTimeout", n_clear_timer);
    gcd(vm, "clearInterval", n_clear_timer);

    gcd(vm, "__jhol_push", n_h_push);
    gcd(vm, "__jhol_push_all", n_h_push_all);
    gcd(vm, "__jhol_iter_to_array", n_iter_to_array);
    gcd(vm, "__jhol_apply", n_h_apply);
    gcd(vm, "__jhol_apply_new", n_h_apply_new);
    gcd(vm, "__jhol_tonum", n_h_tonum);
    gcd(vm, "__jhol_enum_keys", n_h_enum_keys);
    // CommonJS `require` — resolution and execution live in `modules.rs`;
    // the per-module directory comes from the Vm's module stack, so the
    // global is a single stable native.
    gcd(vm, "require", crate::modules::n_require);
    gcd(vm, "__jhol_values", n_h_values);
    gcd(vm, "__jhol_rest_object", n_h_rest_object);
    gcd(vm, "__jhol_assign_all", n_h_assign_all);
    gcd(vm, "__jhol_new_regexp", n_h_new_regexp);

    // ---- RegExp ----
    // Prototype first: `make_regexp` links every literal and constructor call to
    // it, and without a prototype a regex object has no `test`/`exec`.
    let regex_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    REGEX_PROTO.with(|p| p.set(Some(regex_proto)));
    method(vm, regex_proto, "test", n_regexp_test);
    method(vm, regex_proto, "exec", n_regexp_exec);
    method(vm, regex_proto, "toString", n_regexp_to_string);
    let regexp_ctor = vm.new_function_native("RegExp", n_regexp_ctor, 2);
    link(vm, regexp_ctor, regex_proto);
    globals_def(vm, "RegExp", regexp_ctor);
    gcd(vm, "__jhol_rest_array", n_h_rest_array);
    gcd(vm, "__jhol_tagged_template", n_h_tagged_template);
    gcd(vm, "__jhol_set_proto", n_h_set_proto);

    // ---- Error hierarchy ----
    let err_ctor = vm.new_function_native("Error", n_error_ctor, 1);
    let err_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    link(vm, err_ctor, err_proto);
    method(vm, err_proto, "toString", n_error_to_string);
    set_obj_str(vm, err_proto, "name", "Error");
    globals_def(vm, "Error", err_ctor);
    for (name, _ctor) in [
        ("TypeError", "TypeError"),
        ("RangeError", "RangeError"),
        ("SyntaxError", "SyntaxError"),
        ("ReferenceError", "ReferenceError"),
        ("EvalError", "EvalError"),
        ("URIError", "URIError"),
    ] {
        let sub = make_error_sub(vm, err_proto, name);
        globals_def(vm, name, sub);
        // Register the *prototype*, not the constructor: `instanceof` compares
        // the object's prototype chain against `TypeError.prototype`, so a
        // constructor here made `e instanceof TypeError` false for real errors.
        let kp = key(vm, "prototype");
        let proto = vm.get_prop(sub, kp).unwrap_or(err_proto);
        ERROR_PROTOS.with(|m| {
            m.borrow_mut().insert(
                match name {
                    "TypeError" => "TypeError",
                    "RangeError" => "RangeError",
                    "SyntaxError" => "SyntaxError",
                    "ReferenceError" => "ReferenceError",
                    "EvalError" => "EvalError",
                    _ => "URIError",
                },
                proto,
            );
        });
    }

    // ---- Map / Set ----
    let map_ctor = vm.new_function_native("Map", n_map_ctor, 0);
    let map_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    link(vm, map_ctor, map_proto);
    m(vm, map_proto, "set", n_map_set);
    m(vm, map_proto, "get", n_map_get);
    m(vm, map_proto, "has", n_map_has);
    m(vm, map_proto, "delete", n_map_delete);
    m(vm, map_proto, "clear", n_map_clear);
    m(vm, map_proto, "forEach", n_map_for_each);
    m(vm, map_proto, "keys", n_map_keys);
    m(vm, map_proto, "values", n_map_values);
    m(vm, map_proto, "entries", n_map_entries);
    m(vm, map_proto, ITER_SYM_KEY, |vm, this, _args| {
        Ok(make_iter_from(vm, this))
    });
    install_size_getter(vm, map_proto, n_map_size);
    globals_def(vm, "Map", map_ctor);

    let set_ctor = vm.new_function_native("Set", n_set_ctor, 0);
    let set_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    link(vm, set_ctor, set_proto);
    m(vm, set_proto, "add", n_set_add);
    m(vm, set_proto, "has", n_set_has);
    m(vm, set_proto, "delete", n_set_delete);
    m(vm, set_proto, "clear", n_set_clear);
    m(vm, set_proto, "forEach", n_set_for_each);
    m(vm, set_proto, "values", n_set_values);
    m(vm, set_proto, "keys", n_set_values);
    m(vm, set_proto, "entries", n_set_entries);
    m(vm, set_proto, ITER_SYM_KEY, |vm, this, _args| {
        Ok(make_iter_from(vm, this))
    });
    install_size_getter(vm, set_proto, n_set_size);
    globals_def(vm, "Set", set_ctor);

    // ---- Function (the constructor itself; instances already have
    // call/apply/bind/name/length) ----
    let fn_ctor = vm.new_function_native("Function", n_function_ctor, 1);
    // `Function.prototype` is the engine's own function prototype, so
    // `Function.prototype.call` and friends resolve to the same methods
    // every function already carries.
    let fn_proto_v = Value::from_ptr(vm.proto_function as *mut crate::objects::JFunction);
    let k = key(vm, "prototype");
    vm.set_prop(fn_ctor, k, fn_proto_v).unwrap();
    set_obj_str(vm, fn_proto_v, "name", "");
    globals_def(vm, "Function", fn_ctor);

    // ---- Symbol statics ----
    set_fn(vm, sym_ctor, "for", n_symbol_for);
    set_fn(vm, sym_ctor, "keyFor", n_symbol_key_for);
    // Well-known symbols are real symbols whose id is the *internal* key the
    // protocols already use (`@@iterator`), so `obj[Symbol.iterator]` and
    // `for…of` resolve to the same property. Before this they were strings
    // spelled "Symbol(iterator)" while the protocol looked for "@@iterator", so the
    // two never met: `obj[Symbol.iterator]` was undefined even on arrays.
    for (prop, desc) in [
        ("iterator", "iterator"),
        ("asyncIterator", "asyncIterator"),
        ("hasInstance", "hasInstance"),
        ("isConcatSpreadable", "isConcatSpreadable"),
        ("toPrimitive", "toPrimitive"),
        ("toStringTag", "toStringTag"),
        ("species", "species"),
        ("replace", "replace"),
        ("search", "search"),
        ("split", "split"),
        ("match", "match"),
        ("matchAll", "matchAll"),
    ] {
        let v = register_symbol(
            vm,
            format!("@@{}", desc),
            format!("Symbol.{}", desc),
        );
        let k = key(vm, prop);
        vm.set_prop(sym_ctor, k, v).unwrap();
    }

    // ---- atob / btoa (base64, latin1 semantics) ----
    gcd(vm, "atob", n_atob);
    gcd(vm, "btoa", n_btoa);

    // ---- WeakMap / WeakSet: identity-keyed, GC-naive (entries whose key
    // dies stay until cleared; behavior for live keys is exact) ----
    let wmap_ctor = vm.new_function_native("WeakMap", n_weakmap_ctor, 0);
    let wmap_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    link(vm, wmap_ctor, wmap_proto);
    m(vm, wmap_proto, "set", n_weakmap_set);
    m(vm, wmap_proto, "get", n_weakmap_get);
    m(vm, wmap_proto, "has", n_weakmap_has);
    m(vm, wmap_proto, "delete", n_weakmap_delete);
    globals_def(vm, "WeakMap", wmap_ctor);

    let wset_ctor = vm.new_function_native("WeakSet", n_weakset_ctor, 0);
    let wset_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    link(vm, wset_ctor, wset_proto);
    m(vm, wset_proto, "add", n_weakset_add);
    m(vm, wset_proto, "has", n_weakset_has);
    m(vm, wset_proto, "delete", n_weakset_delete);
    globals_def(vm, "WeakSet", wset_ctor);

    // ---- crypto ----
    let crypto_obj = vm.new_object();
    set_fn(
        vm,
        crypto_obj,
        "getRandomValues",
        n_crypto_get_random_values,
    );
    set_fn(vm, crypto_obj, "randomUUID", n_crypto_random_uuid);
    let k = key(vm, "crypto");
    vm.define_prop(Value::from_ptr(vm.globals), k, crypto_obj);

    // ---- Date ----
    let date_ctor = vm.new_function_native("Date", n_date_ctor, 7);
    let date_proto = Value::from_ptr(arena::active().alloc_object(0, obj_proto));
    link(vm, date_ctor, date_proto);
    set_obj_str(vm, date_proto, "name", "Date");
    let date_methods: [(&str, crate::objects::NativeFn); 36] = [
        ("getTime", n_date_get_time),
        ("valueOf", n_date_get_time),
        ("getFullYear", n_date_get_year),
        ("getUTCFullYear", n_date_get_year_utc),
        ("getMonth", n_date_get_month),
        ("getUTCMonth", n_date_get_month_utc),
        ("getDate", n_date_get_date),
        ("getUTCDate", n_date_get_date_utc),
        ("getDay", n_date_get_day),
        ("getUTCDay", n_date_get_day_utc),
        ("getHours", n_date_get_hours),
        ("getUTCHours", n_date_get_hours_utc),
        ("getMinutes", n_date_get_minutes),
        ("getUTCMinutes", n_date_get_minutes_utc),
        ("getSeconds", n_date_get_seconds),
        ("getUTCSeconds", n_date_get_seconds_utc),
        ("getMilliseconds", n_date_get_ms),
        ("getUTCMilliseconds", n_date_get_ms),
        ("getTimezoneOffset", n_date_tz_offset),
        ("toISOString", n_date_to_iso),
        ("toJSON", n_date_to_iso),
        ("toString", n_date_to_string),
        ("toUTCString", n_date_to_utc_string),
        ("toDateString", n_date_to_date_string),
        ("toTimeString", n_date_to_time_string),
        ("setTime", n_date_set_time),
        ("setMilliseconds", n_date_set_ms),
        ("setSeconds", n_date_set_seconds),
        ("setMinutes", n_date_set_minutes),
        ("setHours", n_date_set_hours),
        ("setDate", n_date_set_date),
        ("setMonth", n_date_set_month),
        ("setFullYear", n_date_set_year),
        ("setUTCFullYear", n_date_set_year_utc),
        ("setUTCMonth", n_date_set_month_utc),
        ("setUTCDate", n_date_set_date_utc),
    ];
    for (name, f) in date_methods {
        m(vm, date_proto, name, f);
    }
    globals_def(vm, "Date", date_ctor);
    set_fn(vm, date_ctor, "now", n_date_now);
    set_fn(vm, date_ctor, "parse", n_date_parse);
    set_fn(vm, date_ctor, "UTC", n_date_utc);
}

pub const ITER_SYM_KEY: &str = "@@iterator";

fn globals_def(vm: &mut Vm, name: &str, v: Value) {
    let k = key(vm, name);
    vm.define_prop(Value::from_ptr(vm.globals), k, v);
}

fn gcd(vm: &mut Vm, name: &str, f: NativeFn) {
    let v = vm.new_function_native(name, f, 0);
    globals_def(vm, name, v);
}

fn method(vm: &mut Vm, obj: Value, name: &str, f: NativeFn) -> Value {
    let v = vm.new_function_native(name, f, 0);
    let k = key(vm, name);
    vm.define_prop(obj, k, v);
    v
}

fn m(vm: &mut Vm, obj: Value, name: &str, f: NativeFn) {
    method(vm, obj, name, f);
}

fn set_fn(vm: &mut Vm, obj: Value, name: &str, f: NativeFn) {
    let v = vm.new_function_native(name, f, 1);
    let k = key(vm, name);
    vm.set_prop(obj, k, v).unwrap();
}

fn set_obj_str(vm: &mut Vm, obj: Value, name: &str, val: &str) {
    let v = vm.new_string(val);
    let k = key(vm, name);
    vm.set_prop(obj, k, v).unwrap();
}

fn proto_size_getter(vm: &mut Vm, proto: Value, name: &str, f: NativeFn) {
    let base = arena::active().intern_sym(name);
    let acc = arena::active().accessor_sym(base, "#get:");
    let fnv = vm.new_function_native(name, f, 0);
    vm.define_prop(proto, PropKey::Sym(acc), fnv);
    vm.mark_accessor(proto);
}

fn proto_size_setter(vm: &mut Vm, proto: Value, name: &str, f: NativeFn) {
    let base = arena::active().intern_sym(name);
    let acc = arena::active().accessor_sym(base, "#set:");
    let fnv = vm.new_function_native(name, f, 1);
    vm.define_prop(proto, PropKey::Sym(acc), fnv);
    vm.mark_accessor(proto);
}

// Accessor forms: `this` is the object whose prototype is read or written.
// `obj.__proto__` — the object's [[Prototype]] (`null` when there is none).
// This used to return `this`, so `x.__proto__ === x` for every object.
native!(n_proto_get, |vm, this, _args| {
    let p = if this.is_object() {
        unsafe { this.as_object() }.proto
    } else if this.is_array() {
        unsafe { this.as_array() }.proto
    } else if this.is_function() {
        unsafe { this.as_function() }.proto
    } else if this.is_string() || this.is_number() || this.is_bool() {
        // A primitive's [[Prototype]] is its wrapper prototype
        // (`String.prototype`, `Number.prototype`, `Boolean.prototype`).
        let p = vm.proto_of(this);
        if p.is_object() {
            return Ok(p);
        }
        return Ok(Value::NULL);
    } else {
        return Ok(Value::UNDEFINED);
    };
    // Unset prototype slots are stored as non-objects; report them as `null`.
    if p.is_object() || p.is_function() || p.is_array() {
        Ok(p)
    } else {
        Ok(Value::NULL)
    }
});

// `obj.__proto__ = p` — set [[Prototype]] when `p` is an object or `null`;
// anything else is ignored, as in the specification.
native!(n_proto_set, |vm, this, args| {
    let p = arg(args, 0);
    if this.is_null() || this.is_undefined() {
        return Err(type_error("cannot set prototype of null or undefined"));
    }
    if !(p.is_object() || p.is_function() || p.is_array() || p.is_null()) {
        return Ok(Value::UNDEFINED);
    }
    vm.set_prototype(this, p)?;
    Ok(Value::UNDEFINED)
});

/// `size` is a getter, so it goes through the `#get:` accessor channel the
/// property store already understands.
fn install_size_getter(vm: &mut Vm, proto: Value, f: NativeFn) {
    let base = arena::active().intern_sym("size");
    let acc = arena::active().accessor_sym(base, "#get:");
    let fnv = vm.new_function_native("size", f, 0);
    vm.define_prop(proto, PropKey::Sym(acc), fnv);
    // The flag is what lets a property store skip the prototype scan, so every
    // path that installs an accessor has to set it.
    vm.mark_accessor(proto);
}

fn link(vm: &mut Vm, ctor: Value, proto: Value) {
    let k = key(vm, "prototype");
    vm.set_prop(ctor, k, proto).unwrap();
    let k = key(vm, "constructor");
    vm.set_prop(proto, k, ctor).unwrap();
}

fn make_error_sub(vm: &mut Vm, parent_proto: Value, name: &str) -> Value {
    let proto = Value::from_ptr(arena::active().alloc_object(0, parent_proto));
    let ctor = vm.new_function_native(name, n_error_ctor, 1);
    link(vm, ctor, proto);
    set_obj_str(vm, proto, "name", name);
    method(vm, proto, "toString", n_error_to_string);
    ctor
}

#[allow(dead_code)]
fn err(_vm: &mut Vm) -> Value {
    Value::UNDEFINED
}

native!(n_noop, |_vm, _this, _args| Ok(Value::UNDEFINED));
native!(n_obj_value_of, |_vm, this, _args| Ok(this));
// id -> description, so a symbol prints as `Symbol(desc)` and `keyFor` can answer.
thread_local! {
    static SYM_DESC: std::cell::RefCell<std::collections::HashMap<u32, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static SYM_NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Description of a symbol id, for printing and `Symbol.keyFor`.
pub fn symbol_description(id: u32) -> Option<String> {
    SYM_DESC.with(|m| m.borrow().get(&id).cloned())
}

fn register_symbol(vm: &mut Vm, key: String, desc: String) -> Value {
    let id = vm.arena().intern_sym(&key);
    SYM_DESC.with(|m| {
        m.borrow_mut().insert(id, desc);
    });
    Value::from_symbol(id)
}

native!(n_symbol_ctor, |vm, _this, args| {
    let d = match args.first() {
        Some(v) if !v.is_undefined() => to_str(vm, *v)?,
        _ => String::new(),
    };
    // Unique key per call: two `Symbol("x")` must not be equal.
    let n = SYM_NEXT.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    Ok(register_symbol(vm, format!("\u{0}sym:{}:{}", n, d), d))
});
native!(n_is_enumerable, |vm, this, args| {
    let k = keyv(vm, arg(args, 0));
    if this.is_object() {
        return Ok(Value::from_bool(
            unsafe { this.as_object() }.find_index(k).is_some(),
        ));
    }
    Ok(Value::from_bool(false))
});
native!(n_gen_self, |_vm, this, _args| Ok(this));
native!(n_gen_next, |vm, this, args| vm.resume(this, arg(args, 0)));

// ---------------------------------------------------------------------------
// Object
// ---------------------------------------------------------------------------

native!(n_object_ctor, |vm, _this, args| {
    let v = arg(args, 0);
    if v.is_nil() || v.is_undefined() {
        return Ok(vm.new_object());
    }
    Ok(v)
});
native!(n_hasown, |vm, this, args| {
    let k = keyv(vm, arg(args, 0));
    let r = match unsafe { this.obj_tag() } {
        Some(TAG_OBJECT) => unsafe { this.as_object() }.find_index(k).is_some(),
        Some(TAG_FUNCTION) => unsafe { this.as_function() }.find_index(k).is_some(),
        Some(TAG_ARRAY) => {
            let a = unsafe { this.as_array() };
            match k {
                PropKey::Idx(i) => a.get_raw(i).is_some(),
                PropKey::Sym(s) => {
                    s == arena::active().intern_sym("length")
                        || a.props
                            .as_ref()
                            .is_some_and(|p| p.iter().any(|(pk, _)| *pk == k))
                }
            }
        }
        _ => false,
    };
    Ok(Value::from_bool(r))
});
native!(n_obj_to_string, |vm, this, _args| {
    // Primitives classify by their builtin tag; only null/undefined escape
    // the receiver coercion node applies.
    let n = if this.is_null() {
        "Null"
    } else if this.is_undefined() {
        "Undefined"
    } else if this.is_number() {
        "Number"
    } else if this.is_bool() {
        "Boolean"
    } else if this.is_string() {
        "String"
    } else {
        match unsafe { this.obj_tag() } {
            Some(TAG_ARRAY) => "Array",
            Some(TAG_FUNCTION) => "Function",
            _ => "Object",
        }
    };
    let _ = vm;
    let s = format!("[object {}]", n);
    Ok(vm.new_string(&s))
});
native!(n_is_prototype_of, |vm, this, args| {
    let o = arg(args, 0);
    let mut cur = o;
    let mut guard = 0;
    while guard < 128 {
        let p = match unsafe { cur.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { cur.as_object() }.proto,
            Some(TAG_FUNCTION) => unsafe { cur.as_function() }.proto,
            Some(TAG_ARRAY) => unsafe { cur.as_array() }.proto,
            _ => break,
        };
        if p.bits() == this.bits() {
            return Ok(Value::TRUE);
        }
        cur = p;
        guard += 1;
    }
    Ok(Value::from_bool(false))
});

/// An array-index property: canonical decimal, no leading zero, below 2^32-1.
/// `"01"` and `"-1"` are ordinary string keys, which is why `{01:…,1:…}`
/// enumerates `1` first and `01` after it.
fn canonical_index(key: &PropKey, vm: &mut Vm) -> Option<u32> {
    match key {
        PropKey::Idx(i) => Some(*i),
        PropKey::Sym(_) => {
            let name = vm.prop_name(*key);
            if name.is_empty() || name.len() > 10 || name.starts_with('0') {
                return None;
            }
            if !name.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            name.parse::<u32>().ok().filter(|n| *n < u32::MAX)
        }
    }
}

/// JS own-key order: array indices ascending first, then the rest in insertion
/// order. The store is insertion-ordered only, so `{b:1,"2":2,a:3,"1":4}` used to
/// enumerate `b,2,a,1` where JS says `1,2,b,a` — visible in `Object.keys`,
/// `for…in` and `JSON.stringify`.
fn sort_own_keys(vm: &mut Vm, mut keys: Vec<PropKey>) -> Vec<PropKey> {
    if keys.len() < 2 || !keys.iter().any(|k| canonical_index(k, vm).is_some()) {
        return keys;
    }
    let mut numeric: Vec<(u32, PropKey)> = Vec::new();
    let mut rest: Vec<PropKey> = Vec::new();
    for k in keys.drain(..) {
        match canonical_index(&k, vm) {
            Some(n) => numeric.push((n, k)),
            None => rest.push(k),
        }
    }
    numeric.sort_by_key(|(n, _)| *n);
    numeric.into_iter().map(|(_, k)| k).chain(rest).collect()
}

fn own_keys_of(vm: &mut Vm, v: Value) -> Vec<PropKey> {
    let keys = match unsafe { v.obj_tag() } {
        Some(TAG_OBJECT) => unsafe { v.as_object() }.own_keys(),
        Some(TAG_FUNCTION) => unsafe { v.as_function() }.keys.clone(),
        Some(TAG_ARRAY) => unsafe { v.as_array().own_keys() },
        _ => Vec::new(),
    };
    sort_own_keys(vm, keys)
}

native!(n_object_keys, |vm, _this, args| {
    let v = arg(args, 0);
    // Object.keys is enumerable own keys only: accessor slots (#get:/#set:)
    // are engine-internal and hidden here. (Node's fuller story needs
    // per-property enumerable flags — the recorded design debt.)
    let keys = public_keys(vm, v);
    let out = vm.new_array();
    for k in keys {
        let s = vm.new_string(&vm.prop_name(k));
        unsafe { out.as_array_mut().push(s) };
    }
    Ok(out)
});

/// `#get:` / `#set:` prefixed keys are the engine's internal accessor slots;
/// every user-visible key list must filter them (node has no such
/// properties).
/// Own property *names*, including accessors. Getters and setters live under
/// internal `#get:`/`#set:` keys, so a plain key filter dropped them and
/// `getOwnPropertyNames({get x(){}})` came back empty (node: `["x"]`).
fn own_property_names(vm: &mut Vm, v: Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in own_keys_of(vm, v) {
        let name = vm.prop_name(k);
        if let Some(rest) = name
            .strip_prefix("#get:")
            .or_else(|| name.strip_prefix("#set:"))
        {
            if !out.iter().any(|n| n == rest) {
                out.push(rest.to_string());
            }
        } else if !name.starts_with('#') {
            out.push(name);
        }
    }
    out
}

fn public_keys(vm: &mut Vm, v: Value) -> Vec<PropKey> {
    own_keys_of(vm, v)
        .into_iter()
        .filter(|k| !vm.prop_name(*k).starts_with('#'))
        .collect()
}
native!(n_object_values, |vm, _this, args| {
    let v = arg(args, 0);
    let keys = own_keys_of(vm, v);
    let out = vm.new_array();
    for k in keys {
        let val = match unsafe { v.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { v.as_object() }.get(k).unwrap_or(Value::UNDEFINED),
            Some(TAG_ARRAY) => unsafe { v.as_array() }.get(match k {
                PropKey::Idx(i) => i,
                _ => 0,
            }),
            _ => Value::UNDEFINED,
        };
        unsafe { out.as_array_mut().push(val) };
    }
    Ok(out)
});
native!(n_object_entries, |vm, _this, args| {
    let v = arg(args, 0);
    let keys = own_keys_of(vm, v);
    let out = vm.new_array();
    for k in keys {
        let val = match unsafe { v.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { v.as_object() }.get(k).unwrap_or(Value::UNDEFINED),
            Some(TAG_ARRAY) => unsafe { v.as_array() }.get(match k {
                PropKey::Idx(i) => i,
                _ => 0,
            }),
            _ => Value::UNDEFINED,
        };
        let pair = vm.new_array();
        let ks = vm.new_string(&vm.prop_name(k));
        unsafe {
            pair.as_array_mut().push(ks);
            pair.as_array_mut().push(val);
            out.as_array_mut().push(pair);
        }
    }
    Ok(out)
});
native!(n_object_assign, |vm, _this, args| {
    let target = arg(args, 0);
    for src in args.iter().skip(1) {
        if !src.is_object() && !src.is_function() {
            continue;
        }
        let keys = own_keys_of(vm, *src);
        for k in keys {
            let val = vm.get_prop(*src, k)?;
            vm.set_prop(target, k, val)?;
        }
    }
    Ok(target)
});
native!(n_object_freeze, |_vm, _this, args| {
    // A static method: it operates on the argument, not on the receiver.
    let v = arg(args, 0);
    if v.is_object() {
        unsafe { v.as_object_mut() }.extensible = false;
    }
    Ok(v)
});
native!(n_object_is_frozen, |vm, _this, args| {
    let v = arg(args, 0);
    let f = !v.is_object() || !unsafe { v.as_object() }.extensible;
    Ok(Value::from_bool(f))
});
// This object model has no per-property writability flags, so seal — like
// freeze — is exactly "stop adding and overwriting": extensible = false.
native!(n_object_seal, |_vm, _this, args| {
    let v = arg(args, 0);
    if v.is_object() {
        unsafe { v.as_object_mut() }.extensible = false;
    }
    Ok(v)
});
native!(n_object_is_sealed, |vm, _this, args| {
    let v = arg(args, 0);
    let f = !v.is_object() || !unsafe { v.as_object() }.extensible;
    Ok(Value::from_bool(f))
});

/// `Object.is` — SameValue: NaN equals NaN, and +0 / -0 are distinct.
native!(n_object_is, |_vm, _this, args| {
    let a = arg(args, 0);
    let b = arg(args, 1);
    if a.is_number() && b.is_number() {
        let x = a.as_f64();
        let y = b.as_f64();
        if x.is_nan() && y.is_nan() {
            return Ok(Value::TRUE);
        }
        return Ok(Value::from_bool(
            x == y && (x != 0.0 || x.is_sign_negative() == y.is_sign_negative()),
        ));
    }
    Ok(Value::from_bool(_vm.strict_eq(a, b).unwrap_or(false)))
});

native!(n_object_prevent_extensions, |_vm, _this, args| {
    let v = arg(args, 0);
    if v.is_object() {
        unsafe { v.as_object_mut() }.extensible = false;
    }
    Ok(v)
});
native!(n_object_is_extensible, |vm, _this, args| {
    let v = arg(args, 0);
    let f = v.is_object() && unsafe { v.as_object() }.extensible;
    Ok(Value::from_bool(f))
});
native!(n_get_proto_of, |vm, _this, args| Ok(
    vm.proto_of(arg(args, 0))
));
native!(n_set_proto_of, |vm, _this, args| {
    let o = arg(args, 0);
    let p = arg(args, 1);
    vm.set_prototype(o, p)?;
    Ok(o)
});
native!(n_object_create, |vm, _this, args| {
    let p = arg(args, 0);
    let v = Value::from_ptr(arena::active().alloc_object(0, p));
    if args.len() > 1 {
        let props = arg(args, 1);
        let keys = own_keys_of(vm, props);
        for k in keys {
            let val = vm.get_prop(props, k)?;
            vm.define_prop(v, k, val);
        }
    }
    Ok(v)
});
native!(n_define_property, |vm, _this, args| {
    let target = arg(args, 0);
    let kk = to_str(vm, arg(args, 1))?;
    let k = key(vm, &kk);
    let desc = arg(args, 2);
    let __k = key(vm, "get");
    let getter = vm.get_prop(desc, __k)?;
    let __k = key(vm, "set");
    let setter = vm.get_prop(desc, __k)?;
    let __k = key(vm, "value");
    let val = vm.get_prop(desc, __k)?;
    if getter.is_function() || setter.is_function() {
        if getter.is_function() {
            let gk = arena::active().accessor_sym(
                match k {
                    PropKey::Sym(s) => s,
                    _ => 0,
                },
                "#get:",
            );
            vm.define_prop(target, PropKey::Sym(gk), getter);
        }
        if setter.is_function() {
            let sk = arena::active().accessor_sym(
                match k {
                    PropKey::Sym(s) => s,
                    _ => 0,
                },
                "#set:",
            );
            vm.define_prop(target, PropKey::Sym(sk), setter);
        }
        // Same invariant as above: an installed accessor must be visible to
        // the property store's fast path.
        vm.mark_accessor(target);
        if !getter.is_function() && !setter.is_function() {
            vm.define_prop(target, k, val);
        }
    } else {
        vm.define_prop(target, k, val);
    }
    Ok(target)
});
native!(n_define_properties, |vm, _this, args| {
    let target = arg(args, 0);
    let props = arg(args, 1);
    let keys = own_keys_of(vm, props);
    for k in keys {
        let val = vm.get_prop(props, k)?;
        let __k = key(vm, "value");
        let sub = vm.get_prop(val, __k)?;
        vm.define_prop(target, k, sub);
    }
    Ok(target)
});
native!(n_own_names, |vm, _this, args| {
    let v = arg(args, 0);
    let out = vm.new_array();
    for name in own_property_names(vm, v) {
        let s = vm.new_string(&name);
        unsafe { out.as_array_mut().push(s) };
    }
    Ok(out)
});
native!(n_own_descriptor, |_vm, _this, _args| Ok(Value::UNDEFINED));
native!(n_from_entries, |vm, _this, args| {
    let it = arg(args, 0);
    let out = vm.new_object();
    let values = collect_iterable(vm, it)?;
    for pair in values {
        if pair.is_array() {
            let a = unsafe { pair.as_array() };
            if a.len >= 2 {
                let k = a.get(0);
                let v = a.get(1);
                let kk = keyv(vm, k);
                vm.set_prop(out, kk, v).unwrap();
            }
        }
    }
    Ok(out)
});

// ---------------------------------------------------------------------------
// Function
// ---------------------------------------------------------------------------

native!(n_fn_call, |vm, this, args| {
    let recv = arg(args, 0);
    vm.call_callback(this, recv, &args[1.min(args.len())..])
});
native!(n_fn_apply, |vm, this, args| {
    let recv = arg(args, 0);
    let list = arg(args, 1);
    let vals = if list.is_array() {
        let a = unsafe { list.as_array() };
        a.data.clone()
    } else {
        Vec::new()
    };
    vm.call_callback(this, recv, &vals)
});
native!(n_fn_bind, |vm, this, args| {
    if !this.is_function() {
        return Err(type_error("bind target is not callable"));
    }
    let recv = arg(args, 0);
    let bound = args[1.min(args.len())..].to_vec();
    let f = JFunction::native(Value::UNDEFINED, n_noop, Value::UNDEFINED);
    let fp = vm.arena().alloc_function(f);
    let v = Value::from_ptr(fp);
    unsafe {
        (*fp).hdr.flags |= FLAG_BOUND;
        crate::arena::note_external_allocation(bound.capacity() * std::mem::size_of::<Value>());
        (*fp).bound = bound;
        (*fp).this_val = this;
        (*fp).bound_this = recv;
        (*fp).proto = Value::from_ptr(vm.proto_function_ptr() as *mut JFunction);
        (*fp).proto_obj = this.as_function().proto_obj;
    }
    Ok(v)
});
native!(n_fn_to_string, |vm, this, _args| {
    let n = if unsafe { this.as_function() }.name.is_string() {
        unsafe { this.as_function().name.as_string().as_str().to_string() }
    } else {
        String::new()
    };
    Ok(vm.new_string(&format!("function {}() {{ [native code] }}", n)))
});

// ---------------------------------------------------------------------------
// Array
// ---------------------------------------------------------------------------

fn collect_iterable(vm: &mut Vm, v: Value) -> VmResult<Vec<Value>> {
    let mut out = Vec::new();
    if v.is_array() {
        let a = unsafe { v.as_array() };
        for i in 0..a.len {
            out.push(a.get(i));
        }
        vm.pin_temporaries(&out);
        return Ok(out);
    }
    // Iterator protocol: v[@@iterator]() then next() until done.
    let itk = key(vm, ITER_SYM_KEY);
    let make = vm.get_prop(v, itk)?;
    let iter = if make.is_function() {
        vm.call_direct_args(make, v, &[])?
    } else {
        return Err(type_error("value is not iterable"));
    };
    let nk = key(vm, "next");
    let next = vm.get_prop(iter, nk)?;
    if !next.is_function() {
        return Err(type_error("iterator has no next method"));
    }
    let mut guard = 0;
    loop {
        let res = vm.call_callback(next, iter, &[])?;
        let dk = key(vm, "done");
        let done = vm.get_prop(res, dk)?;
        if done.to_bool() {
            break;
        }
        let vk = key(vm, "value");
        out.push(vm.get_prop(res, vk)?);
        guard += 1;
        if guard > 1_000_000 {
            break;
        }
    }
    Ok(out)
}

native!(n_array_ctor, |vm, _this, args| {
    let out = vm.new_array();
    if args.len() == 1 && args[0].is_number() {
        let n = args[0].as_i64();
        if n > 0 && n < 100_000_000 {
            for _ in 0..n {
                unsafe { out.as_array_mut().push(Value::UNDEFINED) };
            }
            return Ok(out);
        }
    }
    for a in args {
        unsafe { out.as_array_mut().push(*a) };
    }
    Ok(out)
});
native!(n_is_array, |_vm, _this, args| Ok(Value::from_bool(
    arg(args, 0).is_array()
)));
native!(n_array_from, |vm, _this, args| {
    let src = arg(args, 0);
    let mut vals = collect_iterable(vm, src)?;
    let mapfn = arg(args, 1);
    if mapfn.is_function() {
        for (i, v) in vals.iter_mut().enumerate() {
            let idx = Value::from_i32(i as i32);
            let r = vm.call_callback(mapfn, Value::UNDEFINED, &[*v, idx])?;
            *v = r;
        }
    }
    let out = vm.new_array();
    for v in vals {
        unsafe { out.as_array_mut().push(v) };
    }
    Ok(out)
});
native!(n_array_of, |vm, _this, args| {
    let out = vm.new_array();
    for a in args {
        unsafe { out.as_array_mut().push(*a) };
    }
    Ok(out)
});
native!(n_push, |vm, this, args| {
    let a = this_array(vm, this)?;
    for x in args {
        a.push(*x);
    }
    Ok(Value::from_i32(a.len as i32))
});
native!(n_pop, |vm, this, _args| {
    let a = this_array(vm, this)?;
    if a.len == 0 {
        return Ok(Value::UNDEFINED);
    }
    let last = a.data.last().copied().unwrap_or(Value::EMPTY);
    a.data.pop();
    a.len = a.data.len() as u32;
    if last.is_empty() {
        Ok(Value::UNDEFINED)
    } else {
        Ok(last)
    }
});
native!(n_shift, |vm, this, _args| {
    let a = this_array(vm, this)?;
    if a.data.is_empty() {
        return Ok(Value::UNDEFINED);
    }
    let first = a.data.remove(0);
    a.len = a.data.len() as u32;
    if first.is_empty() {
        Ok(Value::UNDEFINED)
    } else {
        Ok(first)
    }
});
native!(n_unshift, |vm, this, args| {
    let a = this_array(vm, this)?;
    let mut newv = Vec::with_capacity(a.data.len() + args.len());
    for x in args {
        newv.push(*x);
    }
    newv.extend(a.data.iter().copied());
    a.data = newv;
    a.len = a.data.len() as u32;
    Ok(Value::from_i32(a.len as i32))
});
fn norm_index(len: usize, i: i64) -> usize {
    if i < 0 {
        let v = len as i64 + i;
        if v < 0 {
            0
        } else {
            v as usize
        }
    } else if i as u128 > len as u128 {
        len
    } else {
        i as usize
    }
}
native!(n_slice, |vm, this, args| {
    let a = this_array(vm, this)?;
    let len = a.len as usize;
    let out = vm.new_array();
    let start = norm_index(len, arg(args, 0).as_i64());
    let end = if args.len() > 1 && !arg(args, 1).is_undefined() {
        norm_index(len, arg(args, 1).as_i64())
    } else {
        len
    };
    for i in start..end {
        let v = a.get(i as u32);
        unsafe { out.as_array_mut().push(v) };
    }
    Ok(out)
});
native!(n_splice, |vm, this, args| {
    let a = this_array(vm, this)?;
    let len = a.len as usize;
    let start = norm_index(len, arg(args, 0).as_i64());
    let del = if args.len() > 1 && !arg(args, 1).is_undefined() {
        (arg(args, 1).as_i64().max(0) as usize).min(len.saturating_sub(start))
    } else {
        len.saturating_sub(start)
    };
    let removed = vm.new_array();
    let mut i = 0;
    while i < del {
        let v = a.data.remove(start);
        unsafe { removed.as_array_mut().push(v) };
        i += 1;
    }
    let ins: Vec<Value> = args.iter().skip(2).copied().collect();
    for (j, v) in ins.iter().enumerate() {
        a.data.insert(start + j, *v);
    }
    a.len = a.data.len() as u32;
    Ok(removed)
});
native!(n_concat, |vm, this, args| {
    let a = this_array(vm, this)?;
    let out = vm.new_array();
    for i in 0..a.len {
        unsafe { out.as_array_mut().push(a.get(i)) };
    }
    for x in args {
        if x.is_array() {
            let av = unsafe { x.as_array() };
            for i in 0..av.len {
                unsafe { out.as_array_mut().push(av.get(i)) };
            }
        } else {
            unsafe { out.as_array_mut().push(*x) };
        }
    }
    Ok(out)
});
native!(n_array_entries, |vm, this, _args| {
    let a = this_array(vm, this)?;
    let mut out = Vec::with_capacity(a.data.len());
    for (i, v) in a.data.iter().enumerate() {
        let pair = vm.new_array_with(vec![Value::from_i32(i as i32), *v]);
        out.push(pair);
    }
    Ok(vm.new_array_with(out))
});

native!(n_array_keys, |vm, this, _args| {
    let a = this_array(vm, this)?;
    let out: Vec<Value> = (0..a.data.len())
        .map(|i| Value::from_i32(i as i32))
        .collect();
    Ok(vm.new_array_with(out))
});

native!(n_array_values, |vm, this, _args| {
    let a = this_array(vm, this)?;
    Ok(vm.new_array_with(a.data.clone()))
});

native!(n_join, |vm, this, args| {
    let a = this_array(vm, this)?;
    let sep = if args.is_empty() {
        ",".to_string()
    } else if arg(args, 0).is_undefined() {
        ",".to_string()
    } else {
        to_str(vm, arg(args, 0))?
    };
    let mut parts = Vec::with_capacity(a.len as usize);
    for i in 0..a.len {
        let v = a.get(i);
        if v.is_nil() || v.is_undefined() {
            parts.push(String::new());
        } else {
            parts.push(vm.to_string(v)?);
        }
    }
    Ok(vm.new_string(&parts.join(&sep)))
});
native!(n_array_to_string, |vm, this, _args| {
    let jk = key(vm, "join");
    let j = vm.get_prop(this, jk)?;
    if j.is_function() {
        return vm.call_callback(j, this, &[]);
    }
    Ok(vm.new_string(""))
});
native!(n_index_of, |vm, this, args| {
    let a = this_array(vm, this)?;
    let needle = arg(args, 0);
    let from = if args.len() > 1 {
        norm_index(a.len as usize, arg(args, 1).as_i64())
    } else {
        0
    };
    for i in from..a.len as usize {
        if vm.strict_eq(a.get(i as u32), needle)? {
            return Ok(Value::from_i32(i as i32));
        }
    }
    Ok(Value::from_i32(-1))
});
native!(n_last_index_of, |vm, this, args| {
    let a = this_array(vm, this)?;
    let needle = arg(args, 0);
    let mut i = a.len as i64 - 1;
    while i >= 0 {
        if vm.strict_eq(a.get(i as u32), needle)? {
            return Ok(Value::from_i32(i as i32));
        }
        i -= 1;
    }
    Ok(Value::from_i32(-1))
});
native!(n_includes, |vm, this, args| {
    let a = this_array(vm, this)?;
    let needle = arg(args, 0);
    for i in 0..a.len {
        let v = a.get(i);
        if vm.strict_eq(v, needle)? {
            return Ok(Value::TRUE);
        }
        if v.is_nan() && needle.is_nan() {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_find, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let len = a.len;
    for i in 0..len {
        let v = unsafe { this.as_array() }.get(i);
        let idx = Value::from_i32(i as i32);
        if vm
            .call_callback(f, Value::UNDEFINED, &[v, idx, this])?
            .to_bool()
        {
            return Ok(v);
        }
    }
    Ok(Value::UNDEFINED)
});
native!(n_find_last, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let mut i = a.len as i64 - 1;
    while i >= 0 {
        let v = unsafe { this.as_array() }.get(i as u32);
        let idx = Value::from_i32(i as i32);
        if vm
            .call_callback(f, Value::UNDEFINED, &[v, idx, this])?
            .to_bool()
        {
            return Ok(v);
        }
        i -= 1;
    }
    Ok(Value::UNDEFINED)
});
native!(n_find_index, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let len = a.len;
    for i in 0..len {
        let v = unsafe { this.as_array() }.get(i);
        let idx = Value::from_i32(i as i32);
        if vm
            .call_callback(f, Value::UNDEFINED, &[v, idx, this])?
            .to_bool()
        {
            return Ok(idx);
        }
    }
    Ok(Value::from_i32(-1))
});
native!(n_filter, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    if vm.callback_arith_kind(f) == 40 {
        if let Some(n) = vm.callback_rhs(f) {
            let m = Value::from_f64(n);
            if m.is_smi() && m.as_i64() != 0 {
                let k = m.as_i64();
                let mut data = Vec::with_capacity(src.len() / 2 + 1);
                for (i, v) in src.iter().enumerate() {
                    if v.is_empty() {
                        continue;
                    }
                    let keep = if v.is_smi() {
                        v.as_i64() % k == 0
                    } else {
                        let idx = Value::from_i32(i as i32);
                        vm.call_callback(f, Value::UNDEFINED, &[*v, idx, this])?
                            .to_bool()
                    };
                    if keep {
                        data.push(*v);
                    }
                }
                let out = vm.new_array();
                let arr = unsafe { out.as_array_mut() };
                arr.len = data.len() as u32;
                arr.data = data;
                return Ok(out);
            }
        }
    }
    let out = vm.new_array();
    for (i, v) in src.iter().enumerate() {
        if v.is_empty() {
            continue;
        }
        let idx = Value::from_i32(i as i32);
        if vm
            .call_callback(f, Value::UNDEFINED, &[*v, idx, this])?
            .to_bool()
        {
            unsafe { out.as_array_mut().push(*v) };
        }
    }
    Ok(out)
});
native!(n_map, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    let kind = vm.callback_arith_kind(f);
    if (32..37).contains(&kind) {
        if let Some(n) = vm.callback_rhs(f) {
            let y = Value::from_f64(n);
            let yi = if y.is_smi() { Some(y.as_i64()) } else { None };
            let mut data = Vec::with_capacity(src.len());
            for v in &src {
                // Holes survive the arithmetic fast path too, otherwise a hole
                // became `NaN` and `join` printed "NaN" where JS prints nothing.
                if v.is_empty() {
                    data.push(Value::EMPTY);
                    continue;
                }
                let fast = if let (Some(k), true) = (yi, v.is_smi()) {
                    let x = v.as_i64();
                    match kind - 32 {
                        0 => x.checked_add(k),
                        1 => x.checked_sub(k),
                        2 => x.checked_mul(k),
                        4 if k != 0 => Some(x % k),
                        _ => None,
                    }
                    .map(Value::from_i64)
                } else {
                    None
                };
                let r = match fast {
                    Some(r) => r,
                    None => {
                        let op = match kind - 32 {
                            0 => ArithOp::Add,
                            1 => ArithOp::Sub,
                            2 => ArithOp::Mul,
                            3 => ArithOp::Div,
                            _ => ArithOp::Mod,
                        };
                        vm.arith(op, *v, y)?
                    }
                };
                data.push(r);
            }
            let out = vm.new_array();
            let arr = unsafe { out.as_array_mut() };
            arr.len = data.len() as u32;
            arr.data = data;
            return Ok(out);
        }
    }
    let out = vm.new_array();
    for (i, v) in src.iter().enumerate() {
        // A hole stays a hole: the callback is not called for it.
        if v.is_empty() {
            unsafe { out.as_array_mut().push(Value::EMPTY) };
            continue;
        }
        let idx = Value::from_i32(i as i32);
        let r = vm.call_callback(f, Value::UNDEFINED, &[*v, idx, this])?;
        unsafe { out.as_array_mut().push(r) };
    }
    Ok(out)
});
native!(n_flat_map, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    let out = vm.new_array();
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        let r = vm.call_callback(f, Value::UNDEFINED, &[*v, idx, this])?;
        if r.is_array() {
            let ra = unsafe { r.as_array() };
            for j in 0..ra.len {
                unsafe { out.as_array_mut().push(ra.get(j)) };
            }
        } else {
            unsafe { out.as_array_mut().push(r) };
        }
    }
    Ok(out)
});
native!(n_for_each, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    for (i, v) in src.iter().enumerate() {
        if v.is_empty() {
            continue;
        }
        let idx = Value::from_i32(i as i32);
        vm.call_callback(f, Value::UNDEFINED, &[*v, idx, this])?;
    }
    Ok(Value::UNDEFINED)
});
native!(n_reduce, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    let mut acc;
    let mut start = 0usize;
    if args.len() > 1 {
        acc = arg(args, 1);
    } else {
        // The initial accumulator is the first *present* element; holes are
        // skipped, exactly like the callback is.
        match src.iter().position(|v| !v.is_empty()) {
            Some(p) => {
                acc = src[p];
                start = p + 1;
            }
            None => return Err(type_error("Reduce of empty array with no initial value")),
        }
    }
    for i in start..src.len() {
        if src[i].is_empty() {
            continue;
        }
        let idx = Value::from_i32(i as i32);
        acc = vm.call_callback(f, Value::UNDEFINED, &[acc, src[i], idx, this])?;
    }
    Ok(acc)
});
native!(n_reduce_right, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    let mut acc;
    let mut start: i64 = src.len() as i64 - 1;
    if args.len() > 1 {
        acc = arg(args, 1);
    } else {
        if src.is_empty() {
            return Err(type_error("Reduce of empty array with no initial value"));
        }
        acc = src[start as usize];
        start -= 1;
    }
    let mut i = start;
    while i >= 0 {
        let idx = Value::from_i32(i as i32);
        acc = vm.call_callback(f, Value::UNDEFINED, &[acc, src[i as usize], idx, this])?;
        i -= 1;
    }
    Ok(acc)
});
native!(n_some, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        if vm
            .call_callback(f, Value::UNDEFINED, &[*v, idx, this])?
            .to_bool()
        {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_every, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    vm.pin_temporaries(&src);
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        if !vm
            .call_callback(f, Value::UNDEFINED, &[*v, idx, this])?
            .to_bool()
        {
            return Ok(Value::from_bool(false));
        }
    }
    Ok(Value::TRUE)
});
native!(n_reverse, |vm, this, _args| {
    let a = this_array(vm, this)?;
    a.data.reverse();
    Ok(this)
});
native!(n_fill, |vm, this, args| {
    let a = this_array(vm, this)?;
    let len = a.len as usize;
    let v = arg(args, 0);
    let start = norm_index(len, arg(args, 1).as_i64());
    let end = if args.len() > 2 && !arg(args, 2).is_undefined() {
        norm_index(len, arg(args, 2).as_i64())
    } else {
        len
    };
    for i in start..end {
        a.data[i] = v;
    }
    Ok(this)
});
native!(n_flat, |vm, this, args| {
    let a = this_array(vm, this)?;
    let mut out: Vec<Value> = Vec::new();
    let depth = if args.is_empty() {
        1
    } else {
        arg(args, 0).as_i64().max(0) as usize
    };
    fn go(v: Value, out: &mut Vec<Value>, depth: usize) {
        if depth > 0 && v.is_array() {
            let a = unsafe { v.as_array() };
            for i in 0..a.len {
                go(a.get(i), out, depth - 1);
            }
        } else {
            out.push(v);
        }
    }
    let data = a.data.clone();
    for v in data {
        go(v, &mut out, depth);
    }
    let res = vm.new_array();
    for v in out {
        unsafe { res.as_array_mut().push(v) };
    }
    Ok(res)
});
native!(n_at, |vm, this, args| {
    let a = this_array(vm, this)?;
    let len = a.len as i64;
    let i = arg(args, 0).as_i64();
    let idx = if i < 0 { len + i } else { i };
    if idx < 0 || idx >= len {
        return Ok(Value::UNDEFINED);
    }
    Ok(a.get(idx as u32))
});
native!(n_sort, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let mut data = a.data.clone();
    vm.pin_temporaries(&data);
    if f.is_function() {
        // Bottom-up merge sort: O(n log n) comparisons against a callback that
        // can run arbitrary JS, and stable, which is what `Array#sort` promises.
        // An insertion sort here made a 100k-element sort take minutes.
        let n = data.len();
        let mut buf = data.clone();
        let smi_sub = vm.callback_arith_kind(f) == 1;
        let mut width = 1usize;
        while width < n {
            let mut lo = 0usize;
            while lo < n {
                let mid = (lo + width).min(n);
                let hi = (lo + 2 * width).min(n);
                if mid < hi {
                    merge_run(vm, &mut data, &mut buf, lo, mid, hi, f, smi_sub)?;
                }
                lo += 2 * width;
            }
            width *= 2;
        }
    } else {
        let mut strs: Vec<(String, Value)> = data
            .iter()
            .map(|v| vm.to_string(*v).map(|text| (text, *v)))
            .collect::<VmResult<_>>()?;
        strs.sort_by(|x, y| x.0.cmp(&y.0));
        data = strs.into_iter().map(|(_, v)| v).collect();
    }
    unsafe { this.as_array_mut() }.data = data;
    Ok(this)
});

/// Merge `data[lo..hi]` (split at `mid`) into `buf` and copy it back, comparing
/// with the JS `comparefn`. A JS comparator can re-enter the engine, so the
/// result is copied out per comparison rather than held across the call.
fn merge_run(
    vm: &mut Vm,
    data: &mut [Value],
    buf: &mut [Value],
    lo: usize,
    mid: usize,
    hi: usize,
    f: Value,
    smi_sub: bool,
) -> VmResult<()> {
    let mut i = lo;
    let mut j = mid;
    let mut k = lo;
    while i < mid && j < hi {
        let a = data[i];
        let b = data[j];
        if smi_sub && a.is_smi() && b.is_smi() {
            if a.as_i64() > b.as_i64() {
                buf[k] = b;
                j += 1;
            } else {
                buf[k] = a;
                i += 1;
            }
            k += 1;
            continue;
        }
        let r = vm.call_callback(f, Value::UNDEFINED, &[a, b])?;
        // Numeric comparators return Smis and doubles; extracting those
        // inline skips a VmResult round-trip per comparison. Anything else
        // (strings, objects with valueOf) still goes through full `to_number`.
        let d = if r.is_smi() {
            r.as_i64() as f64
        } else if r.is_double() {
            r.as_f64()
        } else {
            vm.to_number(r)?
        };
        if d > 0.0 {
            buf[k] = b;
            j += 1;
        } else {
            buf[k] = a;
            i += 1;
        }
        k += 1;
    }
    while i < mid {
        buf[k] = data[i];
        i += 1;
        k += 1;
    }
    while j < hi {
        buf[k] = data[j];
        j += 1;
        k += 1;
    }
    data[lo..hi].copy_from_slice(&buf[lo..hi]);
    Ok(())
}
native!(n_iter_make, |vm, this, _args| {
    let it = vm.new_object();
    let inner = vm.new_array();
    if this.is_array() {
        let a = unsafe { this.as_array() };
        for i in 0..a.len {
            unsafe { inner.as_array_mut().push(a.get(i)) };
        }
    } else if this.is_string() {
        let s = unsafe { this.as_string().as_str().to_string() };
        let chars: Vec<String> = s.chars().map(|c| c.to_string()).collect();
        for c in chars {
            let v = vm.new_string(&c);
            unsafe { inner.as_array_mut().push(v) };
        }
    }
    let k = key(vm, "#src");
    vm.set_prop(it, k, inner).unwrap();
    let k = key(vm, "#i");
    vm.set_prop(it, k, Value::from_i32(0)).unwrap();
    let nf = vm.new_function_native("next", n_iter_next, 0);
    let k = key(vm, "next");
    vm.set_prop(it, k, nf).unwrap();
    Ok(it)
});
native!(n_iter_next, |vm, this, _args| {
    let __k = key(vm, "#src");
    let src = vm.get_prop(this, __k)?;
    let __k = key(vm, "#i");
    let i = vm.get_prop(this, __k)?;
    let idx = i.as_i64();
    let res = vm.new_object();
    let a = unsafe { src.as_array() };
    if idx < a.len as i64 {
        let v = a.get(idx as u32);
        let k = key(vm, "#i");
        vm.set_prop(this, k, Value::from_i64(idx + 1)).unwrap();
        let k = key(vm, "value");
        vm.set_prop(res, k, v).unwrap();
        let k = key(vm, "done");
        vm.set_prop(res, k, Value::FALSE).unwrap();
    } else {
        let k = key(vm, "done");
        vm.set_prop(res, k, Value::TRUE).unwrap();
        let k = key(vm, "value");
        vm.set_prop(res, k, Value::UNDEFINED).unwrap();
    }
    Ok(res)
});

// ---------------------------------------------------------------------------
// String
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn slice_str(s: &str, start: usize, end: usize) -> String {
    // Byte-sliced like `length` and `s[i]`, clamped to char boundaries so the
    // result stays valid UTF-8. The old version materialised every character
    // into a Vec on each call.
    let len = s.len();
    let a = start.min(len);
    let b = end.min(len).max(a);
    let mut la = a;
    while la > 0 && !s.is_char_boundary(la) {
        la -= 1;
    }
    let mut lb = b;
    while lb < len && !s.is_char_boundary(lb) {
        lb += 1;
    }
    s[la..lb].to_string()
}

// `String.raw({ raw: [...] }, ...subs)` — the exact source text of a tagged
// template, joined with the substituted values.
native!(n_string_raw, |vm, _this, args| {
    let strings = arg(args, 0);
    if !(strings.is_object() || strings.is_array()) {
        return Err(type_error(
            "String.raw requires a template object with a `raw` property",
        ));
    }
    let rawk = key(vm, "raw");
    let raw = vm.get_prop(strings, rawk)?;
    let src = if raw.is_undefined() { strings } else { raw };
    let lenk = key(vm, "length");
    let len_v = vm.get_prop(src, lenk)?;
    let len = len_v.as_i64().max(0) as usize;
    let mut out = String::new();
    for i in 0..len {
        let chunk = vm.get_prop(src, PropKey::Idx(i as u32))?;
        if !chunk.is_undefined() {
            out.push_str(&to_str(vm, chunk)?);
        }
        if i + 1 < len && i < args.len() - 1 {
            out.push_str(&to_str(vm, args[i + 1])?);
        }
    }
    Ok(vm.new_string(&out))
});

// `__jhol_tagged_template(strings, raws)` — builds the template object a
// tagged template receives: the cooked strings array with `.raw` attached
// (the whole object frozen, as the specification requires).
native!(n_h_tagged_template, |vm, _this, args| {
    // The cooked strings stay a real array (so `strings.join(...)` and every
    // array method work); `raw` is a non-index own property on it, and the
    // array is frozen as the specification requires.
    let strings = arg(args, 0);
    let raws = arg(args, 1);
    if strings.is_array() {
        let rk = key(vm, "raw");
        vm.set_prop(strings, rk, raws)?;
        unsafe { strings.as_array_mut() }.extensible = false;
    }
    Ok(strings)
});

native!(n_string_ctor, |vm, _this, args| {
    let v = arg(args, 0);
    if v.is_string() {
        return Ok(v);
    }
    // `String()` with no argument is "", but `String(undefined)` and `String(null)`
    // are "undefined"/"null". Treating a present nil argument as "no argument" made
    // all three return "" — found by a symbol test that used String(o[k]) to probe a
    // missing key.
    if args.is_empty() {
        return Ok(vm.new_string(""));
    }
    // The one conversion a symbol allows. `vm.to_string` throws for symbols, which is
    // what makes `${sym}` and `sym + ""` errors while `String(sym)` works.
    if let Some(id) = v.as_symbol() {
        let text = match symbol_description(id) {
            Some(d) => format!("Symbol({})", d),
            None => "Symbol()".to_string(),
        };
        return Ok(vm.new_string(&text));
    }
    let __s = vm.to_string(v)?;
    Ok(vm.new_string(&__s))
});
native!(n_from_char_code, |vm, _this, args| {
    let units = args
        .iter()
        .map(|&v| vm.to_uint32(v).map(|n| n as u16))
        .collect::<VmResult<Vec<_>>>()?;
    Ok(vm.new_string_utf16(&units))
});
native!(n_from_code_point, |vm, _this, args| {
    let mut units = Vec::new();
    for &v in args {
        let n = vm.to_number(v)?;
        if !n.is_finite() || n.fract() != 0.0 || !(0.0..=0x10ffff as f64).contains(&n) {
            return Err(crate::errors::range_error("Invalid code point"));
        }
        if n <= 65535.0 {
            units.push(n as u16);
        } else {
            let n = n as u32 - 65536;
            units.push(0xd800 + (n >> 10) as u16);
            units.push(0xdc00 + (n & 1023) as u16);
        }
    }
    Ok(vm.new_string_utf16(&units))
});
native!(n_char_at, |vm, this, args| {
    let i = string_index(vm, arg(args, 0))?;
    let units = string_units(vm, this)?;
    Ok(vm.new_string_utf16(if i < 0 {
        &[]
    } else {
        units.get(i as usize..i as usize + 1).unwrap_or(&[])
    }))
});
native!(n_str_at, |vm, this, args| {
    let units = string_units(vm, this)?;
    let i = string_index(vm, arg(args, 0))?;
    let i = if i < 0 { units.len() as i64 + i } else { i };
    if i < 0 || i >= units.len() as i64 {
        return Ok(Value::UNDEFINED);
    }
    Ok(vm.new_string_utf16(&units[i as usize..i as usize + 1]))
});
native!(n_char_code_at, |vm, this, args| {
    let i = string_index(vm, arg(args, 0))?;
    let unit = if i < 0 {
        None
    } else if this.is_string() {
        unsafe { this.as_string() }.code_unit(i as usize)
    } else {
        string_units(vm, this)?.get(i as usize).copied()
    };
    Ok(unit.map_or(Value::NAN, |u| Value::from_i32(u as i32)))
});
native!(n_code_point_at, |vm, this, args| {
    let i = string_index(vm, arg(args, 0))?;
    let units = string_units(vm, this)?;
    let Some(&first) = (if i < 0 { None } else { units.get(i as usize) }) else {
        return Ok(Value::UNDEFINED);
    };
    let second = units.get(i as usize + 1).copied().unwrap_or(0);
    let cp = if (0xd800..=0xdbff).contains(&first) && (0xdc00..=0xdfff).contains(&second) {
        0x10000 + ((first as u32 - 0xd800) << 10) + second as u32 - 0xdc00
    } else {
        first as u32
    };
    Ok(Value::from_i32(cp as i32))
});
fn find_sub(hay: &str, needle: &str, from: usize) -> i64 {
    // Byte search. The old version built a `Vec<char>` plus one `String` per
    // position tried, which made `indexOf` quadratic with an allocation per
    // attempt.
    if needle.is_empty() {
        return from.min(hay.len()) as i64;
    }
    let start = from.min(hay.len());
    let rest = &hay.as_bytes()[start..];
    match rest
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())
    {
        Some(off) => (start + off) as i64,
        None => -1,
    }
}
fn rfind_sub(hay: &str, needle: &str) -> i64 {
    // Allocation-free backward scan; see `find_sub`.
    if needle.is_empty() {
        return hay.len() as i64;
    }
    let bytes = hay.as_bytes();
    if needle.len() > bytes.len() {
        return -1;
    }
    for i in (0..=bytes.len() - needle.len()).rev() {
        if &bytes[i..i + needle.len()] == needle.as_bytes() {
            return i as i64;
        }
    }
    -1
}
native!(n_str_index_of, |vm, this, args| {
    let s = this_str(vm, this)?;
    let n = to_str(vm, arg(args, 0))?;
    let from = if args.len() > 1 {
        arg(args, 1).as_i64().max(0) as usize
    } else {
        0
    };
    Ok(Value::from_i64(find_sub(&s, &n, from)))
});
native!(n_str_last_index_of, |vm, this, args| {
    let s = this_str(vm, this)?;
    let n = to_str(vm, arg(args, 0))?;
    Ok(Value::from_i64(rfind_sub(&s, &n)))
});
native!(n_str_includes, |vm, this, args| {
    let s = this_str(vm, this)?;
    let n = to_str(vm, arg(args, 0))?;
    Ok(Value::from_bool(find_sub(&s, &n, 0) >= 0))
});
native!(n_starts_with, |vm, this, args| {
    let s = this_str(vm, this)?;
    let n = to_str(vm, arg(args, 0))?;
    let pos = if args.len() > 1 {
        arg(args, 1).as_i64().max(0) as usize
    } else {
        0
    };
    let chars: Vec<char> = s.chars().collect();
    let nc = n.chars().count();
    if pos > chars.len() {
        return Ok(Value::from_bool(false));
    }
    let end = (pos + nc).min(chars.len());
    let w: String = chars[pos..end].iter().collect();
    Ok(Value::from_bool(w == n))
});
native!(n_ends_with, |vm, this, args| {
    let s = this_str(vm, this)?;
    let n = to_str(vm, arg(args, 0))?;
    let chars: Vec<char> = s.chars().collect();
    let nc = n.chars().count();
    if nc > chars.len() {
        return Ok(Value::from_bool(false));
    }
    let w: String = chars[chars.len() - nc..].iter().collect();
    Ok(Value::from_bool(w == n))
});
native!(n_str_slice, |vm, this, args| {
    let s = string_units(vm, this)?;
    let n = s.len() as i64;
    let a = arg(args, 0).as_i64();
    let b = if args.len() > 1 && !arg(args, 1).is_undefined() {
        arg(args, 1).as_i64()
    } else {
        n
    };
    let start = if a < 0 { (n + a).max(0) } else { a.min(n) };
    let end = if b < 0 { (n + b).max(0) } else { b.min(n) };
    if end <= start {
        return Ok(vm.new_string(""));
    }
    Ok(vm.new_string_utf16(&s[start as usize..end as usize]))
});
native!(n_substring, |vm, this, args| {
    let s = string_units(vm, this)?;
    let n = s.len() as i64;
    let mut a = arg(args, 0).as_i64().min(n).max(0);
    let mut b = if args.len() > 1 && !arg(args, 1).is_undefined() {
        arg(args, 1).as_i64().min(n).max(0)
    } else {
        n
    };
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    Ok(vm.new_string_utf16(&s[a as usize..b as usize]))
});
native!(n_substr, |vm, this, args| {
    let s = string_units(vm, this)?;
    let n = s.len() as i64;
    let start = arg(args, 0).as_i64();
    let len = if args.len() > 1 && !arg(args, 1).is_undefined() {
        arg(args, 1).as_i64()
    } else {
        n
    };
    let st = if start < 0 {
        (n + start).max(0)
    } else {
        start.min(n)
    };
    let en = if len < 0 { st } else { (st + len).min(n) };
    if en <= st {
        return Ok(vm.new_string(""));
    }
    Ok(vm.new_string_utf16(&s[st as usize..en as usize]))
});
native!(n_to_upper, |vm, this, _args| {
    let s = this_str(vm, this)?;
    Ok(vm.new_string(&s.to_uppercase()))
});
native!(n_to_lower, |vm, this, _args| {
    let s = this_str(vm, this)?;
    Ok(vm.new_string(&s.to_lowercase()))
});
native!(n_trim, |vm, this, _args| {
    let s = this_str(vm, this)?;
    Ok(vm.new_string(s.trim()))
});
native!(n_str_normalize, |_vm, this, _args| Ok(this));
native!(n_locale_compare, |vm, this, args| {
    let a = this_str(vm, this)?;
    let b = to_str(vm, arg(args, 0))?;
    let r = match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    Ok(Value::from_i32(r))
});
native!(n_trim_start, |vm, this, _args| {
    let s = this_str(vm, this)?;
    Ok(vm.new_string(s.trim_start()))
});
native!(n_trim_end, |vm, this, _args| {
    let s = this_str(vm, this)?;
    Ok(vm.new_string(s.trim_end()))
});
native!(n_concat_str, |vm, this, args| {
    let mut s = this_str(vm, this)?;
    for a in args {
        s.push_str(&to_str(vm, *a)?);
    }
    Ok(vm.new_string(&s))
});
native!(n_str_to_string, |vm, this, _args| {
    if this.is_string() {
        Ok(this)
    } else {
        Ok(vm.new_string(&js_str("")))
    }
});
native!(n_split, |vm, this, args| {
    let s = this_str(vm, this)?;
    let out = vm.new_array();
    if args.is_empty() || arg(args, 0).is_undefined() {
        unsafe { out.as_array_mut().push(vm.new_string(&s)) };
        return Ok(out);
    }
    // Regex separator: split on every match, keeping capture groups inline.
    if let Some((src, flags)) = as_regexp(vm, arg(args, 0)) {
        let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
        let lim = if args.len() > 1 && !arg(args, 1).is_undefined() {
            arg(args, 1).as_i64().max(0) as usize
        } else {
            usize::MAX
        };
        let mut push = |vm: &mut Vm, part: &str| {
            let cur = (unsafe { out.as_array() }).len as usize;
            if cur >= lim {
                return;
            }
            unsafe { out.as_array_mut().push(vm.new_string(part)) };
        };
        let mut pos = 0usize;
        let mut last = 0usize;
        while let Some(m) = re.exec(&s, pos) {
            push(vm, &s[last..m.start]);
            for g in m.caps.iter().skip(1) {
                match g {
                    Some((a, b)) => push(vm, &s[*a..*b]),
                    None => push(vm, ""),
                }
            }
            pos = if m.end > m.start { m.end } else { m.end + 1 };
            last = m.end;
            if pos > s.len() {
                break;
            }
        }
        let cur = (unsafe { out.as_array() }).len as usize;
        if cur < lim {
            push(vm, &s[last.min(s.len())..]);
        }
        return Ok(out);
    }
    let sep = to_str(vm, arg(args, 0))?;
    if sep.is_empty() {
        for c in s.chars() {
            unsafe { out.as_array_mut().push(vm.new_string(&c.to_string())) };
        }
        return Ok(out);
    }
    for part in s.split(&sep as &str) {
        unsafe { out.as_array_mut().push(vm.new_string(part)) };
    }
    if args.len() > 1 && !arg(args, 1).is_undefined() {
        let lim = arg(args, 1).as_i64().max(0) as usize;
        let a = unsafe { out.as_array_mut() };
        a.data.truncate(lim);
        a.len = a.data.len() as u32;
    }
    Ok(out)
});
fn replace_all_str(hay: &str, needle: &str, rep: &str) -> String {
    if needle.is_empty() {
        return hay.to_string();
    }
    hay.replace(needle, rep)
}
/// Expand `$$`, `$&`, ``$```, `$'` and `$1`..`$9` in a replacement string,
/// which is what both `replace` and `replaceAll` accept.
fn expand_replacement(
    rep: &str,
    hay: &str,
    whole: (usize, usize),
    caps: &[Option<(usize, usize)>],
) -> String {
    let mut out = String::with_capacity(rep.len());
    let b: Vec<char> = rep.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '$' && i + 1 < b.len() {
            match b[i + 1] {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    out.push_str(&hay[whole.0..whole.1]);
                    i += 2;
                    continue;
                }
                '`' => {
                    out.push_str(&hay[..whole.0]);
                    i += 2;
                    continue;
                }
                '\'' => {
                    out.push_str(&hay[whole.1..]);
                    i += 2;
                    continue;
                }
                d @ '1'..='9' => {
                    let n = d.to_digit(10).unwrap() as usize;
                    if let Some(Some((s, e))) = caps.get(n) {
                        out.push_str(&hay[*s..*e]);
                    }
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Shared body of `replace`/`replaceAll` for a regex pattern.
fn replace_with_regex(
    vm: &mut Vm,
    s: &str,
    re: &crate::regex::Regex,
    rep: Value,
    all: bool,
) -> VmResult<Value> {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0usize;
    let mut any = false;
    loop {
        let Some(m) = re.exec(s, pos) else { break };
        any = true;
        out.push_str(&s[pos..m.start]);
        if rep.is_function() {
            let mut cb: Vec<Value> = m
                .caps
                .iter()
                .map(|c| match c {
                    Some((a, b)) => vm.new_string(&s[*a..*b]),
                    None => Value::UNDEFINED,
                })
                .collect();
            cb.push(Value::from_i64(m.start as i64));
            cb.push(vm.new_string(s));
            let r = vm.call_callback(rep, Value::UNDEFINED, &cb)?;
            out.push_str(&to_str(vm, r)?);
        } else {
            let r = to_str(vm, rep)?;
            out.push_str(&expand_replacement(&r, s, (m.start, m.end), &m.caps));
        }
        let advance = if m.end > m.start { m.end } else { m.end + 1 };
        if advance > s.len() {
            break;
        }
        out.push_str(&s[m.end..advance.min(s.len())]);
        pos = advance;
        // `replace` walks every match when the pattern is global; `replaceAll`
        // walks every match regardless. Only a non-global `replace` stops here.
        if !(all || re.flags.global) {
            out.push_str(&s[pos.min(s.len())..]);
            pos = s.len();
            break;
        }
    }
    if !any {
        return Ok(vm.new_string(s));
    }
    if pos < s.len() {
        out.push_str(&s[pos..]);
    }
    Ok(vm.new_string(&out))
}

native!(n_replace, |vm, this, args| {
    let s = this_str(vm, this)?;
    if let Some((src, flags)) = as_regexp(vm, arg(args, 0)) {
        let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
        return replace_with_regex(vm, &s, &re, arg(args, 1), false);
    }
    let pat = to_str(vm, arg(args, 0))?;
    let rep = arg(args, 1);
    // `replace` swaps only the *first* match (unlike `replaceAll`).
    let r = if rep.is_function() {
        let pv = vm.new_string(&pat);
        let sub = vm.call_callback(rep, Value::UNDEFINED, &[pv])?;
        let rep_s = to_str(vm, sub)?;
        s.replacen(&pat, &rep_s, 1)
    } else {
        // A string pattern still expands `$$`, `$&`, ``$` `` and `$'`.
        let rep_s = to_str(vm, rep)?;
        match s.find(&pat) {
            Some(at) => {
                let whole = (at, at + pat.len());
                let expanded = expand_replacement(&rep_s, &s, whole, &[]);
                let mut out = String::with_capacity(s.len() + expanded.len());
                out.push_str(&s[..at]);
                out.push_str(&expanded);
                out.push_str(&s[whole.1..]);
                out
            }
            None => s.clone(),
        }
    };
    Ok(vm.new_string(&r))
});

native!(n_replace_all, |vm, this, args| {
    let s = this_str(vm, this)?;
    if let Some((src, flags)) = as_regexp(vm, arg(args, 0)) {
        let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
        return replace_with_regex(vm, &s, &re, arg(args, 1), true);
    }
    let pat = to_str(vm, arg(args, 0))?;
    let rep = to_str(vm, arg(args, 1))?;
    Ok(vm.new_string(&replace_all_str(&s, &pat, &rep)))
});

native!(n_search, |vm, this, args| {
    let s = this_str(vm, this)?;
    let (src, flags) = match as_regexp(vm, arg(args, 0)) {
        Some(p) => p,
        None => (to_str(vm, arg(args, 0))?, String::new()),
    };
    let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
    Ok(Value::from_i64(match re.exec(&s, 0) {
        Some(m) => m.start as i64,
        None => -1,
    }))
});

native!(n_match, |vm, this, args| {
    let s = this_str(vm, this)?;
    let (src, flags) = match as_regexp(vm, arg(args, 0)) {
        Some(p) => p,
        None => (to_str(vm, arg(args, 0))?, String::new()),
    };
    // `match` with a non-global regex behaves like `exec`.
    let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
    if !re.flags.global {
        let Some(m) = re.exec(&s, 0) else {
            return Ok(Value::NULL);
        };
        let mut items = Vec::with_capacity(m.caps.len());
        for c in &m.caps {
            items.push(match c {
                Some((a, b)) => vm.new_string(&s[*a..*b]),
                None => Value::UNDEFINED,
            });
        }
        let arr = vm.new_array_with(items);
        let ki = key(vm, "index");
        vm.set_prop(arr, ki, Value::from_i64(m.start as i64))?;
        let kin = key(vm, "input");
        let inp = vm.new_string(&s);
        vm.set_prop(arr, kin, inp)?;
        return Ok(arr);
    }
    let mut hits = Vec::new();
    let mut pos = 0usize;
    while let Some(m) = re.exec(&s, pos) {
        hits.push(vm.new_string(&s[m.start..m.end]));
        pos = if m.end > m.start { m.end } else { m.end + 1 };
        if pos > s.len() {
            break;
        }
    }
    if hits.is_empty() {
        return Ok(Value::NULL);
    }
    Ok(vm.new_array_with(hits))
});
native!(n_repeat, |vm, this, args| {
    let s = this_str(vm, this)?;
    let n = arg(args, 0).as_i64();
    if n < 0 {
        return Err(type_error("Invalid count value"));
    }
    Ok(vm.new_string(&s.repeat(n as usize)))
});
native!(n_pad_start, |vm, this, args| {
    let s = this_str(vm, this)?;
    let target = arg(args, 0).as_i64().max(0) as usize;
    let pad = if args.len() > 1 && !arg(args, 1).is_undefined() {
        to_str(vm, arg(args, 1))?
    } else {
        " ".to_string()
    };
    let len = s.chars().count();
    if len >= target || pad.is_empty() {
        return Ok(vm.new_string(&s));
    }
    let need = target - len;
    // Build the padding in one pass. The old loop counted `out.chars()` on
    // every iteration — an O(n) scan inside an O(n) loop, so padding 100k
    // characters took 0.45s and doubled quadratically.
    let prefix: String = pad.chars().cycle().take(need).collect();
    let mut out = String::with_capacity(prefix.len() + s.len());
    out.push_str(&prefix);
    out.push_str(&s);
    Ok(vm.new_string(&out))
});
native!(n_pad_end, |vm, this, args| {
    let s = this_str(vm, this)?;
    let target = arg(args, 0).as_i64().max(0) as usize;
    let pad = if args.len() > 1 && !arg(args, 1).is_undefined() {
        to_str(vm, arg(args, 1))?
    } else {
        " ".to_string()
    };
    let len = s.chars().count();
    if len >= target || pad.is_empty() {
        return Ok(vm.new_string(&s));
    }
    let need = target - len;
    let suffix: String = pad.chars().cycle().take(need).collect();
    let mut out = String::with_capacity(s.len() + suffix.len());
    out.push_str(&s);
    out.push_str(&suffix);
    Ok(vm.new_string(&out))
});

// ---------------------------------------------------------------------------
// Number / Math / JSON / misc
// ---------------------------------------------------------------------------

native!(n_number_ctor, |vm, _this, args| {
    // Number() with no argument is 0; Number(undefined) is NaN; only
    // Number(null) is 0. The old code collapsed all three to 0.
    let n = if args.is_empty() {
        0.0
    } else {
        let v = arg(args, 0);
        if v.is_undefined() {
            f64::NAN
        } else if v.is_null() {
            0.0
        } else {
            vm.to_number(v)?
        }
    };
    Ok(Value::from_f64(n))
});
native!(n_bool_ctor, |vm, _this, args| Ok(Value::from_bool(
    arg(args, 0).to_bool()
)));
native!(n_bool_to_string, |vm, this, _args| Ok(
    vm.new_string(if this.as_bool() { "true" } else { "false" })
));
native!(n_bool_value_of, |_vm, this, _args| Ok(this));
native!(n_number_value_of, |_vm, this, _args| Ok(Value::from_f64(
    this.as_f64()
)));
native!(n_number_is_integer, |vm, _this, args| {
    let v = arg(args, 0);
    Ok(Value::from_bool(
        v.is_number() && v.as_f64().fract() == 0.0 && v.as_f64().is_finite(),
    ))
});
native!(n_number_is_finite, |_vm, _this, args| {
    let v = arg(args, 0);
    Ok(Value::from_bool(v.is_number() && v.as_f64().is_finite()))
});
native!(n_number_is_nan, |_vm, _this, args| Ok(Value::from_bool(
    arg(args, 0).is_nan()
)));
native!(n_number_is_safe_integer, |_vm, _this, args| {
    let v = arg(args, 0);
    let n = v.as_f64();
    Ok(Value::from_bool(
        v.is_number() && n.fract() == 0.0 && n.abs() <= 9007199254740991.0,
    ))
});
native!(n_to_precision, |vm, this, args| {
    let x = this.as_f64();
    if x.is_nan() {
        return Ok(vm.new_string("NaN"));
    }
    if x.is_infinite() {
        return Ok(vm.new_string(if x > 0.0 { "Infinity" } else { "-Infinity" }));
    }
    let pv = arg(args, 0);
    if pv.is_undefined() {
        return Ok(vm.new_string(&f64_to_js_string(x)));
    }
    let p = pv.as_i64().clamp(1, 100) as usize;
    if x == 0.0 {
        let zeros = if p == 1 {
            "0".to_string()
        } else {
            format!("0.{}", "0".repeat(p - 1))
        };
        return Ok(vm.new_string(&zeros));
    }
    // Significant digits: exponential form when the exponent is below -6 or at
    // least the requested precision, decimal form otherwise.
    let exp = x.abs().log10().floor() as i32;
    if exp < -6 || exp >= p as i32 {
        let s = format!("{:.*e}", p - 1, x);
        let s = s.replace("e", "e+").replace("e+-", "e-");
        return Ok(vm.new_string(&s));
    }
    let decimals = (p as i32 - 1 - exp).max(0) as usize;
    Ok(vm.new_string(&format!("{:.*}", decimals, x)))
});

native!(n_to_fixed, |vm, this, args| {
    let d = arg(args, 0).as_i64().clamp(0, 100) as usize;
    let x = this.as_f64();
    if x.is_nan() {
        return Ok(vm.new_string("NaN"));
    }
    if x.is_infinite() {
        return Ok(vm.new_string(if x > 0.0 { "Infinity" } else { "-Infinity" }));
    }
    // `toFixed` rounds the value's *exact* decimal expansion, ties away from
    // zero. Scaling by 10^d and rounding the float instead made
    // `(2.55).toFixed(1)` print "2.6": 2.55 is really 2.54999…, which rounds
    // down. Format a long expansion and round that string.
    if x.abs() >= 1e21 {
        return Ok(vm.new_string(&f64_to_js_string(x)));
    }
    Ok(vm.new_string(&fixed_decimal(x, d)))
});
native!(n_number_to_string, |_vm, this, args| {
    let radix = arg(args, 0).as_i64();
    if radix == 10 || radix == 0 {
        let v = this.as_f64();
        // A single digit is by far the most common request: `i % 10` in a
        // build loop asks for exactly ten strings, forever. Interning turns
        // one arena allocation per call into a hash lookup, and an atom is an
        // ordinary immutable string Value — rooted in `atom_roots`, so the
        // collector keeps it and nothing observable changes.
        if v.fract() == 0.0 && (0.0..10.0).contains(&v) {
            // The ten results are interned once and then read from a plain
            // array: no format, no hash, no map probe. First use interns
            // through the ordinary path, so the atoms are rooted as usual.
            let d = v as usize;
            let cached = _vm.digit_atoms[d];
            if !cached.is_undefined() {
                return Ok(cached);
            }
            let mut buf = [b'0'; 1];
            buf[0] = b'0' + d as u8;
            if let Ok(s) = std::str::from_utf8(&buf) {
                let a = _vm.arena().intern_atom(s);
                _vm.digit_atoms[d] = a;
                return Ok(a);
            }
        }
        Ok(_vm.new_string(&f64_to_js_string(v)))
    } else {
        let x = this.as_f64();
        if !x.is_finite() {
            return Ok(_vm.new_string(&f64_to_js_string(x)));
        }
        let neg = x < 0.0;
        let ax = x.abs();
        let int = ax.trunc();
        let mut frac = ax - int;
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        out.push_str(&to_radix(int as i64, radix as u32));
        if frac > 0.0 {
            // Fractional digits by repeated multiplication; 52 bits of mantissa
            // bound how many can be meaningful.
            out.push('.');
            let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
            let mut count = 0;
            while frac > 0.0 && count < 20 {
                frac *= radix as f64;
                let d = frac.floor();
                out.push(digits[(d as usize).min(radix as usize - 1)] as char);
                frac -= d;
                count += 1;
            }
        }
        Ok(_vm.new_string(&out))
    }
});
/// Round `x` to `d` decimal places using its long decimal expansion, with ties
/// away from zero — the rule `toFixed` uses.
fn fixed_decimal(x: f64, d: usize) -> String {
    let neg = x < 0.0;
    let ax = x.abs();
    // 30 places is past the ~17 significant digits a double carries, so the
    // digits that matter are exact and the rest are zeros.
    let exact = format!("{:.30}", ax);
    let (int_part, frac_part) = match exact.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (exact.clone(), String::new()),
    };
    let mut digits: Vec<u8> = frac_part.bytes().map(|b| b - b'0').collect();
    if d < digits.len() {
        let round_up = {
            let at = digits[d];
            let rest_nonzero = digits[d + 1..].iter().any(|v| *v != 0);
            at > 5 || (at == 5 && (rest_nonzero || true)) && at >= 5
        };
        digits.truncate(d);
        if round_up {
            let mut i = d as isize - 1;
            let mut carry = true;
            while carry && i >= 0 {
                if digits[i as usize] + 1 >= 10 {
                    digits[i as usize] = 0;
                    i -= 1;
                } else {
                    digits[i as usize] += 1;
                    carry = false;
                }
            }
            if carry {
                // carry out of the fraction: bump the integer part
                let bumped = int_part.parse::<u128>().unwrap_or(0) + 1;
                let mut out = String::new();
                if neg {
                    out.push('-');
                }
                out.push_str(&bumped.to_string());
                if d > 0 {
                    out.push('.');
                    for _ in 0..d {
                        out.push('0');
                    }
                }
                return out;
            }
        }
    } else {
        digits.resize(d, 0);
    }
    let mut out = String::with_capacity(int_part.len() + d + 2);
    if neg {
        out.push('-');
    }
    out.push_str(&int_part);
    if d > 0 {
        out.push('.');
        for v in digits {
            out.push((b'0' + v) as char);
        }
    }
    out
}

fn to_radix(mut n: i64, radix: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    let neg = n < 0;
    n = n.abs();
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(digits[(n % radix as i64) as usize]);
        n /= radix as i64;
    }
    if neg {
        out.push(b'-');
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}
native!(n_parse_int, |vm, _this, args| {
    let s = to_str(vm, arg(args, 0))?.trim().to_string();
    let radix = arg(args, 1).as_i64() as u32;
    let parsed = parse_int_like(&s, radix);
    Ok(Value::from_f64(parsed))
});
native!(n_parse_float, |vm, _this, args| {
    let s = to_str(vm, arg(args, 0))?.trim().to_string();
    Ok(Value::from_f64(parse_float_like(&s)))
});
native!(n_global_is_nan, |vm, _this, args| {
    let n = vm.to_number(arg(args, 0))?;
    Ok(Value::from_bool(n.is_nan()))
});
native!(n_global_is_finite, |vm, _this, args| {
    let n = vm.to_number(arg(args, 0))?;
    Ok(Value::from_bool(n.is_finite()))
});
pub fn parse_int_like(s: &str, radix: u32) -> f64 {
    let mut body = s;
    let mut sign = 1i64;
    if body.starts_with('-') {
        sign = -1;
        body = &body[1..];
    } else if body.starts_with('+') {
        body = &body[1..];
    }
    let mut radix = radix;
    if radix == 0 {
        if body.starts_with("0x") || body.starts_with("0X") {
            radix = 16;
            body = &body[2..];
        } else {
            radix = 10;
        }
    } else if radix == 16 && (body.starts_with("0x") || body.starts_with("0X")) {
        body = &body[2..];
    }
    let mut acc: f64 = 0.0;
    let mut any = false;
    for c in body.chars() {
        let d = c.to_digit(radix);
        match d {
            Some(d) => {
                acc = acc * radix as f64 + d as f64;
                any = true;
            }
            None => break,
        }
    }
    if !any {
        f64::NAN
    } else {
        sign as f64 * acc
    }
}
pub fn parse_float_like(s: &str) -> f64 {
    let mut end = 0usize;
    let mut seen_dot = false;
    let mut seen_e = false;
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == '-' || bytes[i] == '+') {
        i += 1;
    }
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() {
            i += 1;
            end = i;
        } else if c == '.' && !seen_dot && !seen_e {
            seen_dot = true;
            i += 1;
            end = i;
        } else if (c == 'e' || c == 'E') && !seen_e && end > 0 {
            seen_e = true;
            i += 1;
            if i < bytes.len() && (bytes[i] == '-' || bytes[i] == '+') {
                i += 1;
            }
        } else {
            break;
        }
    }
    let out: String = bytes[..end].iter().collect();
    let t = out.trim_end_matches('.');
    if t.is_empty() {
        f64::NAN
    } else {
        t.parse().unwrap_or(f64::NAN)
    }
}

macro_rules! math1 {
    ($name:ident, $call:expr) => {
        native!($name, |vm, _this, args| {
            let x = vm.to_number(arg(args, 0))?;
            Ok(Value::from_f64($call(x)))
        });
    };
}
macro_rules! math2 {
    ($name:ident, $call:expr) => {
        native!($name, |vm, _this, args| {
            let x = vm.to_number(arg(args, 0))?;
            let y = vm.to_number(arg(args, 1))?;
            Ok(Value::from_f64($call(x, y)))
        });
    };
}
math1!(n_math_abs, |x: f64| x.abs());
math1!(n_math_ceil, |x: f64| x.ceil());
math1!(n_math_floor, |x: f64| x.floor());
math1!(n_math_trunc, |x: f64| x.trunc());
math1!(n_math_sign, |x: f64| {
    if x.is_nan() {
        f64::NAN
    } else if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        x
    }
});
math1!(n_math_sqrt, |x: f64| x.sqrt());
math1!(n_math_cbrt, |x: f64| x.cbrt());
math1!(n_math_exp, |x: f64| x.exp());
math1!(n_math_log, |x: f64| x.ln());
math1!(n_math_log2, |x: f64| x.log2());
math1!(n_math_log10, |x: f64| x.log10());
math1!(n_math_sin, |x: f64| x.sin());
math1!(n_math_cos, |x: f64| x.cos());
math1!(n_math_tan, |x: f64| x.tan());
math1!(n_math_asin, |x: f64| x.asin());
math1!(n_math_acos, |x: f64| x.acos());
math1!(n_math_atan, |x: f64| x.atan());
math1!(n_math_sinh, |x: f64| x.sinh());
math1!(n_math_cosh, |x: f64| x.cosh());
math1!(n_math_tanh, |x: f64| x.tanh());
math2!(n_math_pow, |a: f64, b: f64| a.powf(b));
math2!(n_math_atan2, |a: f64, b: f64| a.atan2(b));
native!(n_math_round, |vm, _this, args| {
    let x = vm.to_number(arg(args, 0))?;
    // JS rounds .5 toward +Infinity: round(2.5)=3, round(-2.5)=-2. A "away from
    // zero" formula returns -3 for -2.5, which is a different number.
    // `x + 0.5` rounds up to 1.0 for values just below .5 (double rounding), so
    // the half-open interval below .5 is handled directly instead of by
    // arithmetic on the sum.
    let mut r = if x >= 0.0 && x < 0.5 {
        0.0
    } else {
        (x + 0.5).floor()
    };
    let _ = &mut r;
    // -0 is observable (Object.is, 1/x), so keep the sign of a negative tie.
    let r = if r == 0.0 && x < 0.0 { -0.0 } else { r };
    Ok(Value::from_f64(r))
});
native!(n_math_hypot, |vm, _this, args| {
    let mut s = 0.0f64;
    for a in args {
        let x = vm.to_number(*a)?;
        s += x * x;
    }
    Ok(Value::from_f64(s.sqrt()))
});
native!(n_math_min, |vm, _this, args| {
    let mut m = f64::INFINITY;
    for a in args {
        let x = vm.to_number(*a)?;
        if x.is_nan() {
            return Ok(Value::NAN);
        }
        if x < m {
            m = x;
        }
    }
    Ok(Value::from_f64(m))
});
native!(n_math_max, |vm, _this, args| {
    let mut m = f64::NEG_INFINITY;
    for a in args {
        let x = vm.to_number(*a)?;
        if x.is_nan() {
            return Ok(Value::NAN);
        }
        if x > m {
            m = x;
        }
    }
    Ok(Value::from_f64(m))
});
native!(n_math_random, |_vm, _this, _args| Ok(Value::from_f64(0.5)));
native!(n_math_imul, |vm, _this, args| {
    let a = vm.to_int32(arg(args, 0))?;
    let b = vm.to_int32(arg(args, 1))?;
    Ok(Value::from_i32(a.wrapping_mul(b)))
});
native!(n_math_clz32, |vm, _this, args| {
    let x = vm.to_uint32(arg(args, 0))?;
    Ok(Value::from_i32(x.leading_zeros() as i32))
});
native!(n_math_fround, |vm, _this, args| {
    let x = vm.to_number(arg(args, 0))?;
    Ok(Value::from_f64(x as f32 as f64))
});

// ---- JSON ----
native!(n_json_stringify, |vm, _this, args| {
    let v = arg(args, 0);
    let indent = if args.len() > 2 && arg(args, 2).is_number() {
        arg(args, 2).as_i64() as usize
    } else {
        0
    };
    let space = if args.len() > 2 && arg(args, 2).is_string() {
        to_str(vm, arg(args, 2))?
    } else {
        " ".repeat(indent.min(10))
    };
    // JSON.stringify(undefined) and JSON.stringify(function) return undefined
    // (not the string "null") — node prints them as bare `undefined`.
    if v.is_nil() && !v.is_null() {
        return Ok(Value::UNDEFINED);
    }
    if v.is_function() {
        return Ok(Value::UNDEFINED);
    }
    let mut out = String::new();
    json_stringify(vm, v, &space, 0, &mut out)?;
    Ok(vm.new_string(&out))
});
fn json_stringify(
    vm: &mut Vm,
    v: Value,
    space: &str,
    depth: usize,
    out: &mut String,
) -> VmResult<()> {
    let nl = |depth: usize, space: &str, out: &mut String| {
        if space.is_empty() {
            return;
        }
        out.push('\n');
        for _ in 0..depth {
            out.push_str(space);
        }
    };
    if v.is_nil() {
        out.push_str("null");
        return Ok(());
    }
    // `toJSON` runs before encoding an object: `JSON.stringify({d:{toJSON(){…}}})`
    // serialises the method's return value, not the object. Date already relies
    // on this in JS, and libraries return their wire format from it.
    if v.is_object() || v.is_array() {
        let ktj = key(vm, "toJSON");
        if let Ok(f) = vm.get_prop(v, ktj) {
            if f.is_function() {
                let replaced = vm.call_callback(f, v, &[])?;
                if replaced.bits() != v.bits() {
                    return json_stringify(vm, replaced, space, depth, out);
                }
            }
        }
    }
    match unsafe { v.obj_tag() } {
        Some(TAG_ARRAY) => {
            let a = unsafe { v.as_array() };
            if a.len == 0 {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for i in 0..a.len {
                if i > 0 {
                    out.push(',');
                }
                nl(depth + 1, space, out);
                let item = a.get(i);
                let n = item.as_i64();
                if item.is_number() && (item.is_nan() || !item.as_f64().is_finite()) {
                    out.push_str("null");
                } else if item.is_function() {
                    out.push_str("null");
                } else {
                    let _ = n;
                    json_stringify(vm, item, space, depth + 1, out)?;
                }
            }
            nl(depth, space, out);
            out.push(']');
            Ok(())
        }
        Some(TAG_OBJECT) => {
            let obj = unsafe { v.as_object() };
            // An object with no accessors stores plain values in `vals` in the
            // same order as `keys`, so walking the two slices is exactly what
            // `own_keys().filter_map(get)` produces — without a Vec per
            // object. A 5000-element round trip was staging 5000 of them.
            // JSON key order is the same canonical order `Object.keys` uses.
            let keys = sort_own_keys(vm, obj.own_keys());
            let pairs: Vec<(PropKey, Value)> = keys
                .iter()
                .filter_map(|k| obj.get(*k).map(|val| (*k, val)))
                .collect();
            if pairs.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            // Comma tracking follows emission, not index: the accessor filter
            // below can remove the first entry, and a comma for a filtered-out
            // entry would corrupt the output (`{,"plain":7}`).
            let mut first = true;
            for (k, val) in pairs.iter() {
                // Omit undefined/function values before emitting anything, so
                // no key or comma leaks for a property that vanishes.
                if val.is_nil() && !val.is_null() || val.is_function() {
                    continue;
                }
                let ks = vm.prop_name(*k);
                // Internal accessor slots are not properties. Serializing them
                // leaked `#get:x`/`#set:x` (the engine's own metadata) into
                // output, where node emits nothing — an object with a getter
                // round-tripped as an object containing machine-internal keys.
                // Only those two prefixes are filtered, so a user key literally
                // named `#x` still serializes.
                if ks.starts_with("#get:") || ks.starts_with("#set:") {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                nl(depth + 1, space, out);
                out.push('"');
                json_escape(&ks, out);
                out.push('"');
                out.push(':');
                if !space.is_empty() {
                    out.push(' ');
                }
                if val.is_number() && (val.is_nan() || !val.as_f64().is_finite()) {
                    out.push_str("null");
                } else {
                    json_stringify(vm, *val, space, depth + 1, out)?;
                }
            }
            nl(depth, space, out);
            out.push('}');
            Ok(())
        }
        _ => {
            if v.is_string() {
                out.push('"');
                let string = unsafe { v.as_string() };
                if string.ascii != 0 {
                    json_escape(unsafe { string.as_str() }, out);
                } else {
                    for c in char::decode_utf16(string.code_units()) {
                        match c {
                            Ok(c) => {
                                let mut buf = [0; 4];
                                json_escape(c.encode_utf8(&mut buf), out);
                            }
                            Err(e) => {
                                use std::fmt::Write;
                                let _ = write!(out, "\\u{:04x}", e.unpaired_surrogate());
                            }
                        }
                    }
                }
                out.push('"');
                Ok(())
            } else if v.is_number() {
                let n = v.as_f64();
                if n.is_finite() {
                    out.push_str(&f64_to_js_string(n));
                } else {
                    out.push_str("null");
                }
                Ok(())
            } else if v.is_bool() {
                out.push_str(if v.as_bool() { "true" } else { "false" });
                Ok(())
            } else {
                out.push_str("null");
                Ok(())
            }
        }
    }
}
fn json_escape(s: &str, out: &mut String) {
    // Fast path: most keys/values contain nothing to escape — scan bytes and
    // copy the whole run in one push instead of a `push_str` per character.
    // (A 5,000-entry stringify was paying this per key *and* per value.)
    let bytes = s.as_bytes();
    let mut clean = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c < 0x20 || c == b'"' || c == b'\\' {
            if clean < i {
                out.push_str(&s[clean..i]);
            }
            match c {
                b'"' => out.push_str("\\\""),
                b'\\' => out.push_str("\\\\"),
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                0x08 => out.push_str("\\b"),
                0x0c => out.push_str("\\f"),
                _ => out.push_str(&format!("\\u{:04x}", c as u32)),
            }
            clean = i + 1;
        }
        i += 1;
    }
    if clean < s.len() {
        out.push_str(&s[clean..]);
    }
    if clean == 0 && bytes.is_empty() {
        return;
    }
}
native!(n_json_parse, |vm, _this, args| {
    let s = to_str(vm, arg(args, 0))?;
    let mut sc = JsonScan {
        b: s.as_bytes(),
        i: 0,
    };
    let v = json_parse_value(vm, &mut sc)?;
    sc.skip_ws();
    if sc.i != sc.b.len() {
        return Err(type_error("Unexpected non-whitespace character after JSON"));
    }
    Ok(v)
});

/// Byte-index JSON scanner. The previous scanner walked
/// `Peekable<str::Chars>`, which decodes UTF-8 one character at a time and
/// routed every string through a `Vec<u16>` and back. Structural bytes,
/// digits and ASCII strings are all single-byte work; only escapes and
/// non-ASCII text take the slow path, and numbers parse straight out of the
/// borrowed slice with no intermediate allocation.
struct JsonScan<'a> {
    b: &'a [u8],
    i: usize,
}

impl JsonScan<'_> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    #[inline]
    fn next(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }
    #[inline]
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\t' | b'\r')) {
            self.i += 1;
        }
    }
}

fn json_parse_value(vm: &mut Vm, sc: &mut JsonScan) -> VmResult<Value> {
    sc.skip_ws();
    let c = match sc.peek() {
        Some(c) => c,
        None => return Err(type_error("Unexpected end of JSON input")),
    };
    match c {
        b'{' => {
            sc.next();
            let o = vm.new_object();
            sc.skip_ws();
            if sc.peek() == Some(b'}') {
                sc.next();
                return Ok(o);
            }
            loop {
                sc.skip_ws();
                if sc.next() != Some(b'"') {
                    return Err(syntax_error("Unexpected token in JSON"));
                }
                // Keys normalize through the lossy conversion: the lookup
                // name is all that matters, and lone-surrogate keys are a
                // pathological edge.
                let k = json_parse_string_units(sc).map(|(u, _)| String::from_utf16_lossy(&u))?;
                sc.skip_ws();
                if sc.next() != Some(b':') {
                    return Err(type_error("Expected ':' in JSON"));
                }
                let v = json_parse_value(vm, sc)?;
                // Intern the key's symbol directly: `key()` would allocate a
                // JString only to immediately intern its bytes, once per key
                // of every parsed object. And a fresh JSON object needs no
                // store IC or accessor scan — `own_define` is one find_index,
                // a shape transition and the write. (It is also what makes
                // `{"__proto__": x}` an own property, as in node, rather than
                // a prototype-setter call.)
                let kk = PropKey::Sym(vm.arena().intern_sym(&k));
                vm.own_define(o, kk, v);
                sc.skip_ws();
                match sc.next() {
                    Some(b',') => continue,
                    Some(b'}') => break,
                    _ => return Err(type_error("Expected ',' or '}' in JSON")),
                }
            }
            Ok(o)
        }
        b'[' => {
            sc.next();
            let a = vm.new_array();
            sc.skip_ws();
            if sc.peek() == Some(b']') {
                sc.next();
                return Ok(a);
            }
            loop {
                let v = json_parse_value(vm, sc)?;
                unsafe { a.as_array_mut().push(v) };
                sc.skip_ws();
                match sc.next() {
                    Some(b',') => continue,
                    Some(b']') => break,
                    _ => return Err(type_error("Expected ',' or '}' in JSON")),
                }
            }
            Ok(a)
        }
        b'"' => {
            sc.next();
            json_parse_string_value(vm, sc)
        }
        b't' => {
            if sc.b[sc.i..].starts_with(b"true") {
                sc.i += 4;
                Ok(Value::TRUE)
            } else {
                Err(syntax_error("Unexpected token in JSON"))
            }
        }
        b'f' => {
            if sc.b[sc.i..].starts_with(b"false") {
                sc.i += 5;
                Ok(Value::FALSE)
            } else {
                Err(syntax_error("Unexpected token in JSON"))
            }
        }
        b'n' => {
            if sc.b[sc.i..].starts_with(b"null") {
                sc.i += 4;
                Ok(Value::NULL)
            } else {
                Err(syntax_error("Unexpected token in JSON"))
            }
        }
        b'-' | b'0'..=b'9' => {
            let start = sc.i;
            sc.next();
            while let Some(c) = sc.peek() {
                if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E') {
                    sc.next();
                } else {
                    break;
                }
            }
            let tok = &sc.b[start..sc.i];
            if !json_number_ok(tok) {
                return Err(syntax_error("Unexpected token in JSON"));
            }
            // The token is ASCII by construction, so `from_utf8` cannot fail.
            let text = std::str::from_utf8(tok).unwrap_or("0");
            Ok(Value::from_f64(parse_float_like(text)))
        }
        _ => Err(syntax_error("Unexpected token in JSON")),
    }
}

/// The JSON number grammar, checked on the borrowed token: `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`.
/// `01`, `1.`, `.5` and a bare `-` are all invalid JSON, and the old
/// char-collecting path accepted them.
fn json_number_ok(tok: &[u8]) -> bool {
    let mut i = 0usize;
    if i < tok.len() && tok[i] == b'-' {
        i += 1;
    }
    match tok.get(i) {
        Some(b'0') => {
            i += 1;
        }
        Some(b'1'..=b'9') => {
            while matches!(tok.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
        }
        _ => return false,
    }
    if tok.get(i) == Some(&b'.') {
        i += 1;
        if !matches!(tok.get(i), Some(b'0'..=b'9')) {
            return false;
        }
        while matches!(tok.get(i), Some(b'0'..=b'9')) {
            i += 1;
        }
    }
    if matches!(tok.get(i), Some(b'e') | Some(b'E')) {
        i += 1;
        if matches!(tok.get(i), Some(b'+') | Some(b'-')) {
            i += 1;
        }
        if !matches!(tok.get(i), Some(b'0'..=b'9')) {
            return false;
        }
        while matches!(tok.get(i), Some(b'0'..=b'9')) {
            i += 1;
        }
    }
    i == tok.len()
}

/// A string *value* keeps lone surrogates intact, like node: the string
/// comparison `JSON.parse('"\\ud83d"') === "\\uFFFD"` is false in node even
/// though the terminal prints both as U+FFFD. Only keys go through the lossy
/// conversion, where the lookup name is all that matters.
fn json_parse_string_value(vm: &mut Vm, sc: &mut JsonScan) -> VmResult<Value> {
    let (units, has_surrogate) = json_parse_string_units(sc)?;
    if has_surrogate {
        Ok(vm.new_string_utf16(&units))
    } else {
        // Pure ASCII or valid paired UTF-16: build the String losslessly.
        Ok(vm.new_string(&String::from_utf16_lossy(&units)))
    }
}

/// Parse a JSON string body (the opening quote already consumed). Returns the
/// UTF-16 units and whether any surrogate appeared.
/// ASCII runs without escapes are copied straight from the byte slice;
/// escapes and non-ASCII text take the UTF-16 detour.
fn json_parse_string_units(sc: &mut JsonScan) -> VmResult<(Vec<u16>, bool)> {
    let start = sc.i;
    // Fast scan: find the closing quote or the first interesting byte.
    let mut j = sc.i;
    let mut ascii_clean = true;
    while let Some(&c) = sc.b.get(j) {
        if c == b'"' {
            break;
        }
        if c == b'\\' || c < 0x20 || c >= 0x80 {
            ascii_clean = false;
            break;
        }
        j += 1;
    }
    if ascii_clean {
        match sc.b.get(j) {
            Some(b'"') => {
                let units = sc.b[start..j].iter().map(|&c| c as u16).collect();
                sc.i = j + 1;
                return Ok((units, false));
            }
            _ => return Err(type_error("Unterminated string in JSON")),
        }
    }
    // Slow path: escapes and/or non-ASCII.
    let mut out: Vec<u16> = Vec::new();
    let mut has_surrogate = false;
    // Re-emit the clean prefix we already scanned so the loop below stays simple.
    out.extend(sc.b[start..j].iter().map(|&c| c as u16));
    sc.i = j;
    loop {
        let Some(c) = sc.next() else {
            return Err(type_error("Unterminated string in JSON"));
        };
        match c {
            b'"' => break,
            b'\\' => match sc.next() {
                Some(b'n') => out.push(10),
                Some(b't') => out.push(9),
                Some(b'r') => out.push(13),
                Some(b'b') => out.push(8),
                Some(b'f') => out.push(12),
                Some(b'/') => out.push(47),
                Some(b'"') => out.push(34),
                Some(b'\\') => out.push(92),
                Some(b'u') => {
                    let mut n: u32 = 0;
                    for _ in 0..4 {
                        let h = sc
                            .next()
                            .ok_or_else(|| type_error("Bad Unicode escape in JSON"))?;
                        let d = (h as char)
                            .to_digit(16)
                            .ok_or_else(|| type_error("Bad Unicode escape in JSON"))?;
                        n = n * 16 + d;
                    }
                    let u = n as u16;
                    has_surrogate |= (0xD800..0xE000).contains(&u);
                    out.push(u);
                }
                _ => return Err(type_error("Bad escaped character in JSON")),
            },
            c if c < 0x20 => return Err(type_error("Bad control character in JSON")),
            c if c < 0x80 => out.push(c as u16),
            c => {
                // Multi-byte UTF-8: decode the code point from the bytes and
                // push its UTF-16 units, preserving prior semantics.
                let len = if c >= 0xF0 {
                    4
                } else if c >= 0xE0 {
                    3
                } else {
                    2
                };
                let mut buf = [0u8; 4];
                buf[0] = c;
                // Exactly len-1 continuation bytes: an unconditional skip(1)
                // over the 4-slot array read past the character and corrupted
                // the scan position for every following byte.
                for slot in buf.iter_mut().take(len).skip(1) {
                    *slot = sc
                        .next()
                        .ok_or_else(|| type_error("Unterminated string in JSON"))?;
                }
                let text = std::str::from_utf8(&buf[..len])
                    .map_err(|_| type_error("Invalid Unicode in JSON"))?;
                let mut units = [0u16; 2];
                out.extend_from_slice(
                    text.chars()
                        .next()
                        .unwrap_or('\u{fffd}')
                        .encode_utf16(&mut units),
                );
            }
        }
    }
    Ok((out, has_surrogate))
}

// ---------------------------------------------------------------------------
// console / process / timers
// ---------------------------------------------------------------------------

/// Node's console quotes strings that appear *inside* containers (`[ 'a' ]`,
/// `{ k: 'v' }`) but not at top level. Escape the two characters that matter
/// and wrap in single quotes, matching node's inspect style for the common
/// cases.
fn inspect_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn display(vm: &mut Vm, v: Value, depth: usize) -> VmResult<String> {
    display_q(vm, v, depth, false)
}

fn display_q(vm: &mut Vm, v: Value, depth: usize, quote: bool) -> VmResult<String> {
    if v.is_string() {
        let text = unsafe { v.as_string().as_str() }.to_string();
        return Ok(if quote { inspect_string(&text) } else { text });
    }
    if v.is_nan() {
        return Ok("NaN".into());
    }
    if v.is_smi() || v.is_double() {
        let f = v.as_f64();
        // console.log(-0) prints "-0" in node, while String(-0) is "0": the
        // diagnostic path distinguishes the two zeros, the ToString path does
        // not.
        if f == 0.0 && f.is_sign_negative() {
            return Ok("-0".into());
        }
        return Ok(f64_to_js_string(f));
    }
    if v.is_bool() {
        return Ok(if v.as_bool() {
            "true".into()
        } else {
            "false".into()
        });
    }
    if v.is_nil() {
        return Ok(if v.is_null() {
            "null".into()
        } else {
            "undefined".into()
        });
    }
    if v.is_function() {
        let n = if unsafe { v.as_function() }.name.is_string() {
            unsafe { v.as_function().name.as_string().as_str().to_string() }
        } else {
            String::new()
        };
        return Ok(if n.is_empty() {
            "[Function (anonymous)]".into()
        } else {
            format!("[Function: {}]", n)
        });
    }
    if depth > 3 {
        return Ok("[Object]".into());
    }
    if v.is_array() {
        let a = unsafe { v.as_array() };
        let mut parts = Vec::new();
        for i in 0..a.len {
            let item = a.get(i);
            if item.is_empty() {
                parts.push("<1 empty item>".to_string());
            } else {
                parts.push(display_q(vm, item, depth + 1, true)?);
            }
        }
        if parts.is_empty() {
            return Ok("[]".into());
        }
        return Ok(format!("[ {} ]", parts.join(", ")));
    }
    if v.is_object() {
        let keys = unsafe { v.as_object() }.own_keys();
        let mut parts = Vec::new();
        for k in keys {
            let val = unsafe { v.as_object() }.get(k).unwrap_or(Value::UNDEFINED);
            parts.push(format!(
                "{}: {}",
                vm.prop_name(k),
                display_q(vm, val, depth + 1, true)?
            ));
        }
        if parts.is_empty() {
            return Ok("{}".into());
        }
        return Ok(format!("{{ {} }}", parts.join(", ")));
    }
    Ok("[object Object]".into())
}

native!(n_console_log, |vm, _this, args| {
    let mut parts = Vec::new();
    for a in args {
        parts.push(display(vm, *a, 0)?);
    }
    let line = parts.join(" ");
    println!("{}", line);
    Ok(Value::UNDEFINED)
});
native!(n_console_error, |vm, _this, args| {
    let mut parts = Vec::new();
    for a in args {
        parts.push(display(vm, *a, 0)?);
    }
    eprintln!("{}", parts.join(" "));
    Ok(Value::UNDEFINED)
});
native!(n_console_warn, |vm, _this, args| {
    let mut parts = Vec::new();
    for a in args {
        parts.push(display(vm, *a, 0)?);
    }
    eprintln!("{}", parts.join(" "));
    Ok(Value::UNDEFINED)
});
native!(n_process_exit, |vm, _this, args| {
    let code = if args.is_empty() || arg(args, 0).is_undefined() {
        0
    } else {
        arg(args, 0).as_i64() as i32
    };
    vm.exit_code = code;
    Err(crate::errors::VmError::Internal(crate::vm::exit_marker(
        code,
    )))
});
native!(n_process_hrtime, |vm, _this, _args| {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as f64)
        .unwrap_or(0.0);
    let o = vm.new_object();
    let k = key(vm, "bigint");
    vm.set_prop(o, k, Value::from_i64(n as i64)).unwrap();
    Ok(Value::from_f64(n / 1e9))
});
native!(n_process_cwd, |vm, _this, _args| {
    let d = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Ok(vm.new_string(&d))
});

native!(n_set_timeout, |vm, _this, args| {
    let f = arg(args, 0);
    let ms = if args.len() > 1 {
        arg(args, 1).as_i64().max(0) as u64
    } else {
        0
    };
    let id = vm.next_timer_id();
    push_timer(TimerEntry {
        id,
        kind: TimerKind::Timeout,
        fire_at: std::time::Instant::now() + std::time::Duration::from_millis(ms),
        interval: ms,
        cb: f,
        args: args[2.min(args.len())..].to_vec(),
    });
    Ok(Value::from_i64(id as i64))
});
native!(n_set_interval, |vm, _this, args| {
    let f = arg(args, 0);
    let ms = if args.len() > 1 {
        arg(args, 1).as_i64().max(1) as u64
    } else {
        1
    };
    let id = vm.next_timer_id();
    push_timer(TimerEntry {
        id,
        kind: TimerKind::Interval,
        fire_at: std::time::Instant::now() + std::time::Duration::from_millis(ms),
        interval: ms,
        cb: f,
        args: args[2.min(args.len())..].to_vec(),
    });
    Ok(Value::from_i64(id as i64))
});
native!(n_clear_timer, |_vm, _this, args| {
    let id = arg(args, 0).as_i64() as u64;
    crate::vm::cancel_timer(id);
    Ok(Value::UNDEFINED)
});

// ---- Error ----
fn make_error(vm: &mut Vm, msg: Option<Value>, proto: Value, into: Value) -> VmResult<Value> {
    let o = if into.is_object() {
        into
    } else {
        Value::from_ptr(arena::active().alloc_object(0, proto))
    };
    let m = match msg {
        None => Value::UNDEFINED,
        Some(_) => Value::UNDEFINED,
    };
    let _ = m;
    if let Some(msg) = msg {
        if !msg.is_undefined() {
            let s = to_str(vm, msg)?;
            let kk = key(vm, "message");
            let sv = vm.new_string(&s);
            vm.set_prop(o, kk, sv).unwrap();
        }
    }
    Ok(o)
}
native!(n_error_ctor, |vm, this, args| {
    let o = make_error(vm, Some(arg(args, 0)), vm.error_proto(), this)?;
    Ok(o)
});
native!(n_error_to_string, |vm, this, _args| {
    let nk = key(vm, "name");
    let name = vm.get_prop(this, nk)?;
    let mk = key(vm, "message");
    let msg = vm.get_prop(this, mk)?;
    let n = to_str(vm, name).unwrap_or_else(|_| "Error".into());
    let m = to_str(vm, msg).unwrap_or_default();
    Ok(vm.new_string(&if m.is_empty() {
        n
    } else {
        format!("{}: {}", n, m)
    }))
});

// ---- runtime helpers ----
native!(n_h_push, |vm, _this, args| {
    let arr = arg(args, 0);
    let v = arg(args, 1);
    if arr.is_array() {
        unsafe { arr.as_array_mut() }.push(v);
    }
    Ok(arr)
});
/// Materialise a value for array *destructuring*: an array comes back as itself (no
/// allocation on the common `const [a, b] = arr` path), anything else is drained
/// through the iterator protocol exactly as spread does.
///
/// Array patterns used to index the right-hand side directly, so
/// `const [x, y] = new Set([3, 4])` bound `undefined, undefined` while node bound
/// `3, 4`. The machinery already existed for spread; this reuses it instead of
/// writing a second iteration implementation.
native!(n_iter_to_array, |vm, _this, args| {
    let v = arg(args, 0);
    if v.is_array() {
        return Ok(v);
    }
    let arr = vm.new_array();
    let vals = collect_iterable(vm, v)?;
    for item in vals {
        unsafe { arr.as_array_mut() }.push(item);
    }
    Ok(arr)
});

native!(n_h_push_all, |vm, _this, args| {
    let arr = arg(args, 0);
    let src = arg(args, 1);
    if !arr.is_array() {
        return Ok(Value::UNDEFINED);
    }
    if src.is_array() {
        let a = unsafe { src.as_array() };
        let mut vals = Vec::with_capacity(a.len as usize);
        for i in 0..a.len {
            vals.push(a.get(i));
        }
        for v in vals {
            unsafe { arr.as_array_mut() }.push(v);
        }
        return Ok(arr);
    }
    let vals = collect_iterable(vm, src)?;
    for v in vals {
        unsafe { arr.as_array_mut() }.push(v);
    }
    Ok(arr)
});
native!(n_h_apply, |vm, _this, args| {
    let f = arg(args, 0);
    let this = arg(args, 1);
    let list = arg(args, 2);
    let mut vals = Vec::new();
    if list.is_array() {
        let a = unsafe { list.as_array() };
        for i in 0..a.len {
            vals.push(a.get(i));
        }
    } else {
        vals = collect_iterable(vm, list)?;
    }
    vm.call_callback(f, this, &vals)
});
native!(n_h_apply_new, |vm, _this, args| {
    let f = arg(args, 0);
    let list = arg(args, 1);
    let mut vals = Vec::new();
    if list.is_array() {
        let a = unsafe { list.as_array() };
        for i in 0..a.len {
            vals.push(a.get(i));
        }
    } else {
        vals = collect_iterable(vm, list)?;
    }
    let base = vm.push_args(&vals);
    let r = vm.construct_at(f, base, vals.len() as u32);
    vm.pop_to(base);
    r
});
native!(n_h_tonum, |vm, _this, args| Ok(Value::from_f64(
    vm.to_number(arg(args, 0))?
)));
native!(n_h_enum_keys, |vm, _this, args| {
    let v = arg(args, 0);
    let out = vm.new_array();
    let mut seen: Vec<String> = Vec::new();
    let mut cur = v;
    let mut guard = 0;
    while (cur.is_object() || cur.is_function() || cur.is_array()) && guard < 64 {
        // Built-in prototypes are marked: their methods are non-enumerable in
        // JavaScript, so `for (k in {})` must not yield `toString` etc.
        let hidden = match unsafe { cur.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { cur.as_object() }.hdr.flags & FLAG_HIDDEN_KEYS != 0,
            Some(TAG_FUNCTION) => unsafe { cur.as_function() }.hdr.flags & FLAG_HIDDEN_KEYS != 0,
            _ => false,
        };
        if hidden {
            break;
        }
        let keys = own_keys_of(vm, cur);
        for k in keys {
            let name = vm.prop_name(k);
            // `#get:x` / `#set:x` are engine-internal accessor slots and must
            // never surface as properties.
            if name.starts_with('#') {
                continue;
            }
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let s = vm.new_string(&name);
            unsafe { out.as_array_mut().push(s) };
        }
        cur = vm.proto_of(cur);
        guard += 1;
    }
    // `for (k in "ab")` enumerates the string's indices.
    if v.is_string() {
        let n = unsafe { v.as_string().utf16_len as usize };
        for i in 0..n {
            let s = vm.new_string(&i.to_string());
            unsafe { out.as_array_mut().push(s) };
        }
    }
    // An array's own keys are its indices, which the walk above already
    // collected (it visits the array itself, and its prototype is marked
    // hidden). The separate index pass used to double them — and before the
    // walk accepted arrays it inserted into `data` without updating `len`, so
    // the result read back empty and `for (i in arr)` never iterated.
    Ok(out)
});
native!(n_h_values, |vm, this, _args| {
    let it = vm.new_object();
    let inner = vm.new_array();
    if this.is_array() {
        let a = unsafe { this.as_array() };
        for i in 0..a.len {
            unsafe { inner.as_array_mut().push(a.get(i)) };
        }
    } else if this.is_string() {
        let s = unsafe { this.as_string().as_str().to_string() };
        for c in s.chars() {
            unsafe { inner.as_array_mut().push(vm.new_string(&c.to_string())) };
        }
    } else if this.is_object() {
        let keys = own_keys_of(vm, this);
        for k in keys {
            let val = vm.get_prop(this, k)?;
            unsafe { inner.as_array_mut().push(val) };
        }
    }
    let k = key(vm, "#src");
    vm.set_prop(it, k, inner).unwrap();
    let k = key(vm, "#i");
    vm.set_prop(it, k, Value::from_i32(0)).unwrap();
    let nf = vm.new_function_native("next", n_iter_next, 0);
    let k = key(vm, "next");
    vm.set_prop(it, k, nf).unwrap();
    Ok(it)
});
native!(n_h_rest_object, |vm, this, args| {
    let out = vm.new_object();
    let _ = this;
    let src = arg(args, 0);
    let exclude: Vec<String> = args
        .iter()
        .skip(1)
        .map(|v| to_str(vm, *v).unwrap_or_default())
        .collect();
    copy_own_values(vm, out, src, &exclude)?;
    Ok(out)
});
/// Copy own enumerable properties by *name*, reading each value through
/// `get_prop` so a getter is invoked and the destination receives a data
/// property. Copying by internal key instead wrote `#get:`/`#set:` slots, which
/// is why `{...o}` on `{get a(){...}}` produced an object with no `a`.
fn copy_own_values(vm: &mut Vm, dst: Value, src: Value, exclude: &[String]) -> VmResult<()> {
    for name in own_property_names(vm, src) {
        if exclude.iter().any(|e| e == &name) {
            continue;
        }
        let k = key(vm, &name);
        let val = vm.get_prop(src, k)?;
        vm.set_prop(dst, k, val)?;
    }
    Ok(())
}

native!(n_h_assign_all, |vm, _this, args| {
    let dst = arg(args, 0);
    let src = arg(args, 1);
    copy_own_values(vm, dst, src, &[])?;
    Ok(dst)
});
native!(n_h_new_regexp, |vm, _this, args| {
    let pat = to_str(vm, arg(args, 0))?;
    let flags = to_str(vm, arg(args, 1))?;
    make_regexp(vm, &pat, &flags)
});

native!(n_regexp_ctor, |vm, _this, args| {
    // `RegExp(re)` with no flags returns the argument unchanged, like node.
    if args.len() < 2 {
        if let Some((src, flg)) = as_regexp(vm, arg(args, 0)) {
            return make_regexp(vm, &src, &flg);
        }
    }
    let src = if arg(args, 0).is_undefined() {
        String::new()
    } else if let Some((s, _)) = as_regexp(vm, arg(args, 0)) {
        s
    } else {
        to_str(vm, arg(args, 0))?
    };
    let flags = if arg(args, 1).is_undefined() {
        String::new()
    } else {
        to_str(vm, arg(args, 1))?
    };
    make_regexp(vm, &src, &flags)
});

native!(n_regexp_to_string, |vm, this, _args| {
    let (s, f) = as_regexp(vm, this).unwrap_or_default();
    Ok(vm.new_string(&format!("/{}/{}", s, f)))
});

native!(n_regexp_test, |vm, this, args| {
    let (src, flags) = match as_regexp(vm, this) {
        Some(p) => p,
        None => return Err(type_error("RegExp.prototype.test on a non-regexp")),
    };
    let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
    let hay = to_str(vm, arg(args, 0))?;
    let start = if re.flags.global || re.flags.sticky {
        let kl = key(vm, "lastIndex");
        vm.get_prop(this, kl)?.as_i64().max(0) as usize
    } else {
        0
    };
    let found = re.exec(&hay, start.min(hay.len()));
    if re.flags.global || re.flags.sticky {
        let next = match &found {
            Some(m) if m.end > m.start => m.end as i64,
            Some(_) => (start as i64) + 1,
            None => 0,
        };
        let kl = key(vm, "lastIndex");
        vm.set_prop(this, kl, Value::from_i64(next))?;
    }
    Ok(Value::from_bool(found.is_some()))
});

native!(n_regexp_exec, |vm, this, args| {
    let (src, flags) = match as_regexp(vm, this) {
        Some(p) => p,
        None => return Err(type_error("RegExp.prototype.exec on a non-regexp")),
    };
    let re = compiled_regex(&src, &flags).map_err(|e| type_error(&e))?;
    let hay = to_str(vm, arg(args, 0))?;
    let start = if re.flags.global || re.flags.sticky {
        let kl = key(vm, "lastIndex");
        vm.get_prop(this, kl)?.as_i64().max(0) as usize
    } else {
        0
    };
    let found = re.exec(&hay, start.min(hay.len()));
    let Some(m) = found else {
        if re.flags.global || re.flags.sticky {
            let kl = key(vm, "lastIndex");
            vm.set_prop(this, kl, Value::from_i32(0))?;
        }
        return Ok(Value::NULL);
    };
    if re.flags.global || re.flags.sticky {
        let next = if m.end > m.start {
            m.end as i64
        } else {
            (start as i64) + 1
        };
        let kl = key(vm, "lastIndex");
        vm.set_prop(this, kl, Value::from_i64(next))?;
    }
    let mut items = Vec::with_capacity(m.caps.len());
    for c in &m.caps {
        items.push(match c {
            Some((s, e)) => vm.new_string(&hay[*s..*e]),
            None => Value::UNDEFINED,
        });
    }
    let arr = vm.new_array_with(items);
    let ki = key(vm, "index");
    vm.set_prop(arr, ki, Value::from_i64(m.start as i64))?;
    let kin = key(vm, "input");
    let hv = vm.new_string(&hay);
    vm.set_prop(arr, kin, hv)?;
    Ok(arr)
});
// `__proto__` is the internal prototype slot, not a data property, so it needs
// its own helper rather than `Object.defineProperty`.
native!(n_h_set_proto, |vm, _this, args| {
    let o = arg(args, 0);
    let p = arg(args, 1);
    if p.is_object() || p.is_array() || p.is_function() || p.is_null() {
        vm.set_prototype(o, p)?;
    }
    Ok(o)
});
// `__jhol_rest_array(v, from)` — the tail of `v` from `from` on, used by array
// destructuring rest patterns.
native!(n_h_rest_array, |vm, _this, args| {
    let v = arg(args, 0);
    let from = arg(args, 1).as_i64().max(0) as usize;
    let out = vm.new_array();
    if v.is_array() {
        let a = unsafe { v.as_array() };
        for i in from..a.len as usize {
            let x = a.get(i as u32);
            unsafe { out.as_array_mut() }.push(x);
        }
    } else {
        let vals = collect_iterable(vm, v)?;
        for (i, x) in vals.into_iter().enumerate() {
            if i >= from {
                unsafe { out.as_array_mut() }.push(x);
            }
        }
    }
    Ok(out)
});

// ---- Map / Set (array-backed) ----
fn hidden(vm: &mut Vm, obj: Value, name: &str, create: impl FnOnce(&mut Vm) -> Value) -> Value {
    let __k = key(vm, name);
    let existing = vm.get_prop(obj, __k).unwrap_or(Value::UNDEFINED);
    if existing.is_array() {
        return existing;
    }
    let v = create(vm);
    let __k = key(vm, name);
    vm.set_prop(obj, __k, v).unwrap();
    v
}
native!(n_map_ctor, |vm, this, args| {
    let o = if this.is_object() {
        this
    } else {
        vm.new_object()
    };
    let pairs = hidden(vm, o, "#pairs", |vm| vm.new_array());
    if args.len() > 0 && arg(args, 0).is_array() {
        let src = collect_iterable(vm, arg(args, 0))?;
        for pair in src {
            if pair.is_array() {
                let a = unsafe { pair.as_array() };
                if a.len >= 2 {
                    let entry = vm.new_array();
                    unsafe {
                        entry.as_array_mut().push(a.get(0));
                        entry.as_array_mut().push(a.get(1));
                        pairs.as_array_mut().push(entry);
                    }
                }
            }
        }
    }
    Ok(o)
});
native!(n_map_set, |vm, this, args| {
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    let k = arg(args, 0);
    let v = arg(args, 1);
    let a = unsafe { pairs.as_array_mut() };
    for i in 0..a.len {
        let e = a.get(i);
        let ea = unsafe { e.as_array() };
        if vm.strict_eq(ea.get(0), k).unwrap_or(false) {
            let _ = ea;
            let ea2 = unsafe { e.as_array_mut() };
            if ea2.len >= 2 {
                ea2.data[1] = v;
            }
            return Ok(this);
        }
    }
    let entry = vm.new_array();
    unsafe {
        entry.as_array_mut().push(k);
        entry.as_array_mut().push(v);
        pairs.as_array_mut().push(entry);
    }
    Ok(this)
});
native!(n_map_get, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    for i in 0..a.len {
        let e = a.get(i);
        let ea = unsafe { e.as_array() };
        if vm.strict_eq(ea.get(0), k).unwrap_or(false) {
            return Ok(ea.get(1));
        }
    }
    Ok(Value::UNDEFINED)
});
native!(n_map_has, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    for i in 0..a.len {
        let e = a.get(i);
        let ea = unsafe { e.as_array() };
        if vm.strict_eq(ea.get(0), k).unwrap_or(false) {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_map_delete, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array_mut() };
    let n = a.len;
    for i in 0..n {
        let e = a.get(i);
        let ea = unsafe { e.as_array() };
        if vm.strict_eq(ea.get(0), k).unwrap_or(false) {
            a.data.remove(i as usize);
            a.len = a.data.len() as u32;
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_map_clear, |vm, this, _args| {
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    unsafe { pairs.as_array_mut() }.clear();
    Ok(Value::UNDEFINED)
});
native!(n_map_size, |vm, this, _args| {
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    Ok(Value::from_i32(unsafe { pairs.as_array() }.len as i32))
});
native!(n_map_for_each, |vm, this, args| {
    let f = arg(args, 0);
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    let mut vals = Vec::new();
    for i in 0..a.len {
        let e = a.get(i);
        let ea = unsafe { e.as_array() };
        vals.push((ea.get(0), ea.get(1)));
    }
    for &(k, v) in &vals {
        vm.pin_temporaries(&[k, v]);
    }
    for (k, v) in vals {
        vm.call_callback(f, Value::UNDEFINED, &[v, k, this])?;
    }
    Ok(Value::UNDEFINED)
});
fn map_list(vm: &mut Vm, this: Value, which: u8) -> VmResult<Value> {
    let pairs = hidden(vm, this, "#pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    let out = vm.new_array();
    for i in 0..a.len {
        let e = a.get(i);
        let ea = unsafe { e.as_array() };
        let item = match which {
            0 => ea.get(0),
            1 => ea.get(1),
            _ => {
                let pair = vm.new_array();
                unsafe {
                    pair.as_array_mut().push(ea.get(0));
                    pair.as_array_mut().push(ea.get(1));
                }
                pair
            }
        };
        unsafe { out.as_array_mut().push(item) };
    }
    Ok(out)
}
native!(n_map_keys, |vm, this, _args| map_list(vm, this, 0));
native!(n_map_values, |vm, this, _args| map_list(vm, this, 1));
native!(n_map_entries, |vm, this, _args| map_list(vm, this, 2));
native!(n_set_ctor, |vm, this, args| {
    let o = if this.is_object() {
        this
    } else {
        vm.new_object()
    };
    let items = hidden(vm, o, "#items", |vm| vm.new_array());
    if args.len() > 0 {
        // `new Set(iterable)` dedupes, exactly like repeated `add` calls.
        for v in collect_iterable(vm, arg(args, 0))? {
            let dup = {
                let a = unsafe { items.as_array() };
                let mut found = false;
                for i in 0..a.len {
                    if vm.strict_eq(a.get(i), v).unwrap_or(false) {
                        found = true;
                        break;
                    }
                }
                found
            };
            if !dup {
                unsafe { items.as_array_mut() }.push(v);
            }
        }
    }
    Ok(o)
});
native!(n_set_add, |vm, this, args| {
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    let v = arg(args, 0);
    let a = unsafe { items.as_array() };
    for i in 0..a.len {
        if vm.strict_eq(a.get(i), v).unwrap_or(false) {
            return Ok(this);
        }
    }
    unsafe { items.as_array_mut() }.push(v);
    Ok(this)
});
native!(n_set_has, |vm, this, args| {
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    let v = arg(args, 0);
    let a = unsafe { items.as_array() };
    for i in 0..a.len {
        if vm.strict_eq(a.get(i), v).unwrap_or(false) {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_set_delete, |vm, this, args| {
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    let v = arg(args, 0);
    let a = unsafe { items.as_array_mut() };
    for i in 0..a.len {
        if vm.strict_eq(a.get(i), v).unwrap_or(false) {
            a.data.remove(i as usize);
            a.len = a.data.len() as u32;
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_set_clear, |vm, this, _args| {
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    unsafe { items.as_array_mut() }.clear();
    Ok(Value::UNDEFINED)
});
native!(n_set_size, |vm, this, _args| {
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    Ok(Value::from_i32(unsafe { items.as_array() }.len as i32))
});
fn set_list(vm: &mut Vm, this: Value, entries: bool) -> VmResult<Value> {
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    let a = unsafe { items.as_array() };
    let out = vm.new_array();
    for i in 0..a.len {
        let v = a.get(i);
        let item = if entries {
            let pair = vm.new_array();
            unsafe {
                pair.as_array_mut().push(v);
                pair.as_array_mut().push(v);
            }
            pair
        } else {
            v
        };
        unsafe { out.as_array_mut().push(item) };
    }
    Ok(out)
}
native!(n_set_values, |vm, this, _args| set_list(vm, this, false));
native!(n_set_entries, |vm, this, _args| set_list(vm, this, true));
native!(n_set_for_each, |vm, this, args| {
    let f = arg(args, 0);
    let items = hidden(vm, this, "#items", |vm| vm.new_array());
    let a = unsafe { items.as_array() };
    let mut vals = Vec::new();
    for i in 0..a.len {
        vals.push(a.get(i));
    }
    vm.pin_temporaries(&vals);
    for v in vals {
        vm.call_callback(f, Value::UNDEFINED, &[v, v, this])?;
    }
    Ok(Value::UNDEFINED)
});

// ---------------------------------------------------------------------------
// Function global, Symbol registry, base64, WeakMap/WeakSet, crypto, Date
// ---------------------------------------------------------------------------

native!(n_function_ctor, |_vm, _this, _args| {
    // `new Function(body)` needs a compile-from-string pipeline; the common
    // probes are `typeof Function === "function"` and `Function.prototype`,
    // both of which work. Body compilation is documented as unsupported.
    Ok(Value::UNDEFINED)
});

// The symbol registry: description -> the one shared value, so
// `Symbol.for("x") === Symbol.for("x")` holds exactly.
/// The `Symbol.for` registry: description -> interned-atom bits. Atoms are
/// immortal and rooted in `atom_roots`, so holding their bits in a static is
/// GC-safe; the VM is single-threaded, so a thread_local cell is enough.
fn symbol_registry() -> &'static std::cell::RefCell<std::collections::HashMap<String, u64>> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static REG: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    }
    REG.with(|r| unsafe { std::mem::transmute::<&RefCell<HashMap<String, u64>>, &'static RefCell<HashMap<String, u64>>>(r) })
}

native!(n_symbol_for, |vm, _this, args| {
    let d = to_str(vm, arg(args, 0))?;
    // Identity comes from the key itself, so a second `for` call returns the same
    // symbol on any path — no registry needed for equality.
    Ok(register_symbol(vm, format!("\u{0}for:{}", d), d))
});

native!(n_symbol_key_for, |vm, _this, args| {
    let v = arg(args, 0);
    let Some(id) = v.as_symbol() else {
        return Err(type_error("Symbol.keyFor requires a symbol"));
    };
    // Spec: only symbols created through `Symbol.for` have a registry key, and those
    // are exactly the ones interned under the for-prefix.
    let registered = crate::vm::Vm::sym_name_of(id)
        .is_some_and(|name| name.starts_with("\u{0}for:"));
    if !registered {
        return Ok(Value::UNDEFINED);
    }
    match symbol_description(id) {
        Some(d) => Ok(vm.new_string(&d)),
        None => Ok(Value::UNDEFINED),
    }
});

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u8> = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c == '=' || c == '\n' || c == '\r' {
            continue;
        }
        let v = match c {
            'A'..='Z' => c as u8 - b'A',
            'a'..='z' => c as u8 - b'a' + 26,
            '0'..='9' => c as u8 - b'0' + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        vals.push(v);
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let n = (chunk[0] as u32) << 18
            | (chunk[1] as u32) << 12
            | (if chunk.len() > 2 { chunk[2] as u32 } else { 0 }) << 6
            | (if chunk.len() > 3 { chunk[3] as u32 } else { 0 });
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

native!(n_btoa, |vm, _this, args| {
    let s = arg(args, 0);
    if !s.is_string() {
        return Err(type_error("btoa requires a string"));
    }
    // Latin1 semantics: code units above 255 are a RangeError in node.
    let units = unsafe { s.as_string() };
    let mut bytes = Vec::with_capacity(units.utf16_len as usize);
    let mut bad = false;
    if units.ascii != 0 {
        bytes.extend_from_slice(unsafe { units.as_str() }.as_bytes());
    } else {
        for u in units.code_units() {
            if u > 255 {
                bad = true;
                break;
            }
            bytes.push(u as u8);
        }
    }
    if bad {
        return Err(type_error(
            "invalid character: string contains a code unit above 255",
        ));
    }
    Ok(vm.new_string(&b64_encode(&bytes)))
});

native!(n_atob, |vm, _this, args| {
    let s = arg(args, 0);
    if !s.is_string() {
        return Err(type_error("atob requires a string"));
    }
    let text = unsafe { s.as_string().as_str() }.to_string();
    match b64_decode(&text) {
        Some(bytes) => {
            let out: String = bytes.iter().map(|&b| b as char).collect();
            Ok(vm.new_string(&out))
        }
        None => Err(type_error("invalid base64")),
    }
});

// ---- WeakMap / WeakSet ----
// Identity-keyed: object/function keys compare by pointer bits. Entries whose
// key is collected are not swept (GC-naive); live-key behavior is exact.
fn weak_pairs(vm: &mut Vm, this: Value) -> Value {
    hidden(vm, this, "#weak-pairs", |vm| vm.new_array())
}

fn weak_key_ok(v: Value) -> bool {
    v.is_object() || v.is_function()
}

native!(n_weakmap_ctor, |vm, this, _args| {
    // Same shape as Map's ctor: pairs are created on the instance here, not
    // lazily — the lazy path stored into an array the instance never saw.
    let o = if this.is_object() {
        this
    } else {
        vm.new_object()
    };
    weak_pairs(vm, o);
    Ok(o)
});

native!(n_weakmap_set, |vm, this, args| {
    let k = arg(args, 0);
    if !weak_key_ok(k) {
        return Err(type_error("WeakMap key must be an object"));
    }
    let v = arg(args, 1);
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array_mut() };
    for i in 0..a.len {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            let ea = unsafe { e.as_array_mut() };
            if ea.len >= 2 {
                ea.data[1] = v;
            }
            return Ok(this);
        }
    }
    let entry = vm.new_array();
    unsafe {
        entry.as_array_mut().push(k);
        entry.as_array_mut().push(v);
        a.push(entry);
    }
    Ok(this)
});

native!(n_weakmap_get, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    for i in 0..a.len {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            return Ok(unsafe { e.as_array() }.get(1));
        }
    }
    Ok(Value::UNDEFINED)
});

native!(n_weakmap_has, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    for i in 0..a.len {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::FALSE)
});

native!(n_weakmap_delete, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array_mut() };
    let n = a.len;
    for i in 0..n {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            a.data.remove(i as usize);
            a.len = a.data.len() as u32;
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});

native!(n_weakset_ctor, |vm, this, _args| {
    let o = if this.is_object() {
        this
    } else {
        vm.new_object()
    };
    weak_pairs(vm, o);
    Ok(o)
});

native!(n_weakset_add, |vm, this, args| {
    let k = arg(args, 0);
    if !weak_key_ok(k) {
        return Err(type_error("WeakSet value must be an object"));
    }
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array_mut() };
    for i in 0..a.len {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            return Ok(this);
        }
    }
    let entry = vm.new_array();
    unsafe {
        entry.as_array_mut().push(k);
        a.push(entry);
    }
    Ok(this)
});

native!(n_weakset_has, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array() };
    for i in 0..a.len {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::FALSE)
});

native!(n_weakset_delete, |vm, this, args| {
    let k = arg(args, 0);
    let pairs = hidden(vm, this, "#weak-pairs", |vm| vm.new_array());
    let a = unsafe { pairs.as_array_mut() };
    let n = a.len;
    for i in 0..n {
        let e = a.get(i);
        if e.is_array() && unsafe { e.as_array() }.get(0).bits() == k.bits() {
            a.data.remove(i as usize);
            a.len = a.data.len() as u32;
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});

// ---- crypto ----
// Entropy from the OS. Unix reads /dev/urandom; the fallback is a
// splitmix64 chain seeded from the nanosecond clock (documented: the
// fallback is for exotic platforms, not for production entropy).
fn os_random(buf: &mut [u8]) -> bool {
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            return f.read_exact(buf).is_ok();
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = buf;
        false
    }
}

fn fallback_random(buf: &mut [u8]) {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
        | 1;
    for b in buf.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 24) as u8;
    }
}

native!(n_crypto_get_random_values, |vm, _this, args| {
    let a = arg(args, 0);
    if !a.is_array() {
        return Err(type_error("getRandomValues requires an array"));
    }
    let arr = unsafe { a.as_array_mut() };
    let mut buf = vec![0u8; arr.len as usize];
    if !os_random(&mut buf) {
        fallback_random(&mut buf);
    }
    for (i, b) in buf.iter().enumerate() {
        arr.set(i as u32, Value::from_i32(*b as i32));
    }
    Ok(a)
});

native!(n_crypto_random_uuid, |vm, _this, _args| {
    let mut buf = [0u8; 16];
    if !os_random(&mut buf) {
        fallback_random(&mut buf);
    }
    buf[6] = (buf[6] & 0x0f) | 0x40; // version 4
    buf[8] = (buf[8] & 0x3f) | 0x80; // variant
    let hex = |b: &[u8]| -> String { b.iter().map(|x| format!("{:02x}", x)).collect::<String>() };
    let s = format!(
        "{}-{}-{}-{}-{}",
        hex(&buf[0..4]),
        hex(&buf[4..6]),
        hex(&buf[6..8]),
        hex(&buf[8..10]),
        hex(&buf[10..16])
    );
    Ok(vm.new_string(&s))
});

// ---- Date ----
// Epoch-millisecond arithmetic over the proleptic Gregorian calendar.
// Days-to-civil conversion is the standard era decomposition; local time
// comes from a one-time offset probe (`date +%z`) cached for the process.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Local UTC-offset in minutes (what `getTimezoneOffset` returns: positive
/// west of UTC). Probed once via `date +%z` — "+0530" becomes -330.
fn tz_offset_minutes() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
            let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if t.len() == 5 {
                let sign = match &t[0..1] {
                    "+" => 1,
                    "-" => -1,
                    _ => 0,
                };
                if sign != 0 {
                    if let (Ok(h), Ok(m)) = (t[1..3].parse::<i64>(), t[3..5].parse::<i64>()) {
                        return -(sign * (h * 60 + m));
                    }
                }
            }
        }
        0
    })
}

fn date_time(vm: &mut Vm, this: Value) -> f64 {
    let k = key(vm, "#t");
    match vm.get_prop(this, k) {
        Ok(v) if v.is_number() => v.as_f64(),
        _ => f64::NAN,
    }
}

fn date_set_time_raw(vm: &mut Vm, this: Value, ms: f64) {
    let k = key(vm, "#t");
    let _ = vm.set_prop(this, k, Value::from_f64(ms));
}

/// (y, m 0-11, d, h, min, s, ms) in the given zone, from epoch ms.
fn date_fields(mut ms: f64, utc: bool) -> (i64, i64, i64, i64, i64, i64, i64) {
    if !utc {
        ms -= tz_offset_minutes() as f64 * 60_000.0;
    }
    let secs_total = ms.div_euclid(1000.0);
    let millis = ms.rem_euclid(1000.0) as i64;
    let days = (secs_total / 86400.0).floor();
    let secs = (secs_total - days * 86400.0) as i64;
    let (y, m, d) = civil_from_days(days as i64);
    let h = secs / 3600;
    let mi = (secs % 3600) / 60;
    let se = secs % 60;
    (y, m - 1, d, h, mi, se, millis)
}

fn make_epoch_ms(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64, ms: i64, utc: bool) -> f64 {
    let days = days_from_civil(y, m + 1, d);
    let secs = days * 86400 + h * 3600 + mi * 60 + s;
    let mut out = secs as f64 * 1000.0 + ms as f64;
    if !utc {
        out += tz_offset_minutes() as f64 * 60_000.0;
    }
    out
}

fn date_now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(f64::NAN)
}

/// ISO 8601 subset: `YYYY[-MM[-DD]][THH[:MM[:SS[.sss]]]][Z|±HH[:MM]]`.
/// Date-only forms are UTC (per spec); date-time without an offset is local.
fn date_parse_iso(text: &str) -> Option<f64> {
    let b: Vec<char> = text.chars().collect();
    if b.len() < 10 && !text.contains('T') {
        // accept "YYYY", "YYYY-MM", "YYYY-MM-DD"
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        if from > b.len() || to > b.len() || from >= to {
            return None;
        }
        b[from..to].iter().collect::<String>().parse::<i64>().ok()
    };
    if b.len() < 4 {
        return None;
    }
    let y = num(0, 4)?;
    let mut mo = 0i64;
    let mut da = 1i64;
    if b.len() >= 7 && b[4] == '-' {
        mo = num(5, 7)?.checked_sub(1)?;
        if b.len() >= 10 && b[7] == '-' {
            da = num(8, 10)?;
        }
    }
    let mut h = 0i64;
    let mut mi = 0i64;
    let mut se = 0i64;
    let mut ms = 0i64;
    let mut local = false;
    let mut offset: Option<i64> = None;
    if let Some(ti) = b.iter().position(|&c| c == 'T' || c == 't' || c == ' ') {
        if !(ti == 10) {
            return None;
        }
        h = num(11, 13).unwrap_or(-1);
        if b.len() >= 16 && b[13] == ':' {
            mi = num(14, 16).unwrap_or(-1);
            if b.len() >= 19 && b[16] == ':' {
                se = num(17, 19).unwrap_or(-1);
                if b.len() > 19 && b[19] == '.' {
                    let mut frac = String::new();
                    let mut j = 20;
                    while j < b.len() && b[j].is_ascii_digit() {
                        frac.push(b[j]);
                        j += 1;
                    }
                    if frac.is_empty() {
                        return None;
                    }
                    frac.truncate(3);
                    ms = frac.parse().ok()?;
                    if frac.len() < 3 {
                        ms *= 10u64.pow(3 - frac.len() as u32) as i64;
                    }
                    // re-scan for the zone after the fraction
                    let rest: String = b[j..].iter().collect();
                    if rest == "Z" || rest == "z" {
                        offset = Some(0);
                    } else if rest.len() >= 6 && (rest.starts_with('+') || rest.starts_with('-')) {
                        let sign = if rest.starts_with('-') { -1 } else { 1 };
                        let rh: i64 = rest[1..3].parse().ok()?;
                        let rm: i64 = rest[4..6].parse().ok()?;
                        offset = Some(sign * (rh * 60 + rm));
                    } else if !rest.is_empty() {
                        return None;
                    } else {
                        local = true;
                    }
                }
            }
        }
        let _ = ti;
    } else {
        offset = Some(0); // date-only forms are UTC
    }
    if !(1..=12).contains(&(mo + 1))
        || !(1..=31).contains(&da)
        || h < 0
        || h > 24
        || mi < 0
        || mi > 59
        || se < 0
        || se > 60
    {
        return None;
    }
    let mut out = make_epoch_ms(y, mo, da, h, mi, se, ms, offset.is_some() || !local);
    if let Some(off) = offset {
        out += off as f64 * 60_000.0;
    }
    Some(out)
}

native!(n_date_ctor, |vm, this, args| {
    let o = if this.is_object() {
        this
    } else {
        vm.new_object()
    };
    let ms: f64 = if args.is_empty() {
        date_now_ms()
    } else if args.len() == 1 {
        let a = arg(args, 0);
        if a.is_number() {
            a.as_f64()
        } else {
            let text = to_str(vm, a)?;
            date_parse_iso(&text).unwrap_or(f64::NAN)
        }
    } else {
        let y = arg(args, 0);
        // `new Date(98)` means 1998; `new Date(0..=99)` maps 1900+.
        let mut yr = if y.is_number() { y.as_f64() } else { f64::NAN };
        if yr.fract() == 0.0 && (0.0..100.0).contains(&yr) {
            yr += 1900.0;
        }
        let g = |i: usize, dflt: f64| -> f64 {
            let a = arg(args, i);
            if a.is_number() {
                a.as_f64()
            } else {
                dflt
            }
        };
        make_epoch_ms(
            yr as i64,
            g(1, 0.0) as i64,
            g(2, 1.0) as i64,
            g(3, 0.0) as i64,
            g(4, 0.0) as i64,
            g(5, 0.0) as i64,
            g(6, 0.0) as i64,
            false,
        )
    };
    date_set_time_raw(vm, o, ms);
    Ok(o)
});

native!(n_date_now, |_vm, _this, _args| Ok(Value::from_f64(
    date_now_ms()
)));

native!(n_date_utc, |_vm, _this, args| {
    let g = |i: usize, dflt: f64| -> f64 {
        let a = arg(args, i);
        if a.is_number() {
            a.as_f64()
        } else {
            dflt
        }
    };
    let mut yr = g(0, f64::NAN);
    if yr.fract() == 0.0 && (0.0..100.0).contains(&yr) {
        yr += 1900.0;
    }
    if yr.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_f64(make_epoch_ms(
        yr as i64,
        g(1, 0.0) as i64,
        g(2, 1.0) as i64,
        g(3, 0.0) as i64,
        g(4, 0.0) as i64,
        g(5, 0.0) as i64,
        g(6, 0.0) as i64,
        true,
    )))
});

native!(n_date_parse, |vm, _this, args| {
    let a = arg(args, 0);
    if a.is_number() {
        return Ok(Value::from_f64(a.as_f64()));
    }
    let text = to_str(vm, a)?;
    Ok(Value::from_f64(date_parse_iso(&text).unwrap_or(f64::NAN)))
});

native!(n_date_get_time, |_vm, this, _args| Ok(Value::from_f64(
    this_date_ms(_vm, this)
)));

fn this_date_ms(vm: &mut Vm, this: Value) -> f64 {
    if this.is_object() {
        date_time(vm, this)
    } else {
        f64::NAN
    }
}

native!(n_date_get_year, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, false).0))
});

native!(n_date_get_year_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).0))
});

native!(n_date_get_month, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, false).1))
});

native!(n_date_get_month_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).1))
});

native!(n_date_get_date, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, false).2))
});

native!(n_date_get_date_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).2))
});

native!(n_date_get_day, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    let f = date_fields(ms, false);
    // Day 0 (1970-01-01) was a Thursday: dow = (days + 4) % 7, 0 = Sunday.
    Ok(Value::from_i64(
        (days_from_civil(f.0, f.1 + 1, f.2) + 4).rem_euclid(7),
    ))
});

native!(n_date_get_day_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    let f = date_fields(ms, true);
    Ok(Value::from_i64(
        (days_from_civil(f.0, f.1 + 1, f.2) + 4).rem_euclid(7),
    ))
});

native!(n_date_get_hours, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, false).3))
});

native!(n_date_get_hours_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).3))
});

native!(n_date_get_minutes, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, false).4))
});

native!(n_date_get_minutes_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).4))
});

native!(n_date_get_seconds, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, false).5))
});

native!(n_date_get_seconds_utc, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).5))
});

native!(n_date_get_ms, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(date_fields(ms, true).6))
});

native!(n_date_tz_offset, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(Value::NAN);
    }
    Ok(Value::from_i64(tz_offset_minutes()))
});

native!(n_date_set_time, |vm, this, args| {
    let v = arg(args, 0);
    let ms = if v.is_number() { v.as_f64() } else { f64::NAN };
    date_set_time_raw(vm, this, ms);
    Ok(Value::from_f64(ms))
});

fn date_set_field(vm: &mut Vm, this: Value, idx: usize, val: f64, utc: bool) -> VmResult<Value> {
    let cur = this_date_ms(vm, this);
    if cur.is_nan() || val.is_nan() {
        date_set_time_raw(vm, this, f64::NAN);
        return Ok(Value::NAN);
    }
    let mut f = date_fields(cur, utc);
    match idx {
        0 => f.6 = val as i64,
        1 => f.5 = val as i64,
        2 => f.4 = val as i64,
        3 => f.3 = val as i64,
        4 => f.2 = val as i64,
        5 => f.1 = val as i64,
        _ => f.0 = val as i64,
    }
    let out = make_epoch_ms(f.0, f.1, f.2, f.3, f.4, f.5, f.6, utc);
    date_set_time_raw(vm, this, out);
    Ok(Value::from_f64(out))
}

native!(n_date_set_ms, |vm, this, args| {
    date_set_field(vm, this, 0, arg(args, 0).as_f64(), false)
});
native!(n_date_set_seconds, |vm, this, args| {
    date_set_field(vm, this, 1, arg(args, 0).as_f64(), false)
});
native!(n_date_set_minutes, |vm, this, args| {
    date_set_field(vm, this, 2, arg(args, 0).as_f64(), false)
});
native!(n_date_set_hours, |vm, this, args| {
    date_set_field(vm, this, 3, arg(args, 0).as_f64(), false)
});
native!(n_date_set_date, |vm, this, args| {
    date_set_field(vm, this, 4, arg(args, 0).as_f64(), false)
});
native!(n_date_set_month, |vm, this, args| {
    date_set_field(vm, this, 5, arg(args, 0).as_f64(), false)
});
native!(n_date_set_year, |vm, this, args| {
    date_set_field(vm, this, 6, arg(args, 0).as_f64(), false)
});
native!(n_date_set_date_utc, |vm, this, args| {
    date_set_field(vm, this, 4, arg(args, 0).as_f64(), true)
});
native!(n_date_set_month_utc, |vm, this, args| {
    date_set_field(vm, this, 5, arg(args, 0).as_f64(), true)
});
native!(n_date_set_year_utc, |vm, this, args| {
    date_set_field(vm, this, 6, arg(args, 0).as_f64(), true)
});

native!(n_date_to_iso, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Err(type_error("Invalid time value"));
    }
    let (y, mo, d, h, mi, se, msec) = date_fields(ms, true);
    Ok(vm.new_string(&format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        mo + 1,
        d,
        h,
        mi,
        se,
        msec
    )))
});

native!(n_date_to_string, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(vm.new_string("Invalid Date"));
    }
    let (y, mo, d, h, mi, se, _msec) = date_fields(ms, false);
    let dow = (days_from_civil(y, mo + 1, d) + 4).rem_euclid(7) as usize;
    let off = -tz_offset_minutes();
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    Ok(vm.new_string(&format!(
        "{} {} {:02} {} {:02}:{:02}:{:02} GMT{}{:02}{:02}",
        DAY_NAMES[dow],
        MONTH_NAMES[mo as usize],
        d,
        y,
        h,
        mi,
        se,
        sign,
        a / 60,
        a % 60
    )))
});

native!(n_date_to_utc_string, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(vm.new_string("Invalid Date"));
    }
    let (y, mo, d, h, mi, se, _msec) = date_fields(ms, true);
    let dow = (days_from_civil(y, mo + 1, d) + 4).rem_euclid(7) as usize;
    Ok(vm.new_string(&format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAY_NAMES[dow], d, MONTH_NAMES[mo as usize], y, h, mi, se
    )))
});

native!(n_date_to_date_string, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(vm.new_string("Invalid Date"));
    }
    let (y, mo, d, _h, _mi, _se, _msec) = date_fields(ms, false);
    let dow = (days_from_civil(y, mo + 1, d) + 4).rem_euclid(7) as usize;
    Ok(vm.new_string(&format!(
        "{} {} {:02} {}",
        DAY_NAMES[dow], MONTH_NAMES[mo as usize], d, y
    )))
});

native!(n_date_to_time_string, |vm, this, _args| {
    let ms = this_date_ms(vm, this);
    if ms.is_nan() {
        return Ok(vm.new_string("Invalid Date"));
    }
    let (_y, _mo, _d, h, mi, se, _msec) = date_fields(ms, false);
    let off = -tz_offset_minutes();
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    Ok(vm.new_string(&format!(
        "{:02}:{:02}:{:02} GMT{}{:02}{:02}",
        h,
        mi,
        se,
        sign,
        a / 60,
        a % 60
    )))
});

// ---------------------------------------------------------------------------
// Microtasks and Promise
//
// Everything here runs off one FIFO microtask queue. Promise callbacks are queued
// rather than called inline, which is what makes `.then` ordering match node instead
// of depending on where `resolve` happens to be called.
//
// Closures: a native fn receives no captured environment, so a callback that needs
// data uses a *bound* function whose `bound` array carries it — the same mechanism
// Function.prototype.bind uses. `n_promise_settle` gets its promise as `this`.
// ---------------------------------------------------------------------------

thread_local! {
    /// The Promise prototype, so a promise created by the constructor or by a
    /// combinator is linked to `then`/`catch`. Without this, `p.then(...)` was
    /// "undefined is not a function" — the object was linked to Object.prototype.
    static PROMISE_PROTO: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn promise_hidden_key(vm: &mut Vm, name: &str) -> PropKey {
    let id = vm.arena().intern_sym(&format!("\u{0}{}", name));
    PropKey::Sym(id)
}

fn get_hidden(vm: &mut Vm, obj: Value, name: &str) -> Value {
    let k = promise_hidden_key(vm, name);
    vm.get_prop(obj, k).unwrap_or(Value::UNDEFINED)
}

fn set_hidden(vm: &mut Vm, obj: Value, name: &str, v: Value) {
    let k = promise_hidden_key(vm, name);
    vm.define_prop(obj, k, v);
}

native!(n_queue_microtask, |vm, _this, args| {
    let f = arg(args, 0);
    if !f.is_function() {
        return Err(type_error("queueMicrotask expects a function"));
    }
    vm.queue_microtask(f, Vec::new());
    Ok(Value::UNDEFINED)
});

/// New promise in the pending state.
fn new_pending_promise(vm: &mut Vm) -> Value {
    let p = vm.new_object();
    let proto = PROMISE_PROTO.with(|c| c.get());
    if proto != 0 {
        let _ = vm.set_prototype(p, Value(proto));
    }
    set_hidden(vm, p, "state", Value::from_i32(0));
    set_hidden(vm, p, "value", Value::UNDEFINED);
    let handlers = vm.new_array();
    set_hidden(vm, p, "handlers", handlers);
    p
}

/// Bind `f` so that calling it runs with `this = bound_this`.
fn bind_native(vm: &mut Vm, f: Value, bound_this: Value, bound: Vec<Value>) -> Value {
    let f_proto_obj = unsafe { f.as_function() }.proto_obj;
    let nf = JFunction::native(Value::UNDEFINED, n_noop, Value::UNDEFINED);
    let fp = vm.arena().alloc_function(nf);
    let v = Value::from_ptr(fp);
    unsafe {
        (*fp).hdr.flags |= FLAG_BOUND;
        crate::arena::note_external_allocation(bound.capacity() * std::mem::size_of::<Value>());
        (*fp).bound = bound;
        (*fp).this_val = f;
        (*fp).bound_this = bound_this;
        (*fp).proto = Value::from_ptr(vm.proto_function_ptr() as *mut JFunction);
        (*fp).proto_obj = f_proto_obj;
    }
    v
}

/// Settle the promise in `this`. `a` selects fulfilled (1) or rejected (2).
fn settle(vm: &mut Vm, promise: Value, state: i32, value: Value) -> VmResult<()> {
    if !promise.is_object() {
        return Ok(());
    }
    let current = get_hidden(vm, promise, "state").as_i64();
    if current != 0 {
        return Ok(()); // A promise settles once.
    }
    set_hidden(vm, promise, "state", Value::from_i32(state));
    set_hidden(vm, promise, "value", value);
    // Run every handler that was attached while pending, as microtasks.
    let handlers = get_hidden(vm, promise, "handlers");
    if handlers.is_array() {
        let list = unsafe { handlers.as_array() }.data.clone();
        for h in list {
            if h.is_array() {
                let parts = unsafe { h.as_array() }.data.clone();
                if parts.len() == 4 {
                    let run = vm.new_function_native("promise-job", n_promise_job, 0);
                    let job = bind_native(vm, run, Value::UNDEFINED, parts);
                    vm.queue_microtask(job, Vec::new());
                }
            }
        }
        let empty = vm.new_array();
        set_hidden(vm, promise, "handlers", empty);
    }
    Ok(())
}

native!(n_promise_fulfil, |vm, this, args| {
    settle(vm, this, 1, arg(args, 0)).map(|_| Value::UNDEFINED)
});
native!(n_promise_reject_ref, |vm, this, args| {
    settle(vm, this, 2, arg(args, 0)).map(|_| Value::UNDEFINED)
});

native!(n_promise_ctor, |vm, _this, args| {
    let executor = arg(args, 0);
    if !executor.is_function() {
        return Err(type_error("Promise resolver is not a function"));
    }
    let p = new_pending_promise(vm);
    let fulfil = vm.new_function_native("resolve", n_promise_fulfil, 1);
    let reject = vm.new_function_native("reject", n_promise_reject_ref, 1);
    let resolve_fn = bind_native(vm, fulfil, p, Vec::new());
    let reject_fn = bind_native(vm, reject, p, Vec::new());
    if let Err(e) = vm.call_direct_args(executor, Value::UNDEFINED, &[resolve_fn, reject_fn]) {
        if let crate::errors::VmError::Exception(thrown) = &e {
            settle(vm, p, 2, thrown.value)?;
        }
    }
    Ok(p)
});

/// Run one `then` job: `bound = [promise, onFulfilled, onRejected, derived]`.
native!(n_promise_job, |vm, _this, args| {
    let p = arg(args, 0);
    let on_f = arg(args, 1);
    let on_r = arg(args, 2);
    let derived = arg(args, 3);
    let state = get_hidden(vm, p, "state").as_i64();
    let value = get_hidden(vm, p, "value");
    let handler = if state == 1 { on_f } else { on_r };
    if !handler.is_function() {
        // No handler for this outcome: the derived promise inherits it.
        return settle(vm, derived, state as i32, value).map(|_| Value::UNDEFINED);
    }
    match vm.call_direct_args(handler, Value::UNDEFINED, &[value]) {
        Ok(v) => resolve_with(vm, derived, v).map(|_| Value::UNDEFINED),
        Err(crate::errors::VmError::Exception(thrown)) => {
            settle(vm, derived, 2, thrown.value)?;
            Ok(Value::UNDEFINED)
        }
        Err(e) => Err(e),
    }
});

/// Resolve `p` with `v`, assimilating a thenable.
fn resolve_with(vm: &mut Vm, p: Value, v: Value) -> VmResult<()> {
    if v.is_object() {
        let then_key = vm.arena().intern_sym("then");
        let then = vm.get_prop(v, PropKey::Sym(then_key)).unwrap_or(Value::UNDEFINED);
        if then.is_function() {
            // The spec absorbs a thenable in a *job*, not inline: that extra
            // microtask is observable, and calling `then` synchronously moved every
            // subsequent `.then` one turn earlier than node.
            let assimilate = vm.new_function_native("promise-assimilate", n_promise_assimilate, 0);
            let job = bind_native(vm, assimilate, Value::UNDEFINED, vec![p, then, v]);
            vm.queue_microtask(job, Vec::new());
            return Ok(());
        }
    }
    settle(vm, p, 1, v)
}

/// `bound = [promise, then, thenable]`: call the thenable's `then` with our
/// resolve/reject pair, as a microtask.
native!(n_promise_assimilate, |vm, _this, args| {
    let p = arg(args, 0);
    let then = arg(args, 1);
    let thenable = arg(args, 2);
    let fulfil = vm.new_function_native("resolve", n_promise_fulfil, 1);
    let reject = vm.new_function_native("reject", n_promise_reject_ref, 1);
    let rf = bind_native(vm, fulfil, p, Vec::new());
    let rj = bind_native(vm, reject, p, Vec::new());
    match vm.call_direct_args(then, thenable, &[rf, rj]) {
        Ok(_) => Ok(Value::UNDEFINED),
        Err(crate::errors::VmError::Exception(thrown)) => {
            settle(vm, p, 2, thrown.value)?;
            Ok(Value::UNDEFINED)
        }
        Err(e) => Err(e),
    }
});

native!(n_promise_then, |vm, this, args| {
    let derived = new_pending_promise(vm);
    let on_f = arg(args, 0);
    let on_r = arg(args, 1);
    let state = get_hidden(vm, this, "state").as_i64();
    if state == 0 {
        let handlers = get_hidden(vm, this, "handlers");
        let entry = vm.new_array();
        for v in [this, on_f, on_r, derived] {
            unsafe { entry.as_array_mut() }.push(v);
        }
        if handlers.is_array() {
            unsafe { handlers.as_array_mut() }.push(entry);
        }
    } else {
        let run = vm.new_function_native("promise-job", n_promise_job, 0);
        let job = bind_native(vm, run, Value::UNDEFINED, vec![this, on_f, on_r, derived]);
        vm.queue_microtask(job, Vec::new());
    }
    Ok(derived)
});

native!(n_promise_catch, |vm, this, args| {
    let on_r = arg(args, 0);
    let derived = new_pending_promise(vm);
    let state = get_hidden(vm, this, "state").as_i64();
    if state == 0 {
        let handlers = get_hidden(vm, this, "handlers");
        let entry = vm.new_array();
        for v in [this, Value::UNDEFINED, on_r, derived] {
            unsafe { entry.as_array_mut() }.push(v);
        }
        if handlers.is_array() {
            unsafe { handlers.as_array_mut() }.push(entry);
        }
    } else if state == 2 {
        let run = vm.new_function_native("promise-job", n_promise_job, 0);
        let job = bind_native(vm, run, Value::UNDEFINED, vec![this, Value::UNDEFINED, on_r, derived]);
        vm.queue_microtask(job, Vec::new());
    } else {
        let v = get_hidden(vm, this, "value");
        settle(vm, derived, 1, v)?;
    }
    Ok(derived)
});

native!(n_promise_resolve_static, |vm, _this, args| {
    let v = arg(args, 0);
    // A promise resolves to itself.
    if v.is_object() {
        // `get_prop` returns Ok(undefined) for a missing key, so "is there a
        // state slot" must check the value, not the Result — otherwise every
        // object looked like a promise and `Promise.resolve(obj)` returned the
        // object, whose own `then` then ran inline.
        let k = promise_hidden_key(vm, "state");
        if matches!(vm.get_prop(v, k), Ok(state) if !state.is_undefined()) {
            return Ok(v);
        }
    }
    let p = new_pending_promise(vm);
    resolve_with(vm, p, v)?;
    Ok(p)
});

native!(n_promise_reject_static, |vm, _this, args| {
    let p = new_pending_promise(vm);
    settle(vm, p, 2, arg(args, 0))?;
    Ok(p)
});
