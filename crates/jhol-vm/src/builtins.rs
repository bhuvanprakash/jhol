//! JS-visible builtins: the global object graph and everything on it.
//!
//! Native functions are plain `fn` pointers (no captures), which is what lets a
//! builtin live in an arena cell that outlives any Rust frame. Where the engine
//! needs a runtime helper that JS never names directly (`__jhol_apply`,
//! `__jhol_enum_keys`, …) it is installed under a prefixed global.

use std::collections::HashMap;

use crate::arena::{self, Arena};
use crate::bytecode::FnProto;
use crate::errors::{throw_string, type_error, VmError, VmResult};
use crate::objects::*;
use crate::value::{f64_to_js_string, Value};
use crate::vm::{cancel_timer, push_timer, take_due_timers, TimerEntry, TimerKind, Vm};

macro_rules! native {
    ($name:ident, |$vm:ident, $this:ident, $args:ident| $body:expr) => {
        pub fn $name($vm: &mut Vm, $this: Value, $args: &[Value]) -> VmResult<Value> {
            $body
        }
    };
}

#[inline]
fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::UNDEFINED)
}

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

fn as_array<'a>(vm: &'a mut Vm, v: Value) -> VmResult<&'a mut JArray> {
    if v.is_array() {
        Ok(unsafe { v.as_array_mut() })
    } else {
        Err(type_error("expected an array"))
    }
}

/// Read `this` as an array, throwing a JS-visible TypeError when it is not one.
fn this_array(vm: &mut Vm, this: Value) -> VmResult<&'static mut JArray> {
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
        let src = if src.is_array() { src } else { vm.get_prop(v, kp2).unwrap_or(Value::UNDEFINED) };
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
    if value.is_string() { Ok(unsafe { value.as_string() }.code_units().collect()) }
    else { Ok(vm.to_string(value)?.encode_utf16().collect()) }
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
    let fn_proto = Value::from_ptr(a.alloc_function(JFunction::native(Value::UNDEFINED, n_noop, Value::NULL)));
    // An object, not an array: methods live on the prototype property bag.
    let arr_proto = Value::from_ptr(a.alloc_object(0, Value::NULL));
    unsafe { arr_proto.as_object_mut() }.hdr.flags |= FLAG_HIDDEN_KEYS;
    let str_proto = Value::from_ptr(a.alloc_object(0, Value::NULL));
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
    method(vm, arr_proto, "push", n_push);
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
    method(vm, arr_proto, "toString", n_array_to_string);
    method(vm, arr_proto, ITER_SYM_KEY, |vm, this, _args| Ok(make_iter_from(vm, this)));

    // ---- String.prototype ----
    method(vm, str_proto, "charAt", n_char_at);
    method(vm, str_proto, "charCodeAt", n_char_code_at);
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
    method(vm, str_proto, "replaceAll", n_replace_all);
    method(vm, str_proto, "repeat", n_repeat);
    method(vm, str_proto, "padStart", n_pad_start);
    method(vm, str_proto, "padEnd", n_pad_end);
    method(vm, str_proto, "concat", n_concat_str);
    method(vm, str_proto, "toString", n_str_to_string);
    method(vm, str_proto, "valueOf", n_str_to_string);
    method(vm, str_proto, "normalize", n_str_normalize);
    method(vm, str_proto, "localeCompare", n_locale_compare);
    method(vm, str_proto, ITER_SYM_KEY, |vm, this, _args| Ok(make_iter_from(vm, this)));

    // ---- constructors ----
    let obj_ctor = vm.new_function_native("Object", n_object_ctor, 1);
    link(vm, obj_ctor, obj_proto);
    set_fn(vm, obj_ctor, "keys", n_object_keys);
    set_fn(vm, obj_ctor, "values", n_object_values);
    set_fn(vm, obj_ctor, "entries", n_object_entries);
    set_fn(vm, obj_ctor, "assign", n_object_assign);
    set_fn(vm, obj_ctor, "freeze", n_object_freeze);
    set_fn(vm, obj_ctor, "isFrozen", n_object_is_frozen);
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
    set_fn(vm, str_ctor, "fromCharCode", n_from_char_code);
    set_fn(vm, str_ctor, "fromCodePoint", n_from_code_point);
    globals_def(vm, "String", str_ctor);

    let num_ctor = vm.new_function_native("Number", n_number_ctor, 1);
    let num_proto = Value::from_ptr(arena::active().alloc_object(0, Value::NULL));
    unsafe { num_proto.as_object_mut() }.hdr.flags |= FLAG_HIDDEN_KEYS;
    link(vm, num_ctor, num_proto);
    method(vm, num_proto, "toFixed", n_to_fixed);
    method(vm, num_proto, "toString", n_number_to_string);
    method(vm, num_proto, "valueOf", n_number_value_of);
    set_fn(vm, num_ctor, "isInteger", n_number_is_integer);
    set_fn(vm, num_ctor, "isFinite", n_number_is_finite);
    set_fn(vm, num_ctor, "isNaN", n_number_is_nan);
    set_fn(vm, num_ctor, "isSafeInteger", n_number_is_safe_integer);
    set_fn(vm, num_ctor, "parseFloat", n_parse_float);
    set_fn(vm, num_ctor, "parseInt", n_parse_int);
    let k = key(vm, "MAX_SAFE_INTEGER");
    vm.set_prop(num_ctor, k, Value::from_f64(9007199254740991.0)).unwrap();
    let k = key(vm, "MIN_SAFE_INTEGER");
    vm.set_prop(num_ctor, k, Value::from_f64(-9007199254740991.0)).unwrap();
    let k = key(vm, "MAX_VALUE");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::MAX)).unwrap();
    let k = key(vm, "MIN_VALUE");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::MIN_POSITIVE)).unwrap();
    let k = key(vm, "EPSILON");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::EPSILON)).unwrap();
    let k = key(vm, "POSITIVE_INFINITY");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::INFINITY)).unwrap();
    let k = key(vm, "NEGATIVE_INFINITY");
    vm.set_prop(num_ctor, k, Value::from_f64(f64::NEG_INFINITY)).unwrap();
    let k = key(vm, "NaN");
    vm.set_prop(num_ctor, k, Value::NAN).unwrap();
    globals_def(vm, "Number", num_ctor);
    vm.set_number_proto(num_proto);

    let bool_ctor = vm.new_function_native("Boolean", n_bool_ctor, 1);
    let bool_proto = Value::from_ptr(arena::active().alloc_object(0, Value::NULL));
    link(vm, bool_ctor, bool_proto);
    method(vm, bool_proto, "toString", n_bool_to_string);
    method(vm, bool_proto, "valueOf", n_bool_value_of);
    globals_def(vm, "Boolean", bool_ctor);

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
    gcd(vm, "parseInt", n_parse_int);
    gcd(vm, "parseFloat", n_parse_float);
    gcd(vm, "isNaN", n_global_is_nan);
    gcd(vm, "isFinite", n_global_is_finite);
    gcd(vm, "setTimeout", n_set_timeout);
    gcd(vm, "setInterval", n_set_interval);
    gcd(vm, "clearTimeout", n_clear_timer);
    gcd(vm, "clearInterval", n_clear_timer);

    gcd(vm, "__jhol_push", n_h_push);
    gcd(vm, "__jhol_push_all", n_h_push_all);
    gcd(vm, "__jhol_apply", n_h_apply);
    gcd(vm, "__jhol_apply_new", n_h_apply_new);
    gcd(vm, "__jhol_tonum", n_h_tonum);
    gcd(vm, "__jhol_enum_keys", n_h_enum_keys);
    gcd(vm, "__jhol_values", n_h_values);
    gcd(vm, "__jhol_rest_object", n_h_rest_object);
    gcd(vm, "__jhol_assign_all", n_h_assign_all);
    gcd(vm, "__jhol_new_regexp", n_h_new_regexp);
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
    for (name, ctor) in [
        ("TypeError", "TypeError"),
        ("RangeError", "RangeError"),
        ("SyntaxError", "SyntaxError"),
        ("ReferenceError", "ReferenceError"),
    ] {
        let sub = make_error_sub(vm, err_proto, name);
        globals_def(vm, name, sub);
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
    m(vm, map_proto, ITER_SYM_KEY, |vm, this, _args| Ok(make_iter_from(vm, this)));
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
    m(vm, set_proto, ITER_SYM_KEY, |vm, this, _args| Ok(make_iter_from(vm, this)));
    install_size_getter(vm, set_proto, n_set_size);
    globals_def(vm, "Set", set_ctor);
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

fn method(vm: &mut Vm, obj: Value, name: &str, f: NativeFn) {
    let v = vm.new_function_native(name, f, 0);
    let k = key(vm, name);
    vm.define_prop(obj, k, v);
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
native!(n_proto_get, |vm, _this, _args| {
    let _ = vm;
    Ok(_this)
});

native!(n_proto_set, |_vm, _this, _args| {
    Ok(_this)
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

fn err(vm: &mut Vm) -> Value {
    Value::UNDEFINED
}

native!(n_noop, |_vm, _this, _args| Ok(Value::UNDEFINED));
native!(n_obj_value_of, |_vm, this, _args| Ok(this));
native!(n_symbol_ctor, |vm, _this, args| {
    let d = to_str(vm, arg(args, 0)).unwrap_or_default();
    Ok(vm.new_string(&format!("Symbol({})", d)))
});
native!(n_is_enumerable, |vm, this, args| {
    let k = keyv(vm, arg(args, 0));
    if this.is_object() {
        return Ok(Value::from_bool(unsafe { this.as_object() }.find_index(k).is_some()));
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
        _ => false,
    };
    Ok(Value::from_bool(r))
});
native!(n_obj_to_string, |_vm, this, _args| {
    let tag = unsafe { this.obj_tag() };
    let n = match tag {
        Some(TAG_ARRAY) => "Array",
        Some(TAG_FUNCTION) => "Function",
        Some(TAG_OBJECT) => "Object",
        _ => "Object",
    };
    let s = format!("[object {}]", n);
    Ok(_vm.new_string(&s))
});
native!(n_is_prototype_of, |vm, this, args| {
    let o = arg(args, 0);
    let mut cur = o;
    let mut guard = 0;
    while cur.is_object() && guard < 128 {
        let p = unsafe { cur.as_object() }.proto;
        if p.bits() == this.bits() {
            return Ok(Value::TRUE);
        }
        cur = p;
        guard += 1;
    }
    Ok(Value::from_bool(false))
});

fn own_keys_of(vm: &mut Vm, v: Value) -> Vec<PropKey> {
    match unsafe { v.obj_tag() } {
        Some(TAG_OBJECT) => unsafe { v.as_object() }.own_keys(),
        Some(TAG_FUNCTION) => unsafe { v.as_function() }.keys.clone(),
        Some(TAG_ARRAY) => unsafe { v.as_array().own_keys() }
            .into_iter()
            .map(PropKey::Idx)
            .collect(),
        _ => Vec::new(),
    }
}

native!(n_object_keys, |vm, _this, args| {
    let v = arg(args, 0);
    let keys = own_keys_of(vm, v);
    let out = vm.new_array();
    for k in keys {
        let s = vm.new_string(&vm.prop_name(k));
        unsafe { out.as_array_mut().push(s) };
    }
    Ok(out)
});
native!(n_object_values, |vm, _this, args| {
    let v = arg(args, 0);
    let keys = own_keys_of(vm, v);
    let out = vm.new_array();
    for k in keys {
        let val = match unsafe { v.obj_tag() } {
            Some(TAG_OBJECT) => unsafe { v.as_object() }.get(k).unwrap_or(Value::UNDEFINED),
            Some(TAG_ARRAY) => unsafe { v.as_array() }.get(match k { PropKey::Idx(i) => i, _ => 0 }),
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
            Some(TAG_ARRAY) => unsafe { v.as_array() }.get(match k { PropKey::Idx(i) => i, _ => 0 }),
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
native!(n_get_proto_of, |vm, _this, args| Ok(vm.proto_of(arg(args, 0))));
native!(n_set_proto_of, |vm, _this, args| {
    let o = arg(args, 0);
    let p = arg(args, 1);
    if o.is_object() {
        unsafe { o.as_object_mut() }.proto = p;
    } else if o.is_array() {
        unsafe { o.as_array_mut() }.proto = p;
    }
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
            let gk = arena::active().accessor_sym(match k { PropKey::Sym(s) => s, _ => 0 }, "#get:");
            vm.define_prop(target, PropKey::Sym(gk), getter);
        }
        if setter.is_function() {
            let sk = arena::active().accessor_sym(match k { PropKey::Sym(s) => s, _ => 0 }, "#set:");
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
    let keys = own_keys_of(vm, v);
    let out = vm.new_array();
    for k in keys {
        let s = vm.new_string(&vm.prop_name(k));
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
    vm.call_direct_args(this, recv, &args[1.min(args.len())..])
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
    vm.call_direct_args(this, recv, &vals)
});
native!(n_fn_bind, |vm, this, args| {
    let recv = arg(args, 0);
    let mut bound = unsafe { this.as_function() }.bound.clone();
    bound.extend_from_slice(&args[1.min(args.len())..]);
    let f = JFunction::native(Value::UNDEFINED, n_noop, Value::UNDEFINED);
    let fp = vm.arena().alloc_function(f);
    let v = Value::from_ptr(fp);
    unsafe {
        (*fp).hdr.flags |= FLAG_BOUND;
        (*fp).bound = bound;
        (*fp).this_val = this;
        (*fp).proto = Value::from_ptr(vm.proto_function_ptr() as *mut JFunction);
        (*fp).proto_obj = unsafe { this.as_function() }.proto_obj;
    }
    let _ = recv;
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
        let res = vm.call_direct_args(next, iter, &[])?;
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
native!(n_is_array, |_vm, _this, args| Ok(Value::from_bool(arg(args, 0).is_array())));
native!(n_array_from, |vm, _this, args| {
    let src = arg(args, 0);
    let mut vals = collect_iterable(vm, src)?;
    let mapfn = arg(args, 1);
    if mapfn.is_function() {
        for (i, v) in vals.iter_mut().enumerate() {
            let idx = Value::from_i32(i as i32);
            let r = vm.call_direct_args(mapfn, Value::UNDEFINED, &[*v, idx])?;
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
    drop(a);
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
        return vm.call_direct_args(j, this, &[]);
    }
    Ok(vm.new_string(""))
});
native!(n_index_of, |vm, this, args| {
    let a = this_array(vm, this)?;
    let needle = arg(args, 0);
    let from = if args.len() > 1 { norm_index(a.len as usize, arg(args, 1).as_i64()) } else { 0 };
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
        let v = a.get(i);
        let idx = Value::from_i32(i as i32);
        if vm.call_direct_args(f, Value::UNDEFINED, &[v, idx, this])?.to_bool() {
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
        let v = a.get(i as u32);
        let idx = Value::from_i32(i as i32);
        if vm.call_direct_args(f, Value::UNDEFINED, &[v, idx, this])?.to_bool() {
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
        let v = a.get(i);
        let idx = Value::from_i32(i as i32);
        if vm.call_direct_args(f, Value::UNDEFINED, &[v, idx, this])?.to_bool() {
            return Ok(idx);
        }
    }
    Ok(Value::from_i32(-1))
});
native!(n_filter, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    let out = vm.new_array();
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        if vm.call_direct_args(f, Value::UNDEFINED, &[*v, idx, this])?.to_bool() {
            unsafe { out.as_array_mut().push(*v) };
        }
    }
    Ok(out)
});
native!(n_map, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    let out = vm.new_array();
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        let r = vm.call_direct_args(f, Value::UNDEFINED, &[*v, idx, this])?;
        unsafe { out.as_array_mut().push(r) };
    }
    Ok(out)
});
native!(n_flat_map, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    let out = vm.new_array();
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        let r = vm.call_direct_args(f, Value::UNDEFINED, &[*v, idx, this])?;
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
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        vm.call_direct_args(f, Value::UNDEFINED, &[*v, idx, this])?;
    }
    Ok(Value::UNDEFINED)
});
native!(n_reduce, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    let mut acc;
    let mut start = 0usize;
    if args.len() > 1 {
        acc = arg(args, 1);
    } else {
        if src.is_empty() {
            return Err(type_error("Reduce of empty array with no initial value"));
        }
        acc = src[0];
        start = 1;
    }
    for i in start..src.len() {
        let idx = Value::from_i32(i as i32);
        acc = vm.call_direct_args(f, Value::UNDEFINED, &[acc, src[i], idx, this])?;
    }
    Ok(acc)
});
native!(n_reduce_right, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
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
        acc = vm.call_direct_args(f, Value::UNDEFINED, &[acc, src[i as usize], idx, this])?;
        i -= 1;
    }
    Ok(acc)
});
native!(n_some, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        if vm.call_direct_args(f, Value::UNDEFINED, &[*v, idx, this])?.to_bool() {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::from_bool(false))
});
native!(n_every, |vm, this, args| {
    let f = arg(args, 0);
    let a = this_array(vm, this)?;
    let src: Vec<Value> = a.data.clone();
    for (i, v) in src.iter().enumerate() {
        let idx = Value::from_i32(i as i32);
        if !vm.call_direct_args(f, Value::UNDEFINED, &[*v, idx, this])?.to_bool() {
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
    let depth = if args.is_empty() { 1 } else { arg(args, 0).as_i64().max(0) as usize };
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
    if f.is_function() {
        // Bottom-up merge sort: O(n log n) comparisons against a callback that
        // can run arbitrary JS, and stable, which is what `Array#sort` promises.
        // An insertion sort here made a 100k-element sort take minutes.
        let n = data.len();
        let mut buf = data.clone();
        let mut width = 1usize;
        while width < n {
            let mut lo = 0usize;
            while lo < n {
                let mid = (lo + width).min(n);
                let hi = (lo + 2 * width).min(n);
                if mid < hi {
                    merge_run(vm, &mut data, &mut buf, lo, mid, hi, f)?;
                }
                lo += 2 * width;
            }
            width *= 2;
        }
    } else {
        let mut strs: Vec<(String, Value)> = data
            .iter()
            .map(|v| (vm.to_string(*v).unwrap_or_default(), *v))
            .collect();
        strs.sort_by(|x, y| x.0.cmp(&y.0));
        data = strs.into_iter().map(|(_, v)| v).collect();
    }
    a.data = data;
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
) -> VmResult<()> {
    let mut i = lo;
    let mut j = mid;
    let mut k = lo;
    while i < mid && j < hi {
        let a = data[i];
        let b = data[j];
        let r = vm.call_direct_args(f, Value::UNDEFINED, &[a, b])?;
        if vm.to_number(r)? > 0.0 {
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

native!(n_string_ctor, |vm, _this, args| {
    let v = arg(args, 0);
    if v.is_string() { return Ok(v); }
    if v.is_nil() {
        return Ok(vm.new_string(""));
    }
    let __s = vm.to_string(v)?;
    Ok(vm.new_string(&__s))
});
native!(n_from_char_code, |vm, _this, args| {
    let units = args.iter().map(|&v| vm.to_uint32(v).map(|n| n as u16)).collect::<VmResult<Vec<_>>>()?;
    Ok(vm.new_string_utf16(&units))
});
native!(n_from_code_point, |vm, _this, args| {
    let mut units = Vec::new();
    for &v in args {
        let n = vm.to_number(v)?;
        if !n.is_finite() || n.fract() != 0.0 || !(0.0..=0x10ffff as f64).contains(&n) {
            return Err(crate::errors::range_error("Invalid code point"));
        }
        if n <= 65535.0 { units.push(n as u16); }
        else { let n = n as u32 - 65536; units.push(0xd800 + (n >> 10) as u16); units.push(0xdc00 + (n & 1023) as u16); }
    }
    Ok(vm.new_string_utf16(&units))
});
native!(n_char_at, |vm, this, args| {
    let i = string_index(vm, arg(args, 0))?;
    let units = string_units(vm, this)?;
    Ok(vm.new_string_utf16(if i < 0 { &[] } else { units.get(i as usize..i as usize + 1).unwrap_or(&[]) }))
});
native!(n_str_at, |vm, this, args| {
    let units = string_units(vm, this)?;
    let i = string_index(vm, arg(args, 0))?;
    let i = if i < 0 { units.len() as i64 + i } else { i };
    if i < 0 || i >= units.len() as i64 { return Ok(Value::UNDEFINED); }
    Ok(vm.new_string_utf16(&units[i as usize..i as usize + 1]))
});
native!(n_char_code_at, |vm, this, args| {
    let i = string_index(vm, arg(args, 0))?;
    let unit = if i < 0 { None } else if this.is_string() {
        unsafe { this.as_string() }.code_unit(i as usize)
    } else { string_units(vm, this)?.get(i as usize).copied() };
    Ok(unit.map_or(Value::NAN, |u| Value::from_i32(u as i32)))
});
native!(n_code_point_at, |vm, this, args| {
    let i = string_index(vm, arg(args, 0))?;
    let units = string_units(vm, this)?;
    let Some(&first) = (if i < 0 { None } else { units.get(i as usize) }) else { return Ok(Value::UNDEFINED); };
    let second = units.get(i as usize + 1).copied().unwrap_or(0);
    let cp = if (0xd800..=0xdbff).contains(&first) && (0xdc00..=0xdfff).contains(&second) {
        0x10000 + ((first as u32 - 0xd800) << 10) + second as u32 - 0xdc00
    } else { first as u32 };
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
    match rest.windows(needle.len()).position(|w| w == needle.as_bytes()) {
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
    let from = if args.len() > 1 { arg(args, 1).as_i64().max(0) as usize } else { 0 };
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
    let pos = if args.len() > 1 { arg(args, 1).as_i64().max(0) as usize } else { 0 };
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
    let st = if start < 0 { (n + start).max(0) } else { start.min(n) };
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
native!(n_replace, |vm, this, args| {
    let s = this_str(vm, this)?;
    let pat = to_str(vm, arg(args, 0))?;
    let rep = arg(args, 1);
    // `replace` swaps only the *first* match (unlike `replaceAll`).
    let r = if rep.is_function() {
        let pv = vm.new_string(&pat);
        let sub = vm.call_direct_args(rep, Value::UNDEFINED, &[pv])?;
        let rep_s = to_str(vm, sub)?;
        s.replacen(&pat, &rep_s, 1)
    } else {
        s.replacen(&pat, &to_str(vm, rep)?, 1)
    };
    Ok(vm.new_string(&r))
});

native!(n_replace_all, |vm, this, args| {
    let s = this_str(vm, this)?;
    let pat = to_str(vm, arg(args, 0))?;
    let rep = to_str(vm, arg(args, 1))?;
    Ok(vm.new_string(&replace_all_str(&s, &pat, &rep)))
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
    let mut out = String::new();
    let mut i = 0;
    while out.chars().count() < need {
        out.push(pad.chars().nth(i % pad.chars().count()).unwrap_or(' '));
        i += 1;
    }
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
    let mut out = s.clone();
    let mut i = 0;
    while out.chars().count() < target {
        out.push(pad.chars().nth(i % pad.chars().count()).unwrap_or(' '));
        i += 1;
    }
    let _ = need;
    Ok(vm.new_string(&out))
});

// ---------------------------------------------------------------------------
// Number / Math / JSON / misc
// ---------------------------------------------------------------------------

native!(n_number_ctor, |vm, _this, args| {
    let v = arg(args, 0);
    let n = if v.is_undefined() || v.is_null() {
        0.0
    } else {
        vm.to_number(v)?
    };
    Ok(Value::from_f64(n))
});
native!(n_bool_ctor, |vm, _this, args| Ok(Value::from_bool(arg(args, 0).to_bool())));
native!(n_bool_to_string, |vm, this, _args| Ok(vm.new_string(if this.as_bool() { "true" } else { "false" })));
native!(n_bool_value_of, |_vm, this, _args| Ok(this));
native!(n_number_value_of, |_vm, this, _args| Ok(Value::from_f64(this.as_f64())));
native!(n_number_is_integer, |vm, _this, args| {
    let v = arg(args, 0);
    Ok(Value::from_bool(v.is_number() && v.as_f64().fract() == 0.0 && v.as_f64().is_finite()))
});
native!(n_number_is_finite, |_vm, _this, args| {
    let v = arg(args, 0);
    Ok(Value::from_bool(v.is_number() && v.as_f64().is_finite()))
});
native!(n_number_is_nan, |_vm, _this, args| Ok(Value::from_bool(arg(args, 0).is_nan())));
native!(n_number_is_safe_integer, |_vm, _this, args| {
    let v = arg(args, 0);
    let n = v.as_f64();
    Ok(Value::from_bool(v.is_number() && n.fract() == 0.0 && n.abs() <= 9007199254740991.0))
});
native!(n_to_fixed, |_vm, this, args| {
    let d = arg(args, 0).as_i64().clamp(0, 100) as usize;
    Ok(_vm.new_string(&format!("{:.*}", d, this.as_f64())))
});
native!(n_number_to_string, |_vm, this, args| {
    let radix = arg(args, 0).as_i64();
    if radix == 10 || radix == 0 {
        Ok(_vm.new_string(&f64_to_js_string(this.as_f64())))
    } else {
        let n = this.as_f64() as i64;
        Ok(_vm.new_string(&to_radix(n, radix as u32)))
    }
});
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
    let r = if x >= 0.0 {
        (x + 0.5).floor()
    } else {
        -((-x) + 0.5).floor()
    };
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
    let mut out = String::new();
    json_stringify(vm, v, &space, 0, &mut out)?;
    Ok(vm.new_string(&out))
});
fn json_stringify(vm: &mut Vm, v: Value, space: &str, depth: usize, out: &mut String) -> VmResult<()> {
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
            let keys = unsafe { v.as_object() }.own_keys();
            let pairs: Vec<(PropKey, Value)> = keys
                .iter()
                .filter_map(|k| unsafe { v.as_object() }.get(*k).map(|val| (*k, val)))
                .collect();
            if pairs.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            for (i, (k, val)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                nl(depth + 1, space, out);
                let ks = vm.prop_name(*k);
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
                if string.ascii != 0 { json_escape(unsafe { string.as_str() }, out); }
                else {
                    for c in char::decode_utf16(string.code_units()) {
                        match c {
                            Ok(c) => { let mut buf = [0; 4]; json_escape(c.encode_utf8(&mut buf), out); }
                            Err(e) => { use std::fmt::Write; let _ = write!(out, "\\u{:04x}", e.unpaired_surrogate()); }
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
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}
native!(n_json_parse, |vm, _this, args| {
    let s = to_str(vm, arg(args, 0))?;
    let mut chars = s.chars().peekable();
    let v = json_parse_value(vm, &mut chars)?;
    Ok(v)
});
fn json_parse_value(
    vm: &mut Vm,
    it: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> VmResult<Value> {
    skip_ws(it);
    let c = match it.peek() {
        Some(c) => *c,
        None => return Err(type_error("Unexpected end of JSON input")),
    };
    match c {
        '{' => {
            it.next();
            let o = vm.new_object();
            skip_ws(it);
            if it.peek() == Some(&'}') {
                it.next();
                return Ok(o);
            }
            loop {
                skip_ws(it);
                let k = match it.next() {
                    Some('"') => json_parse_string(it),
                    _ => return Err(type_error("Unexpected token in JSON")),
                };
                skip_ws(it);
                if it.next() != Some(':') {
                    return Err(type_error("Expected ':' in JSON"));
                }
                let v = json_parse_value(vm, it)?;
                let name = String::from_utf16_lossy(&k);
                let kk = key(vm, &name);
                vm.set_prop(o, kk, v).unwrap();
                skip_ws(it);
                match it.next() {
                    Some(',') => continue,
                    Some('}') => break,
                    _ => return Err(type_error("Expected ',' or '}' in JSON")),
                }
            }
            Ok(o)
        }
        '[' => {
            it.next();
            let a = vm.new_array();
            skip_ws(it);
            if it.peek() == Some(&']') {
                it.next();
                return Ok(a);
            }
            loop {
                let v = json_parse_value(vm, it)?;
                unsafe { a.as_array_mut().push(v) };
                skip_ws(it);
                match it.next() {
                    Some(',') => continue,
                    Some(']') => break,
                    _ => return Err(type_error("Expected ',' or ']' in JSON")),
                }
            }
            Ok(a)
        }
        '"' => {
            it.next();
            let s = json_parse_string(it);
            Ok(vm.new_string_utf16(&s))
        }
        't' => {
            for _ in 0..4 {
                it.next();
            }
            Ok(Value::TRUE)
        }
        'f' => {
            for _ in 0..5 {
                it.next();
            }
            Ok(Value::FALSE)
        }
        'n' => {
            for _ in 0..4 {
                it.next();
            }
            Ok(Value::NULL)
        }
        _ => {
            let mut buf = String::new();
            while let Some(&c) = it.peek() {
                if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E' {
                    buf.push(c);
                    it.next();
                } else {
                    break;
                }
            }
            Ok(Value::from_f64(parse_float_like(&buf)))
        }
    }
}
fn skip_ws(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&c) = it.peek() {
        if c == ' ' || c == '\n' || c == '\t' || c == '\r' {
            it.next();
        } else {
            break;
        }
    }
}
fn json_parse_string(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Vec<u16> {
    let mut out = Vec::new();
    while let Some(c) = it.next() {
        match c {
            '"' => break,
            '\\' => match it.next() {
                Some('n') => out.push(10),
                Some('t') => out.push(9),
                Some('r') => out.push(13),
                Some('b') => out.push(8),
                Some('f') => out.push(12),
                Some('/') => out.push(47),
                Some('"') => out.push(34),
                Some('\\') => out.push(92),
                Some('u') => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        if let Some(h) = it.next() {
                            hex.push(h);
                        }
                    }
                    if let Ok(n) = u32::from_str_radix(&hex, 16) {
                        out.push(n as u16);
                    }
                }
                _ => {}
            },
            c => { let mut buf=[0u16;2]; out.extend_from_slice(c.encode_utf16(&mut buf)); },
        }
    }
    out
}

// ---------------------------------------------------------------------------
// console / process / timers
// ---------------------------------------------------------------------------

fn display(vm: &mut Vm, v: Value, depth: usize) -> VmResult<String> {
    if v.is_string() {
        return Ok(unsafe { v.as_string().as_str().to_string() });
    }
    if v.is_nan() {
        return Ok("NaN".into());
    }
    if v.is_smi() || v.is_double() {
        return Ok(f64_to_js_string(v.as_f64()));
    }
    if v.is_bool() {
        return Ok(if v.as_bool() { "true".into() } else { "false".into() });
    }
    if v.is_nil() {
        return Ok(if v.is_null() { "null".into() } else { "undefined".into() });
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
                parts.push(display(vm, item, depth + 1)?);
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
            parts.push(format!("{}: {}", vm.prop_name(k), display(vm, val, depth + 1)?));
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
    Err(crate::errors::VmError::Internal(crate::vm::exit_marker(code)))
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
    let d = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    Ok(vm.new_string(&d))
});

native!(n_set_timeout, |vm, _this, args| {
    let f = arg(args, 0);
    let ms = if args.len() > 1 { arg(args, 1).as_i64().max(0) as u64 } else { 0 };
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
    let ms = if args.len() > 1 { arg(args, 1).as_i64().max(1) as u64 } else { 1 };
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
    let o = if into.is_object() { into } else { Value::from_ptr(arena::active().alloc_object(0, proto)) };
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
    Ok(vm.new_string(&if m.is_empty() { n } else { format!("{}: {}", n, m) }))
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
    vm.call_direct_args(f, this, &vals)
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
native!(n_h_tonum, |vm, _this, args| Ok(Value::from_f64(vm.to_number(arg(args, 0))?)));
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
    let keys = own_keys_of(vm, src);
    for k in keys {
        let name = vm.prop_name(k);
        if exclude.contains(&name) {
            continue;
        }
        let val = vm.get_prop(src, k)?;
        vm.set_prop(out, k, val).unwrap();
    }
    Ok(out)
});
native!(n_h_assign_all, |vm, _this, args| {
    let dst = arg(args, 0);
    let src = arg(args, 1);
    let keys = own_keys_of(vm, src);
    for k in keys {
        let val = vm.get_prop(src, k)?;
        vm.set_prop(dst, k, val)?;
    }
    Ok(dst)
});
native!(n_h_new_regexp, |vm, _this, args| {
    let o = vm.new_object();
    let pat = to_str(vm, arg(args, 0))?;
    let flags = to_str(vm, arg(args, 1))?;
    set_obj_str(vm, o, "source", &pat);
    set_obj_str(vm, o, "flags", &flags);
    let k = key(vm, "lastIndex");
    vm.set_prop(o, k, Value::from_i32(0)).unwrap();
    Ok(o)
});
/// `__proto__` is the internal prototype slot, not a data property, so it needs
/// its own helper rather than `Object.defineProperty`.
native!(n_h_set_proto, |vm, _this, args| {
    let o = arg(args, 0);
    let p = arg(args, 1);
    if o.is_object() {
        unsafe { o.as_object_mut() }.proto = p;
    } else if o.is_array() {
        unsafe { o.as_array_mut() }.proto = p;
    }
    let _ = vm;
    Ok(o)
});
/// `__jhol_rest_array(v, from)` — the tail of `v` from `from` on, used by array
/// destructuring rest patterns.
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
native!(n_h_tagged_template, |vm, _this, args| {
    let strings = arg(args, 0);
    let s = to_str(vm, strings)?;
    Ok(vm.new_string(&s))
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
    for (k, v) in vals {
        vm.call_direct_args(f, Value::UNDEFINED, &[v, k, this])?;
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
    for v in vals {
        vm.call_direct_args(f, Value::UNDEFINED, &[v, v, this])?;
    }
    Ok(Value::UNDEFINED)
});
