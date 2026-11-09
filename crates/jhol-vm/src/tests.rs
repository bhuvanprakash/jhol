//! Unit tests for the engine's own primitives.
//!
//! These exercise the parts that are easy to break and hard to see from
//! JavaScript: the value encoding, the arena's block protocol, and the property
//! store's shape transitions.
//!
//! Each `Engine` installs an arena on its own thread, so every test runs in its
//! own thread to keep per-thread globals apart.

use crate::arena::Arena;
use crate::objects::{PropKey, TAG_FUNCTION};
use crate::value::{f64_to_js_string, Value};

fn in_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(f)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked")
}

fn eval_str(src: &str) -> String {
    let mut e = crate::vm::Engine::new();
    e.ensure_started();
    let v = e.eval_source(src, "test.js").expect("script threw");
    e.vm.to_string(v).expect("value has no string form")
}

fn eval(src: &str) -> Result<Value, crate::errors::VmError> {
    let mut e = crate::vm::Engine::new();
    e.ensure_started();
    e.eval_source(src, "test.js")
}

// ------------------------- value encoding -------------------------

#[test]
fn nan_boxing_round_trips_primitives() {
    in_thread(|| {
        let leaked = Arena::new();
        leaked.install();
        for n in [0i64, 1, -1, 42, 1 << 40, -(1 << 40)] {
            let v = Value::from_i64(n);
            assert!(v.is_smi(), "{} should be a Smi", n);
            assert_eq!(v.as_i64(), n);
            assert_eq!(v.as_f64(), n as f64);
            assert!(v.is_number());
        }
        for f in [0.5, -0.5, 1e300, f64::MAX, f64::MIN] {
            let v = Value::from_f64(f);
            assert!(v.is_double(), "{} should stay a double", f);
            assert_eq!(v.as_f64(), f);
        }
        // The reserved NaN must not be read as a box.
        assert!(Value::NAN.is_double());
        assert!(Value::NAN.is_number());
        assert!(!Value::NAN.is_ptr());
        // NaNs from arithmetic canonicalise to the same encoding.
        assert_eq!(Value::from_f64(f64::NAN), Value::from_f64(0.0 / 0.0));
        // Pointers land inside the arena and come back bit-identical.
        let s = Value::from_str_owned("abc");
        assert!(s.is_string());
        assert_eq!(s.as_str(), Some("abc"));
    });
}

#[test]
fn singletons_and_truthiness() {
    in_thread(|| {
        let leaked = Arena::new();
        leaked.install();
        assert!(Value::UNDEFINED.is_undefined());
        assert!(Value::NULL.is_null());
        assert!(Value::TRUE.as_bool());
        assert!(!Value::FALSE.as_bool());
        assert!(!Value::UNDEFINED.to_bool());
        assert!(!Value::NULL.to_bool());
        assert!(!Value::from_i64(0).to_bool());
        assert!(Value::from_i64(7).to_bool());
        assert!(!Value::NAN.to_bool());
        assert!(!Value::from_str_owned("").to_bool());
        assert!(Value::from_str_owned("x").to_bool());
        assert!(Value::TRUE.to_bool());
        assert!(!Value::FALSE.to_bool());

        assert_eq!(Value::from_i64(1).type_of(), "number");
        assert_eq!(Value::from_str_owned("s").type_of(), "string");
        assert_eq!(Value::TRUE.type_of(), "boolean");
        assert_eq!(Value::UNDEFINED.type_of(), "undefined");
        assert_eq!(Value::from_f64(1.0).type_of(), "number");
    });
}

#[test]
fn arithmetic_fast_paths() {
    in_thread(|| {
        let leaked = Arena::new();
        leaked.install();
        assert_eq!(
            Value::from_i64(2)
                .try_num_add(&Value::from_i64(3))
                .unwrap()
                .as_i64(),
            5
        );
        // Overflow out of the Smi range promotes to a double rather than wrapping.
        let big = Value::from_i64((1 << 43) - 1)
            .try_num_add(&Value::from_i64(1))
            .unwrap();
        assert!(big.is_double());
        assert_eq!(big.as_f64(), (1i64 << 43) as f64);
        assert!(Value::from_i64(1)
            .try_num_add(&Value::from_str_owned("a"))
            .is_none());
        assert_eq!(
            Value::from_f64(1.5)
                .try_num_add(&Value::from_f64(2.25))
                .unwrap()
                .as_f64(),
            3.75
        );
        let a = Value::from_i64(6);
        let b = Value::from_i64(7);
        assert_eq!(
            a.try_num_binop(&b, crate::value::ArithOp::Mul)
                .unwrap()
                .as_i64(),
            42
        );
    });
}

#[test]
fn number_formatting_matches_js() {
    in_thread(|| {
        let leaked = Arena::new();
        leaked.install();
        assert_eq!(f64_to_js_string(0.0), "0");
        assert_eq!(f64_to_js_string(-0.0), "0");
        assert_eq!(f64_to_js_string(3.5), "3.5");
        assert_eq!(f64_to_js_string(-2.0), "-2");
        assert_eq!(f64_to_js_string(1024.0), "1024");
        assert_eq!(f64_to_js_string(f64::NAN), "NaN");
        assert_eq!(f64_to_js_string(f64::INFINITY), "Infinity");
        assert_eq!(f64_to_js_string(f64::NEG_INFINITY), "-Infinity");
    });
}

// ----------------------------- arena ------------------------------

#[test]
fn arena_recycles_blocks_without_losing_the_walk() {
    in_thread(|| {
        let leaked = Arena::new();
        leaked.install();
        let a = crate::arena::active();
        let sym = a.intern_sym("k");
        let mut live: Vec<Value> = Vec::new();
        for i in 0..200u32 {
            let o = Value::from_ptr(a.alloc_object(0, Value::NULL));
            let shape = a.shape_for(unsafe { o.as_object() }.shape, PropKey::Sym(sym));
            {
                let om = unsafe { o.as_object_mut() };
                om.shape = shape;
                om.set(PropKey::Sym(sym), Value::from_i64(i as i64));
            }
            live.push(o);
        }
        // Object[key=...] in the same order always yields the same shape id.
        let s0 = unsafe { live[0].as_object() }.shape;
        let s199 = unsafe { live[199].as_object() }.shape;
        assert_eq!(s0, s199);
        assert_eq!(
            unsafe { live[199].as_object() }
                .get(PropKey::Sym(sym))
                .unwrap()
                .as_i64(),
            199
        );
        // Sweeping twice in a row must be idempotent.
        let roots: Vec<&[Value]> = vec![live.as_slice()];
        a.collect(&roots);
        let after = a.live_bytes();
        a.collect(&roots);
        assert_eq!(a.live_bytes(), after, "sweep must be idempotent");
        for (i, o) in live.iter().enumerate() {
            assert_eq!(
                unsafe { o.as_object() }
                    .get(PropKey::Sym(sym))
                    .unwrap()
                    .as_i64(),
                i as i64,
                "live objects survive a collection"
            );
        }
    });
}

#[test]
fn arena_symbol_and_accessor_interning() {
    in_thread(|| {
        let leaked = Arena::new();
        leaked.install();
        let a = crate::arena::active();
        let name = a.intern_sym("name");
        assert_eq!(name, a.intern_sym("name"), "interning is stable");
        assert_ne!(name, a.intern_sym("other"));
        let g = a.accessor_sym(name, "#get:");
        assert_eq!(g, a.accessor_sym(name, "#get:"));
        let s = a.accessor_sym(name, "#set:");
        assert_ne!(g, s);
        assert_eq!(a.sym_name(g), "#get:name");
        assert_eq!(a.sym_name(s), "#set:name");
    });
}

#[test]
fn objects_and_functions_carry_a_property_bag() {
    in_thread(|| {
        let mut e = crate::vm::Engine::new();
        e.ensure_started();
        let f =
            e.vm.new_function_native("probe", |_vm, _t, _a| Ok(Value::from_i64(7)), 0);
        assert!(f.is_function());
        assert_eq!(unsafe { f.obj_tag() }, Some(TAG_FUNCTION));
        let ks = e.vm.new_string("answer");
        let k = e.vm.key_from_value(ks);
        e.vm.define_prop(f, k, Value::from_i64(42));
        assert_eq!(e.vm.get_prop(f, k).expect("read from fn").as_i64(), 42);
    });
}

// -------------------------- JavaScript ----------------------------

// Hidden-issue sweep: cases where the engine looked right but differed from JS.
// Strictness is per function: sloppy functions coerce a missing receiver to the
// global object, class methods and "use strict" bodies keep `undefined`.
#[test]
fn parameter_defaults_fill_only_missing_arguments() {
    in_thread(|| {
        // The comparison loaded a constant *index* instead of a register, so a
        // default fired for a supplied argument and was skipped for a missing
        // one. Arrows were the visible case; plain functions were wrong too.
        assert_eq!(
            eval_str("(function(){ return JSON.stringify(((a,b=2)=>[a,b])(1)) })()"),
            "[1,2]"
        );
        assert_eq!(
            eval_str("(function(){ return JSON.stringify(((a=1,b=2)=>[a,b])(9)) })()"),
            "[9,2]"
        );
        assert_eq!(eval_str("(function(a=1,b=2){ return a+b })()"), "3");
        assert_eq!(eval_str("(function(a,b=2){ return a+b })(1)"), "3");
    });
}

#[test]
fn numeric_formatting_matches_js() {
    in_thread(|| {
        // toFixed rounds the exact decimal expansion: 2.55 is 2.54999… in binary.
        assert_eq!(
            eval_str("(2.55).toFixed(1) + ',' + (2.5).toFixed(0) + ',' + (1.005).toFixed(2)"),
            "2.5,3,1.00"
        );
        // fractional radix conversion
        assert_eq!(
            eval_str("(0.5).toString(2) + ',' + (0.25).toString(2) + ',' + (255).toString(16)"),
            "0.1,0.01,ff"
        );
    });
}

#[test]
fn this_binding_follows_function_strictness() {
    in_thread(|| {
        assert_eq!(
            eval_str("(function(){ return this === globalThis })()"),
            "true"
        );
        assert_eq!(
            eval_str("(function(){ const o={v:2,m(){return this.v}}; return o.m.call(null) })()"),
            "undefined"
        );
        assert_eq!(
            eval_str("(function(){ 'use strict'; return this === undefined })()"),
            "true"
        );
        // a class method is always strict, even when extracted
        assert_eq!(
            eval_str("(function(){ class A{m(){return this===undefined}} const f = new A().m; return f() })()"),
            "true"
        );
    });
}

#[test]
fn spread_and_rest_read_values_not_accessor_slots() {
    in_thread(|| {
        // a getter must be invoked and become a data property
        assert_eq!(
            eval_str("(function(){ const o={get a(){return 5}}; const s={...o}; return s.a + '|' + Object.keys(s).join() })()"),
            "5|a"
        );
        // rest excludes what the pattern bound
        assert_eq!(
            eval_str("(function(){ const o={a:1,b:2,c:3}; const {a,c,...r}=o; return Object.keys(r).join() })()"),
            "b"
        );
        assert_eq!(
            eval_str(
                "(function(){ const o={a:1}; const {a,...r}=o; return Object.keys(r).length })()"
            ),
            "0"
        );
    });
}

#[test]
fn json_stringify_calls_to_json() {
    in_thread(|| {
        assert_eq!(
            eval_str("JSON.stringify({d:{toJSON(){return 'T'}}})"),
            "{\"d\":\"T\"}"
        );
    });
}

#[test]
fn sparse_arrays_keep_holes_and_skip_them() {
    in_thread(|| {
        // map preserves the hole (join renders it empty), callbacks skip it,
        // reduce skips it, and arithmetic on a hole must not crash the VM (it
        // used to overflow the stack through `to_primitive`).
        assert_eq!(eval_str("[1,,3].map(x => x*2).join('-')"), "2--6");
        assert_eq!(eval_str("[1,,3].filter(x => true).length"), "2");
        assert_eq!(eval_str("[1,,3].reduce((s,x) => s+x, 0)"), "4");
        assert_eq!(
            eval_str("(function(){let n=0; [1,,3].forEach(()=>n++); return n})()"),
            "2"
        );
    });
}

#[test]
fn delete_on_an_array_index_leaves_a_hole() {
    in_thread(|| {
        assert_eq!(
            eval_str("(function(){const a=[1,2,3]; delete a[1]; return a.length + '|' + a.join('-') + '|' + Object.keys(a).join()})()"),
            "3|1--3|0,2"
        );
    });
}

#[test]
fn own_keys_order_puts_indices_first() {
    in_thread(|| {
        assert_eq!(
            eval_str("Object.keys({b:1,'2':2,a:3,'1':4}).join()"),
            "1,2,b,a"
        );
        assert_eq!(
            eval_str("Object.keys({o:1,'10':2,'2':3,y:4}).join()"),
            "2,10,o,y"
        );
        // "01" is not an index
        assert_eq!(eval_str("Object.keys({'01':1,'1':2}).join()"), "1,01");
        assert_eq!(eval_str("JSON.stringify({b:1,'2':2})"), "{\"2\":2,\"b\":1}");
    });
}

#[test]
fn string_replace_expands_dollar_patterns() {
    in_thread(|| {
        assert_eq!(eval_str("'a$&b'.replace('a','$&$&')"), "aa$&b");
        // $` is everything before the match, $' everything after.
        assert_eq!(eval_str("'abc'.replace('b',\"$`|$'\")"), "aa|cc");
    });
}

#[test]
fn math_round_handles_the_half_open_interval() {
    in_thread(|| {
        // the double just below .5 must round to 0, not 1
        assert_eq!(
            eval_str(
                "Math.round(0.49999999999999994) + ',' + Math.round(0.5) + ',' + Math.round(-2.5)"
            ),
            "0,1,-2"
        );
    });
}

#[test]
fn number_statics_are_the_same_function_objects() {
    in_thread(|| {
        assert_eq!(
            eval_str("Number.parseInt===parseInt && Number.parseFloat===parseFloat"),
            "true"
        );
    });
}

#[test]
fn arguments_object_reflects_the_call() {
    in_thread(|| {
        assert_eq!(
            eval_str("(function(){ return arguments.length })(1,2,3)"),
            "3"
        );
        assert_eq!(
            eval_str("(function(a){ return arguments[1] })(\"x\",\"y\")"),
            "y"
        );
        assert_eq!(eval_str("(function(){ return arguments.length })()"), "0");
    });
}

// Sweep of JS-conformance fixes found by differential testing against node.
#[test]
fn math_round_matches_js_ties() {
    in_thread(|| {
        // .5 rounds toward +Infinity, and -0 keeps its sign.
        assert_eq!(
            eval_str(
                "Math.round(-2.5) + ',' + Math.round(2.5) + ',' + Object.is(Math.round(-0.5), -0)"
            ),
            "-2,3,true"
        );
    });
}

#[test]
fn to_precision_selects_significant_digits() {
    in_thread(|| {
        assert_eq!(
            eval_str("(1.5).toPrecision(3) + ',' + (1234).toPrecision(2)"),
            "1.50,1.2e+3"
        );
    });
}

#[test]
fn own_property_names_include_accessors() {
    in_thread(|| {
        assert_eq!(
            eval_str("Object.getOwnPropertyNames({get x(){return 1}}).join()"),
            "x"
        );
    });
}

#[test]
fn array_entries_yield_index_value_pairs() {
    in_thread(|| {
        assert_eq!(eval_str("[...['a','b'].entries()].join('|')"), "0,a|1,b");
        assert_eq!(eval_str("[...['a','b'].keys()].join()"), "0,1");
    });
}

#[test]
fn runtime_faults_are_error_instances() {
    in_thread(|| {
        assert_eq!(
            eval_str("(function(){try{null.x}catch(e){return e instanceof TypeError && e instanceof Error && e.name}})()"),
            "TypeError"
        );
    });
}

// `finally` used to be dropped in three cases: when the try/finally was the last
// statement of a program, when the try threw without a catch, and when it was
// left through `return`. These pin all three, plus the override rule.
#[test]
fn finally_runs_on_the_normal_path_as_last_statement() {
    in_thread(|| {
        assert_eq!(eval_str("var x=0; try{ x=1 } finally { x=2 } x"), "2");
    });
}

#[test]
fn finally_runs_before_a_rethrow() {
    in_thread(|| {
        assert_eq!(
            eval_str(
                "var x=0; try { try { throw new Error('e') } finally { x=7 } } catch (e) {} x"
            ),
            "7"
        );
        // and the exception still propagates when there is no catch
        assert!(eval("try { throw 1 } finally { }").is_err());
    });
}

#[test]
fn finally_runs_when_try_returns() {
    in_thread(|| {
        assert_eq!(
            eval_str("(function(){ var x=0; (function(){ try { return 1 } finally { x=5 } })(); return x })()"),
            "5"
        );
        // a `return` inside `finally` overrides the try's return value
        assert_eq!(
            eval_str("(function(){ try { return 1 } finally { return 2 } })()"),
            "2"
        );
    });
}

#[test]
fn finally_runs_for_catch_return_path() {
    in_thread(|| {
        assert_eq!(
            eval_str(
                "(function(){ var x=0; (function(){ try { throw new Error('e') } catch(e) { return 3 } finally { x=9 } })(); return x })()"
            ),
            "9"
        );
    });
}

#[test]
fn evaluates_arithmetic_and_strings() {
    in_thread(|| {
        assert_eq!(eval_str("1 + 2 * 3"), "7");
        assert_eq!(eval_str("(1 + 2) * 3"), "9");
        assert_eq!(eval_str("2 ** 3 ** 2"), "512");
        assert_eq!(eval_str("7 - 2 - 1"), "4");
        assert_eq!(eval_str("100 / 10 / 2"), "5");
        assert_eq!(eval_str("'a' + 'b' + 1"), "ab1");
        assert_eq!(eval_str("'n=' + (1 + 1)"), "n=2");
        assert_eq!(eval_str("10 % 3"), "1");
        assert_eq!(eval_str("-(2 ** 2)"), "-4");
    });
}

#[test]
fn functions_closures_and_recursion() {
    in_thread(|| {
        assert_eq!(
            eval_str("function f(n){return n<2?n:f(n-1)+f(n-2);} f(15)"),
            "610"
        );
        assert_eq!(
            eval_str(
                "function mk(){let c=0; return function(){return ++c;};} let f=mk(); f(); f(); f()"
            ),
            "3"
        );
        // Two closures from the same factory keep independent counters.
        assert_eq!(
            eval_str("function mk(){let c=0; return function(){return ++c;};} let a=mk(),b=mk(); a();a();b()+''"),
            "1"
        );
    });
}

#[test]
fn arrays_objects_and_prototypes() {
    in_thread(|| {
        assert_eq!(eval_str("[1,2,3].map(x=>x*2).join(',')"), "2,4,6");
        assert_eq!(
            eval_str("[1,2,3,4].filter(x=>x%2===0).reduce((a,b)=>a+b,0)"),
            "6"
        );
        assert_eq!(eval_str("[3,1,2].sort((a,b)=>a-b).join('')"), "123");
        assert_eq!(
            eval_str("const o={a:1,b:2}; Object.keys(o).join('+')"),
            "a+b"
        );
        assert_eq!(
            eval_str("const o={a:1}; o.b=2; Object.values(o).join('+')"),
            "1+2"
        );
        assert_eq!(
            eval_str(
                "function A(n){this.n=n;} A.prototype.hi=function(){return 'hi '+this.n;}; new A('z').hi()"
            ),
            "hi z"
        );
        assert_eq!(eval_str("new Array(1,2,3).length"), "3");
        assert_eq!(eval_str("[1,2,3,4].indexOf(3)"), "2");
        assert_eq!(eval_str("'a,b,c'.split(',').join('-')"), "a-b-c");
    });
}

#[test]
fn classes_constructors_and_static_members() {
    in_thread(|| {
        assert_eq!(
            eval_str("class A{constructor(n){this.n=n;} get d(){return this.n*2;}} new A(4).d"),
            "8"
        );
        assert_eq!(eval_str("class A{ static one(){return 1;} } A.one()"), "1");
        // A fused class field initialiser runs before the user constructor body.
        assert_eq!(
            eval_str("class B{ x=5; constructor(){this.x+=1;} } new B().x"),
            "6"
        );
    });
}

#[test]
fn control_flow_and_loops() {
    in_thread(|| {
        assert_eq!(eval_str("let s=0; for(let i=1;i<=5;i++) s+=i; s"), "15");
        assert_eq!(
            eval_str("function f(s){switch(s){case 1: return 'one'; case 2: return 'two'; default: return 'x';}} f(2)"),
            "two"
        );
        assert_eq!(
            eval_str("let i=0,o=[]; while(i<3){o.push(i++);} o.join('')"),
            "012"
        );
        assert_eq!(eval_str("let i=0; do { i++; } while(i<4); i"), "4");
        assert_eq!(eval_str("let t=0; for(const v of [1,2,3]) t+=v; t"), "6");
        // `continue label` skips the rest of the inner loop only.
        assert_eq!(
            eval_str(
                "let t=0; outer: for(let i=0;i<3;i++){for(let j=0;j<3;j++){if(j===1) continue outer; t++;}} t"
            ),
            "3"
        );
    });
}

#[test]
fn exceptions_unwind_through_calls() {
    in_thread(|| {
        assert_eq!(
            eval_str("try{ throw new Error('x'); }catch(e){ e.message }"),
            "x"
        );
        assert_eq!(
            eval_str("function f(){ try { throw new Error('deep'); } catch(e){ return 'c:'+e.message; } } f()"),
            "c:deep"
        );
        // A rethrow crosses a call boundary and is caught by the outer frame.
        assert_eq!(
            eval_str(
                "function t(){ try { throw new Error('i'); } catch(e){ throw e; } } try { t(); } catch(e){ e.message }"
            ),
            "i"
        );
    });
}

#[test]
fn generators_suspend_and_resume() {
    in_thread(|| {
        assert_eq!(
            eval_str(
                "function* g(){yield 1; yield 2; yield 3;} const it=g(); it.next().value+','+it.next().value+','+it.next().value+','+it.next().done"
            ),
            "1,2,3,true"
        );
        assert_eq!(
            eval_str("function* g(){yield 1; yield 2;} [...g()].join('|')"),
            "1|2"
        );
        assert_eq!(
            eval_str("function* g(){yield 'a'; yield 'b';} let o=''; for(const v of g()) o+=v; o"),
            "ab"
        );
    });
}

#[test]
fn json_round_trips() {
    in_thread(|| {
        assert_eq!(
            eval_str("JSON.stringify({a:1,b:[true,null,'x']})"),
            r#"{"a":1,"b":[true,null,"x"]}"#
        );
        assert_eq!(
            eval_str("const p=JSON.parse('{\"k\":10,\"l\":[1,2]}'); p.k+','+p.l.join('-')"),
            "10,1-2"
        );
    });
}

#[test]
fn template_literals_and_optional_chaining() {
    in_thread(|| {
        assert_eq!(eval_str("const n='w'; `${n}${1+1}!`"), "w2!");
        assert_eq!(eval_str("const o={a:{b:1}}; (o?.a?.b) ?? 9"), "1");
        assert_eq!(eval_str("const o={}; (o?.a?.b) ?? 9"), "9");
    });
}

#[test]
fn destructuring_patterns() {
    in_thread(|| {
        assert_eq!(eval_str("const [a,b=9]=[1]; a+'-'+b"), "1-9");
        assert_eq!(eval_str("const {x=1,y=2}={x:5}; x+','+y"), "5,2");
        assert_eq!(eval_str("const [,second]=['skip','keep']; second"), "keep");
        assert_eq!(eval_str("const {a:{b}}={a:{b:7}}; b"), "7");
        assert_eq!(
            eval_str("const [h,...t]=[1,2,3]; h+':'+t.join('+')"),
            "1:2+3"
        );
    });
}

#[test]
fn uncaught_errors_surface_with_a_message() {
    in_thread(|| {
        // The message has to be read with the *same* engine: values are offsets
        // into that engine's arena, so a second engine would read them as
        // unrelated memory.
        let mut e = crate::vm::Engine::new();
        e.ensure_started();
        match e.eval_source("undefinedFn()", "test.js") {
            Err(crate::errors::VmError::Exception(t)) => {
                let msg = e.vm.to_string(t.value).unwrap_or_default();
                assert!(msg.contains("is not a function"), "got {:?}", msg);
            }
            other => panic!("expected a thrown exception, got {:?}", other),
        }
    });
}

#[test]
fn function_var_hoisting_and_for_in_registers() {
    in_thread(|| {
        assert_eq!(eval_str("function f(){{var q={x:7};}return q.x;} f()"), "7");
        assert_eq!(eval_str("function f(x){var x;{var x;}return x;} f(9)"), "9");
        assert_eq!(eval_str("var x=3;{var x;} x"), "3");
        assert_eq!(
            eval_str("let sum=0;const o={a:1,b:2};for(let k in o){sum+=o[k];}sum"),
            "3"
        );
        assert_eq!(
            eval_str("let s='';for(var k in {a:1,b:2}){if(k==='a')continue;s+=k;}s+k"),
            "bb"
        );
        assert_eq!(
            eval_str("let s=0;for(let k in {a:1,b:2}){for(let q in {x:1,y:2}){s++;}}s"),
            "4"
        );
    });
}

#[test]
fn utf16_units_surrogates_and_ascii() {
    in_thread(|| {
        assert_eq!(
            eval_str("'💥'.length+':'+ '💥'.charCodeAt(0)+':'+ '💥'.charCodeAt(1)"),
            "2:55357:56485"
        );
        assert_eq!(
            eval_str("'💥'[0].charCodeAt(0)+':'+ '💥'.at(-1).charCodeAt(0)"),
            "55357:56485"
        );
        assert_eq!(
            eval_str("String.fromCharCode(0xd800).charCodeAt(0)"),
            "55296"
        );
        assert_eq!(
            eval_str("String.fromCharCode(0xd800)===String.fromCharCode(0xfffd)"),
            "false"
        );
        assert_eq!(
            eval_str("(''+String.fromCharCode(0xd800)).charCodeAt(0)"),
            "55296"
        );
        assert_eq!(eval_str("'💥'.slice(1).charCodeAt(0)"), "56485");
        assert_eq!(
            eval_str("JSON.stringify(String.fromCharCode(0xd800))"),
            "\"\\ud800\""
        );
        assert_eq!(eval_str("'abc'.charCodeAt('1')+':'+ 'abc'.length"), "98:3");
        assert_eq!(
            eval_str("String.fromCodePoint(0x1f4a5).codePointAt(0)"),
            "128165"
        );
    });
}

#[test]
fn guarded_native_regions_and_exact_side_exits() {
    in_thread(|| {
        let mut e = crate::vm::Engine::new();
        e.ensure_started();
        let source = "const o={add(x,y){return x+y;},mul(x,y){return x*y;}};let s=0;for(let i=0;i<1000;i++){if((i&1)===0)s+=o.add(i,3);else s+=o.mul(i,2);}s";
        let v = e.eval_source(source, "region.js").unwrap();
        assert_eq!(e.vm.to_string(v).unwrap(), "751000");
        if crate::jit::NATIVE_SUPPORTED {
            assert!(
                e.vm.compiled_loop_count() > 0,
                "bitwise/call loop must use native region"
            );
            assert!(
                e.vm.completed_loop_count() > 0,
                "real compiler output must finish natively instead of immediately deoptimizing"
            );
        } else {
            assert_eq!(
                e.vm.compiled_loop_count(),
                0,
                "unsupported hosts must interpret"
            );
        }
        let completed = e.vm.completed_loop_count();
        let full_workload = format!("{}\nsum", include_str!("../bench/method_calls.js"));
        let value = e.eval_source(&full_workload, "method-calls.js").unwrap();
        assert_eq!(value.as_f64(), 750001000000.0);
        if crate::jit::NATIVE_SUPPORTED {
            assert!(
                e.vm.completed_loop_count() > completed,
                "the unchanged million-call fixture must finish natively too"
            );
        }
        assert_eq!(eval_str("function f(o){let s=0;for(let i=0;i<1000;i++){s+=o.x;}return s;}const o={x:1};let a=f(o);o.x=3;a+f(o)"), "4000");
        assert_eq!(eval_str("function f(o){let s=0;for(let i=0;i<1000;i++){s+=o.x;}return s;}const o={x:1};let a=f(o);delete o.x;let p={x:3};Object.setPrototypeOf(o,p);a+f(o)"), "4000");
        assert_eq!(eval_str("function f(o){let s=0;for(let i=0;i<1000;i++){s+=o.f(i);}return s;}const o={f(x){return x+1;}};let a=f(o);o.f=function(x){return x+2;};a+f(o)"), "1002000");
        assert_eq!(
            eval_str("let s=0;for(let i=0;i<1000;i++){s+=(i&1);}s"),
            "500"
        );
    });
}

#[cfg(not(all(
    target_arch = "aarch64",
    target_endian = "little",
    any(target_os = "macos", target_os = "linux")
)))]
#[test]
fn unsupported_hosts_never_publish_aarch64_entries() {
    in_thread(|| {
        use crate::bytecode::{Ic, Instr, Op};
        use crate::objects::FnKind;
        let mut engine = crate::vm::Engine::new();
        let function = engine
            .eval_source("(function(x){return x+1;})", "unsupported-jit.js")
            .unwrap();
        let crate::objects::FnKind::Bytecode(proto) = &unsafe { function.as_function() }.kind
        else {
            panic!("bytecode function");
        };
        assert!(crate::jit::try_compile_func(proto).is_none());
        let instructions = [
            Instr::new(Op::JumpLocalCmp, 0, 0, 3),
            Instr::new(Op::LocalAdd, 1, 0, 0),
            Instr::new(Op::LocalAddKJ, 0, 1, 0),
        ];
        let caches = [Ic::default(); 3];
        let mut constants = |index| [10.0, 1.0].get(index).copied();
        assert!(unsafe {
            crate::jit::try_compile(
                &instructions,
                0,
                3,
                &mut constants,
                &mut |_| None,
                caches.as_ptr(),
            )
        }
        .is_none());
        assert!(unsafe {
            crate::jit::try_compile_region(
                &instructions,
                0,
                3,
                &mut constants,
                caches.as_ptr(),
                engine.vm.arena().base() as usize,
            )
        }
        .is_none());
    });
}

#[test]
fn engine_drop_releases_live_function_prototypes() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        let function = engine
            .eval_source("(function(x){return x+1;})", "prototype-lifetime.js")
            .unwrap();
        let proto = match &unsafe { function.as_function() }.kind {
            crate::objects::FnKind::Bytecode(proto) => proto.clone(),
            _ => panic!("bytecode function"),
        };
        if crate::jit::NATIVE_SUPPORTED {
            let first = crate::jit::try_compile_func(&proto).expect("native leaf");
            assert_eq!(
                crate::jit::try_compile_func(&proto),
                Some(first),
                "one mapping per prototype"
            );
            proto.jit_deopts.store(
                crate::jit::NATIVE_DEOPT_LIMIT,
                std::sync::atomic::Ordering::Relaxed,
            );
            assert!(
                crate::jit::try_compile_func(&proto).is_none(),
                "disabled functions must not be recompiled by a region"
            );
        }
        let weak = std::sync::Arc::downgrade(&proto);
        drop(proto);
        drop(engine);
        assert!(
            weak.upgrade().is_none(),
            "live arena closures must release their Rust-owned prototypes on engine shutdown"
        );
    });
}

#[test]
fn property_presence_is_distinct_from_an_undefined_value() {
    in_thread(|| {
        assert_eq!(
            eval_str("const p={x:7};const o=Object.create(p);o.x=undefined;o.x===undefined"),
            "true"
        );
        assert_eq!(eval_str("const p={get x(){throw 'prototype getter ran';}};const o=Object.create(p);Object.defineProperty(o,'x',{get(){return undefined;}});o.x===undefined"), "true");
        assert_eq!(eval_str("const p={get x(){throw 'prototype getter ran';}};const o=Object.create(p);Object.defineProperty(o,'x',{set(v){this.y=v;}});o.x===undefined"), "true");
        assert_eq!(
            eval_str("const p={0:7};const o=Object.create(p);o[0]=undefined;o[0]===undefined"),
            "true"
        );
        assert_eq!(eval_str("const p={0:7};function f(){}Object.setPrototypeOf(f,p);f[0]=undefined;f[0]===undefined"), "true");
    });
}

#[test]
fn prototype_mutations_invalidate_setters_and_reject_cycles() {
    in_thread(|| {
        for mutation in ["Object.setPrototypeOf(mid,top)", "mid.__proto__=top"] {
            let source = format!("const top={{set x(v){{this.hit=v;}}}};const mid={{}};function write(o){{o.x=3;}}write(Object.create(mid));{mutation};const o=Object.create(mid);write(o);o.hit===3");
            assert_eq!(eval_str(&source), "true");
        }
        assert_eq!(
            eval_str("function f(){}const p={x:7};Object.setPrototypeOf(f,p);f.x===7"),
            "true"
        );
        for mutation in ["Object.setPrototypeOf(o,o)", "o.__proto__=o"] {
            assert_eq!(
                eval_str(&format!(
                    "const o={{}};let threw=false;try{{{mutation};}}catch(e){{threw=true;}}threw"
                )),
                "true"
            );
        }
        assert_eq!(eval_str("const a={};const b=Object.create(a);let threw=false;try{Object.setPrototypeOf(a,b);}catch(e){threw=true;}threw&&Object.getPrototypeOf(a)===Object.prototype"), "true");
        assert_eq!(eval_str("const o={};Object.preventExtensions(o);let threw=false;try{Object.setPrototypeOf(o,{});}catch(e){threw=true;}threw&&Object.setPrototypeOf(o,Object.prototype)===o"), "true");
        assert_eq!(eval_str("const o={};o.__proto__=7;let threw=false;try{Object.setPrototypeOf(o,7);}catch(e){threw=true;}threw&&Object.getPrototypeOf(o)===Object.prototype"), "true");
        assert_eq!(eval_str("let threw=false;try{Object.setPrototypeOf(Object.prototype,Object.create(null));}catch(e){threw=true;}threw&&Object.getPrototypeOf(Object.prototype)===null"), "true");
        assert_eq!(
            eval_str("const a=[];const p={x:11};Object.setPrototypeOf(a,p);a.x===11"),
            "true"
        );
        assert_eq!(eval_str("let p=null;for(let i=0;i<100;i++)p=Object.create(p);const o={};Object.setPrototypeOf(o,p);Object.getPrototypeOf(o)===p"), "true");
    });
}

#[test]
fn setter_memos_expire_when_prototype_addresses_are_recycled() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        let top = engine
            .eval_source("({set x(v){this.hit=v;}})", "setter-recycle.js")
            .unwrap();
        let key = engine.vm.key_from_value(Value::from_str_owned("x"));
        let scope = engine.vm.arena().root_scope(&[]);
        let old = Value::from_ptr(engine.vm.arena().alloc_object(0, Value::NULL));
        let receiver = Value::from_ptr(engine.vm.arena().alloc_object(0, old));
        engine
            .vm
            .set_prop(receiver, key, Value::from_i32(1))
            .unwrap();
        let roots = [top, Value::from_ptr(engine.vm.globals)];
        drop(scope);
        engine.vm.arena().collect(&[&roots]);
        let _filler = engine.vm.arena().alloc_object(0, Value::NULL);
        let replacement = Value::from_ptr(engine.vm.arena().alloc_object(0, top));
        assert_eq!(
            replacement.bits(),
            old.bits(),
            "probe must reuse the cached prototype's address"
        );
        let receiver = Value::from_ptr(engine.vm.arena().alloc_object(0, replacement));
        engine
            .vm
            .set_prop(receiver, key, Value::from_i32(3))
            .unwrap();
        let hit = engine.vm.key_from_value(Value::from_str_owned("hit"));
        assert_eq!(engine.vm.get_prop(receiver, hit).unwrap().as_f64(), 3.0);
    });
}

#[test]
fn collection_traces_function_prototype_and_own_properties() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        let function = engine
            .eval_source("(function(){return 1;})", "function-roots.js")
            .unwrap();
        let scope = engine.vm.arena().root_scope(&[]);
        let prototype = engine.vm.new_object();
        let property = engine.vm.new_object();
        let key = engine.vm.key_from_value(Value::from_str_owned("extra"));
        engine.vm.set_prototype(function, prototype).unwrap();
        engine.vm.set_prop(function, key, property).unwrap();
        drop(scope);
        let roots = [function, Value::from_ptr(engine.vm.globals)];
        engine.vm.arena().collect(&[&roots]);
        assert_eq!(
            unsafe { prototype.obj_tag() },
            Some(crate::objects::TAG_OBJECT),
            "function [[Prototype]] must remain live"
        );
        assert_eq!(
            unsafe { property.obj_tag() },
            Some(crate::objects::TAG_OBJECT),
            "function own property must remain live"
        );
    });
}

#[test]
fn automatic_collection_reclaims_guest_churn_and_preserves_host_values() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        engine.vm.arena().set_collection_budget(16 * 1024);
        let saved = engine
            .eval_source("({answer:42})", "host-result.js")
            .unwrap();
        let result = engine
            .eval_source(
                "for(let i=0;i<30000;i++)globalThis.scratch={i,a:[i,i+1]};scratch.a[1]",
                "churn.js",
            )
            .unwrap();
        assert_eq!(result.as_f64(), 30000.0);
        assert!(engine.vm.arena().collection_count() > 10);
        assert!(
            engine.vm.arena().offset() < 2 * 1024 * 1024,
            "short-lived objects must reuse the arena"
        );
        let key = engine.vm.key_from_value(Value::from_str_owned("answer"));
        assert_eq!(engine.vm.get_prop(saved, key).unwrap().as_f64(), 42.0);
    });
}

#[test]
fn external_array_storage_triggers_collection_and_sparse_push_keeps_length() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        engine.vm.arena().set_collection_budget(16 * 1024);
        let value = engine.eval_source("for(let k=0;k<100;k++){globalThis.big=[];for(let i=0;i<10000;i++)big.push(i);}big.length", "external-pressure.js").unwrap();
        assert_eq!(value.as_f64(), 10000.0);
        assert!(
            engine.vm.arena().collection_count() > 50,
            "array buffers must count toward collection pressure"
        );
        let value = engine.eval_source("let holes=Array(3);holes.push(7);let sparse=[];sparse[100000]=9;sparse.length=0;sparse.length=100001;[holes.length,holes[3],sparse[100000]].join(':')", "sparse-length.js").unwrap();
        assert_eq!(engine.vm.to_string(value).unwrap(), "4:7:");
    });
}

#[test]
fn native_callback_snapshots_and_errors_survive_collection() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        engine.vm.arena().set_collection_budget(16 * 1024);
        let source = "globalThis.churn=function(){for(let k=0;k<150;k++)globalThis.tmp={k,a:[k]};};let a=[];for(let i=32;i>0;i--)a.push({n:i});a.sort((x,y)=>{a.fill(null);churn();return x.n-y.n;});[a[0].n,a[31].n,a.length].join(':')";
        let value = engine.eval_source(source, "sort-roots.js").unwrap();
        assert_eq!(engine.vm.to_string(value).unwrap(), "1:32:32");
        let value = engine.eval_source("let input=[];for(let i=0;i<10000;i++)input.push(i);input.reduce((a,x)=>({n:a.n+1}),{n:0}).n", "reduce-roots.js").unwrap();
        assert_eq!(value.as_f64(), 10000.0);
        let value = engine.eval_source("let caught=false;try{[1,2].map(x=>{churn();throw {ok:true};});}catch(e){caught=e.ok;}caught", "callback-throw.js").unwrap();
        assert_eq!(value, Value::TRUE);
        let value = engine.eval_source("let caughtSort=false;try{[{toString(){throw 'sort';}},{}].sort();}catch(e){caughtSort=e==='sort';}caughtSort", "sort-throw.js").unwrap();
        assert_eq!(value, Value::TRUE);
        let value = engine
            .eval_source(
                "'AB'.charCodeAt({valueOf(){churn();return 1;}})",
                "leaf-coercion.js",
            )
            .unwrap();
        assert_eq!(value.as_f64(), 66.0);
        let value = engine.eval_source("let a=[];a.push=function(v){churn();this[0]=v;return this.length;};a.push({n:42});a[0].n", "leaf-replacement.js").unwrap();
        assert_eq!(value.as_f64(), 42.0);
    });
}

#[test]
fn bound_functions_keep_receiver_arguments_and_constructor_semantics() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        engine.vm.arena().set_collection_budget(16 * 1024);
        let source = "function f(a,b,c){return this.n+a+b+c;}globalThis.bound=f.bind({n:40},1);let twice=bound.bind({n:900},2);let zero=f.bind({n:7});for(let i=0;i<3000;i++)globalThis.tmp={i,a:[i]};[bound(2,3),bound.call({n:4},2,3),twice(3),zero(1,2,3)].join(':')";
        let value = engine.eval_source(source, "bound-this.js").unwrap();
        assert_eq!(engine.vm.to_string(value).unwrap(), "46:46:46:13");
        let value = engine.eval_source("function C(a,b){this.sum=a+b;}let B=C.bind({sum:-1},5);let D=B.bind({sum:-2},7);let o=new D();[o.sum,o instanceof C].join(':')", "bound-new.js").unwrap();
        assert_eq!(engine.vm.to_string(value).unwrap(), "12:true");
        let value = engine.eval_source("function F(){return function(){return 7;};}function S(){return 'text';}let A=F.bind(null),B=S.bind(null);[typeof (new A()),typeof (new B())].join(':')", "bound-return-types.js").unwrap();
        assert_eq!(engine.vm.to_string(value).unwrap(), "function:object");
        let value = engine
            .eval_source(
                "let threw=false;try{let B=(()=>1).bind(null);new B();}catch(e){threw=true;}threw",
                "bound-arrow-new.js",
            )
            .unwrap();
        assert_eq!(value, Value::TRUE);
        let value = engine.eval_source("let caught=false;try{(function(){}).bind.call(1,null);}catch(e){caught=true;}caught", "bound-invalid.js").unwrap();
        assert_eq!(value, Value::TRUE);
        let value = engine.eval_source("function recurse(n){if(n<=0)return 0;return nextBound(n-1)+1;}globalThis.nextBound=recurse.bind(null);nextBound(200)", "bound-recursion.js").unwrap();
        assert_eq!(value.as_f64(), 200.0);
        let value = engine.eval_source("function fail(){throw 9;}let caught=0;try{fail.bind(null)();}catch(e){caught=e;}caught", "bound-throw.js").unwrap();
        assert_eq!(value.as_f64(), 9.0);
    });
}

#[test]
fn object_coercion_accepts_primitives_and_rejects_object_results() {
    in_thread(|| {
        for (source, expected) in [
            ("+({valueOf(){return 7;}})", "7"),
            ("let n=0;let a={valueOf(){n++;return {};},toString(){n++;return '5';}};[+a,n].join(':')", "5:2"),
            ("let threw=false;try{+({valueOf(){return {};},toString(){return {};}});}catch(e){threw=true;}threw", "true"),
        ] {
            let mut engine = crate::vm::Engine::new();
            engine.ensure_started();
            let value = engine.eval_source(source, "coercion.js").unwrap_or_else(|error| panic!("{source}: {error:?}"));
            assert_eq!(engine.vm.to_string(value).unwrap(), expected, "{source}");
        }
    });
}

#[test]
fn generator_states_are_traced_weakly_and_slots_are_reused() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        engine.vm.arena().set_collection_budget(16 * 1024);
        let value = engine.eval_source("function* f(n){let held={n};yield held;return held.n;}globalThis.live=f(37);live.next();for(let i=0;i<5000;i++){f(i);globalThis.tmp={i,a:[i]};}delete live['#gen'];live.next().value", "generator-roots.js").unwrap();
        assert_eq!(value.as_f64(), 37.0);
        assert!(
            engine.vm.gens.len() < 512,
            "unreachable generators must not grow the registry forever"
        );
        let value = engine.eval_source("let rejected=false;try{live.next.call({'#gen':0});}catch(e){rejected=true;}rejected", "generator-brand.js").unwrap();
        assert_eq!(value, Value::TRUE);
    });
}

#[test]
fn timers_root_future_callbacks_and_run_bytecode() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        engine.vm.arena().set_collection_budget(16 * 1024);
        engine.eval_source("globalThis.observed=0;(function(){const held={n:42};setTimeout(()=>{for(let i=0;i<3000;i++)globalThis.tmp={i,a:[i]};},0);setTimeout(()=>{globalThis.observed=held.n;},2);})();", "timer-roots.js").unwrap();
        engine.vm.collect_garbage();
        engine.run_event_loop().unwrap();
        assert_eq!(
            engine
                .eval_source("globalThis.observed", "timer-result.js")
                .unwrap()
                .as_f64(),
            42.0
        );
        assert_eq!(crate::vm::timer_count(), 0);
    });
}

#[cfg(all(
    target_arch = "aarch64",
    target_endian = "little",
    any(target_os = "macos", target_os = "linux")
))]
#[test]
fn native_call_region_finishes_and_exits_before_failed_calls() {
    in_thread(|| {
        use crate::bytecode::{Ic, Instr, Op};
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        let function = engine
            .eval_source("(function(x,y){return x+y;})", "native-call.js")
            .unwrap();
        let instructions = [
            Instr::new(Op::JumpLocalCmp, 0, 0, 5),
            Instr::new(Op::Move, 3, 1, 0),
            Instr::new(Op::Move, 4, 0, 0),
            Instr::new(Op::Call, 1, 2, 2),
            Instr::new(Op::LocalAddKJ, 0, 1, 0),
        ];
        let caches = [Ic::default(); 5];
        let code = unsafe {
            crate::jit::try_compile_region(
                &instructions,
                0,
                5,
                &mut |index| [10.0, 1.0].get(index).copied(),
                caches.as_ptr(),
                engine.vm.arena().base() as usize,
            )
        }
        .expect("native call region");
        let mut registers = [
            Value::from_i32(0),
            Value::from_i32(0),
            function,
            Value::UNDEFINED,
            Value::UNDEFINED,
        ];
        assert_eq!(
            unsafe { crate::jit::run_raw(code.code, registers.as_mut_ptr(), 0) },
            1
        );
        assert_eq!(registers[0].as_f64(), 10.0);
        assert_eq!(registers[1].as_f64(), 45.0);
        registers[0] = Value::from_i32(0);
        registers[1] = Value::TRUE;
        assert_eq!(
            unsafe { crate::jit::run_raw(code.code, registers.as_mut_ptr(), 0) },
            5,
            "failed call must resume at bytecode 3"
        );
        assert_eq!(
            registers[1],
            Value::TRUE,
            "failed call must not overwrite its output"
        );
        registers[2] = Value(0);
        assert_eq!(
            unsafe { crate::jit::run_raw(code.code, registers.as_mut_ptr(), 0) },
            5,
            "a zero-valued non-function must miss an empty target cache safely"
        );
    });
}

#[cfg(all(
    target_arch = "aarch64",
    target_endian = "little",
    any(target_os = "macos", target_os = "linux")
))]
#[test]
fn native_regions_accept_guarded_cell_commits_without_repeating_writes() {
    in_thread(|| {
        use std::sync::atomic::Ordering;
        let mut engine = crate::vm::Engine::new();
        let function = engine
            .eval_source(
                "(function(){let n=0;return function(){return ++n;};})()",
                "commit-leaf.js",
            )
            .unwrap();
        let crate::objects::FnKind::Bytecode(proto) = &unsafe { function.as_function() }.kind
        else {
            panic!("bytecode");
        };
        assert_eq!(proto.nupvals, 1, "one descriptor per captured binding");
        assert!(crate::jit::try_compile_func(proto).is_some());
        assert_eq!(proto.jit_pure.load(Ordering::Relaxed), 0);
        assert_eq!(
            proto.jit_region_safe.load(Ordering::Relaxed),
            1,
            "{:?}",
            proto.code
        );
        // A later failing instruction must leave this completed call committed
        // exactly once, with the interpreter resuming after the call.
        use crate::bytecode::{Ic, Instr, Op};
        let instructions = [
            Instr::new(Op::JumpLocalCmp, 0, 0, 4),
            Instr::new(Op::Call, 1, 2, 0),
            Instr::new(Op::Add, 4, 1, 5),
            Instr::new(Op::LocalAddKJ, 0, 1, 0),
        ];
        let caches = [Ic::default(); 4];
        let region = unsafe {
            crate::jit::try_compile_region(
                &instructions,
                0,
                6,
                &mut |i| [2.0, 1.0].get(i).copied(),
                caches.as_ptr(),
                engine.vm.arena().base() as usize,
            )
        }
        .expect("cell-commit region");
        let mut registers = [
            Value::from_i32(0),
            Value::from_i32(0),
            function,
            Value::UNDEFINED,
            Value::UNDEFINED,
            Value::UNDEFINED,
        ];
        assert_eq!(
            unsafe { crate::jit::run_raw(region.code, registers.as_mut_ptr(), 0) },
            4
        );
        assert_eq!(registers[1].as_f64(), 1.0);
        assert_eq!(
            unsafe { (*function.as_function().upvalues[0]).v }.as_f64(),
            1.0
        );
        let value = engine.eval_source("globalThis.counterFactory=function(){let n=0;return {inc(){return ++n;},set(v){n=v;},get(){return n;}};};let c=counterFactory();for(let i=0;i<1000;i++)c.inc();c.set(8796093022206);for(let i=0;i<10;i++)c.inc();c.get()", "commit-overflow.js").unwrap();
        assert_eq!(value.as_f64(), 8796093022216.0);
        let value = engine.eval_source("let a=counterFactory(),b=counterFactory();for(let i=0;i<997;i++){let f=(i&1)?a.inc:b.inc;f();}a.get()+b.get()", "commit-callees.js").unwrap();
        assert_eq!(value.as_f64(), 997.0);
        let value = engine
            .eval_source(
                "let n=0;function inc(x){n+=x;return n;}for(let i=0;i<1000;i++)inc(1);inc('x')",
                "commit-coercion.js",
            )
            .unwrap();
        assert_eq!(engine.vm.to_string(value).unwrap(), "1000x");
    });
}

#[test]
fn arrow_functions_keep_lexical_this_and_gc_roots() {
    in_thread(|| {
        assert_eq!(eval_str("const o={n:7,make(){return x=>this.n+x;}};const f=o.make();let s=0;for(let i=0;i<1000;i++)s+=f(i);s"), "506500");
        assert_eq!(eval_str("function make(){return x=>this+x;}const o={f:make.call(7)};let s=0;for(let i=0;i<1000;i++)s+=o.f(i);s"), "506500");
        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        let function = engine
            .eval_source(
                "(function(){const o={n:7};return (function(){return ()=>this.n;}).call(o);})()",
                "arrow-gc.js",
            )
            .unwrap();
        let roots = [function, Value::from_ptr(engine.vm.globals)];
        engine.vm.arena().collect(&[&roots]);
        let result = engine
            .vm
            .call_direct_args(function, Value::UNDEFINED, &[])
            .unwrap();
        assert_eq!(result.as_f64(), 7.0);
    });
}

#[test]
fn native_call_targets_can_change_inside_one_region() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        let value = engine.eval_source("function a(x){return x+1;}function b(x){return x+2;}let f=a;let s=0;for(let i=0;i<977;i++){if((i&1)===0)f=a;else f=b;s+=f(i);}s", "changing-target.js").unwrap();
        assert_eq!(value.as_f64(), 478241.0);
        if crate::jit::NATIVE_SUPPORTED {
            assert!(engine.vm.completed_loop_count() > 0);
        }
    });
}

#[test]
fn native_call_targets_reset_across_collection_and_replacement() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        let run = engine
            .eval_source(
                "(function(o){let s=0;for(let i=0;i<733;i++)s+=o.f(i);return s;})",
                "call-gc.js",
            )
            .unwrap();
        let object = engine.vm.new_object();
        let key = engine.vm.key_from_value(Value::from_str_owned("f"));
        for increment in 1..=3 {
            let source = format!("(function(x){{return x+{increment};}})");
            let function = engine.eval_source(&source, "replacement.js").unwrap();
            engine.vm.set_prop(object, key, function).unwrap();
            let completed = engine.vm.completed_loop_count();
            let value = engine
                .vm
                .call_direct_args(run, Value::UNDEFINED, &[object])
                .unwrap();
            assert_eq!(value.as_f64(), (733 * 732 / 2 + 733 * increment) as f64);
            if crate::jit::NATIVE_SUPPORTED {
                assert!(engine.vm.completed_loop_count() > completed);
            }
            engine.vm.set_prop(object, key, Value::UNDEFINED).unwrap();
            let roots = [run, object, Value::from_ptr(engine.vm.globals)];
            engine.vm.arena().collect(&[&roots]);
        }
    });
}

#[test]
fn numeric_native_boundaries_match_javascript() {
    in_thread(|| {
        assert_eq!(eval_str("1/(-0)"), "-Infinity");
        assert_eq!(eval_str("1/(0*-1)"), "-Infinity");
        assert_eq!(eval_str("1/(-4%2)"), "-Infinity");
        assert_eq!(eval_str("-1>>>0"), "4294967295");
        assert_eq!(
            eval_str("(4294967297.5|0)+':'+(NaN|0)+':'+(Infinity|0)"),
            "1:0:0"
        );
        assert_eq!(
            eval_str("function m(a,b){return a*b;}let s=0;for(let i=0;i<100;i++){s=m(0,-1);}1/s"),
            "-Infinity"
        );
        assert_eq!(eval_str("function m(a,b){return a*b;}let s=0;for(let i=0;i<100;i++){s=m(4294967296,4294967296);}s"), "18446744073709552000");
    });
}

#[test]
fn unicode_literals_and_json_code_units() {
    in_thread(|| {
        assert_eq!(
            eval_str(
                r#"['\ud83d\udca5'.length,'\ud83d\udca5'.codePointAt(0),'\ud800'.charCodeAt(0),'\ud800' === '\ufffd',JSON.parse('"\\ud800"').charCodeAt(0)].join(',')"#
            ),
            "2,128165,55296,false,55296"
        );
    });
}

#[test]
fn unicode_identifier_lookahead_always_advances_at_scalar_boundaries() {
    in_thread(|| {
        assert_eq!(eval_str("let ünïcode=7;let 日本語=11;ünïcode+日本語"), "18");
        assert_eq!(eval_str("let\u{a0}é\u{2003}=\u{feff}7;\u{2028}é"), "7");
        for space in [
            '\u{000b}', '\u{00a0}', '\u{1680}', '\u{2000}', '\u{200a}', '\u{2028}', '\u{2029}',
            '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
        ] {
            crate::parser::parse_program(&format!("let{space}x{space}={space}1;")).unwrap();
        }
        assert!(
            crate::lexer::tokenize(r"let \uZZZZ = 1").is_err(),
            "malformed escapes must terminate with an error"
        );
        assert!(crate::lexer::tokenize(r"class C { #\uZZZZ = 1 }").is_err());
    });
}

#[test]
fn captured_lexicals_and_exception_completion_run_once() {
    in_thread(|| {
        assert_eq!(eval_str("let n=0;function f(i){n++;if(i===777)throw 9;return i;}let s=0;try{for(let i=0;i<1234;i++)s+=f(i);}catch(e){s+=n;}s+':'+n"),"302254:778");
        assert_eq!(eval_str("let n=0;function f(){n++;}f();n"), "1");
        assert_eq!(eval_str("let n=0;if(true){n++;n++;}n"), "2");
    });
}

#[test]
fn utf16_payload_survives_forced_collection() {
    in_thread(|| {
        let arena = Arena::new().install();
        let units = [0x61, 0xd800, 0xd83d, 0xdca5, 0x62];
        let value = Value::from_ptr(arena.alloc_string_utf16(&units));
        let atom = arena.intern_atom_utf16(&[0xdfff]);
        for _ in 0..3 {
            for _ in 0..1000 {
                arena.alloc_string("discard");
            }
            arena.collect(&[&[value]]);
            assert_eq!(
                unsafe { value.as_string() }
                    .code_units()
                    .collect::<Vec<_>>(),
                units
            );
            assert_eq!(unsafe { atom.as_string() }.code_unit(0), Some(0xdfff));
        }
    });
}

#[test]
fn native_region_roots_survive_forced_collection() {
    in_thread(|| {
        let mut e = crate::vm::Engine::new();
        e.ensure_started();
        let f = e
            .eval_source(
                "(function(o){let s=0;for(let i=0;i<733;i++){s+=(i&1);s+=o.x;}return s;})",
                "gc-region.js",
            )
            .unwrap();
        let o = e.vm.new_object();
        let k = e.vm.key_from_value(Value::from_str_owned("x"));
        e.vm.set_prop(o, k, Value::from_i32(7)).unwrap();
        for _ in 0..3 {
            let result = e.vm.call_direct_args(f, Value::UNDEFINED, &[o]).unwrap();
            assert_eq!(result.as_f64(), 5497.0);
            if crate::jit::NATIVE_SUPPORTED {
                assert!(e.vm.compiled_loop_count() > 0);
            }
            let roots = [f, o, Value::from_ptr(e.vm.globals)];
            e.vm.arena().collect(&[&roots]);
        }
    });
}

#[test]
fn mid_region_numeric_guard_preserves_prior_updates() {
    in_thread(|| {
        assert_eq!(
            eval_str("const a=[1,2.5,3];let s=0;for(let i=0;i<1207;i++){s+=(i&1);s+=a[i%3];}s"),
            "3217"
        );
    });
}

#[cfg(all(
    target_arch = "aarch64",
    target_endian = "little",
    any(target_os = "macos", target_os = "linux")
))]
#[test]
fn native_property_region_completes_without_interpreter_fallback() {
    in_thread(|| {
        use crate::bytecode::{Ic, Instr, Op};

        let mut engine = crate::vm::Engine::new();
        engine.ensure_started();
        let object = engine.vm.new_object();
        let key = engine.vm.key_from_value(Value::from_str_owned("x"));
        engine.vm.set_prop(object, key, Value::from_i32(7)).unwrap();
        let instructions = [
            Instr::new(Op::JumpLocalCmp, 0, 0, 4),
            Instr::new(Op::GetProp, 3, 2, 0),
            Instr::new(Op::LocalAdd, 1, 3, 0),
            Instr::new(Op::LocalAddKJ, 0, 1, 0),
        ];
        let mut caches = [Ic::default(); 4];
        let receiver = unsafe { object.as_object() };
        caches[1] = Ic {
            key,
            shape: receiver.shape,
            slot: receiver.find_index(key).unwrap() as u32,
            state: 1,
            ..Ic::default()
        };
        // SAFETY: `caches` outlives the call and covers every index the region reads.
        let code = unsafe {
            crate::jit::try_compile_region(
                &instructions,
                0,
                4,
                &mut |index| [5.0, 1.0].get(index).copied(),
                caches.as_ptr(),
                engine.vm.arena().base() as usize,
            )
        }
        .expect("property region must compile");
        let mut registers = [
            Value::from_i32(0),
            Value::from_i32(0),
            object,
            Value::UNDEFINED,
        ];
        let status = unsafe { crate::jit::run_raw(code.code, registers.as_mut_ptr(), 0) };
        assert_eq!(
            status, 1,
            "native region must finish, not just compile and deopt"
        );
        assert_eq!(registers[0].as_f64(), 5.0);
        assert_eq!(registers[1].as_f64(), 35.0);
    });
}

#[test]
fn native_regions_recheck_dynamic_loop_bounds_and_side_exit_exactly() {
    in_thread(|| {
        let mut engine = crate::vm::Engine::new();
        let value = engine
            .eval_source(
                "let n=211,s=0;for(let i=0;i<n;i++){if(i===100)n=133;s+=i;}s",
                "dynamic-bound.js",
            )
            .unwrap();
        assert_eq!(value.as_f64(), 8778.0);
        if crate::jit::NATIVE_SUPPORTED && std::env::var("JHOL_JIT_ENABLE").as_deref() != Ok("0") {
            assert!(
                engine.vm.completed_loop_count() > 0,
                "dynamic loop must finish natively"
            );
        }
        let mut engine = crate::vm::Engine::new();
        let value = engine.eval_source(
            "const xs=[];for(let i=0;i<211;i++)xs.push({x:1});let s=0;for(let i=0;i<xs.length;i++)s+=xs[i].x;s",
            "array-bound.js",
        ).unwrap();
        assert_eq!(value.as_f64(), 211.0);
        if crate::jit::NATIVE_SUPPORTED && std::env::var("JHOL_JIT_ENABLE").as_deref() != Ok("0") {
            assert!(
                engine.vm.completed_loop_count() > 0,
                "array-bound loop must finish natively"
            );
        }
        // The arithmetic guard fails after earlier iterations have committed.
        // Resume at the failing addition, without repeating the loop increment.
        assert_eq!(eval_str("const xs=[];for(let i=0;i<211;i++)xs.push({x:i===100?2.5:1});let s=0;for(let i=0;i<xs.length;i++)s+=xs[i].x;s"), "212.5");
        // A getter in the test must run exactly once per test, including exit.
        assert_eq!(eval_str("let reads=0;const limit={get n(){reads++;return 137;}};let s=0;for(let i=0;i<limit.n;i++)s+=i;[s,reads].join(',')"), "9316,138");
        // A shape/accessor miss must not replay earlier observable calls.
        assert_eq!(eval_str("let reads=0;const xs=[];for(let i=0;i<211;i++)xs.push(i===100?{get x(){reads++;return 7;}}:{x:1});let s=0;for(let i=0;i<xs.length;i++)s+=xs[i].x;[s,reads].join(',')"), "217,1");
    });
}

#[test]
fn computed_member_delete_removes_the_property() {
    in_thread(|| {
        // `delete obj[expr]` must actually delete: the computed key lives in a
        // register, but DeleteProp only read the inline cache (which only
        // dot-syntax populates), so computed deletes were silently no-ops.
        assert_eq!(
            eval_str("const o={a:1,b:2}; delete o['b']; o.b===undefined ? 1 : 0"),
            "1"
        );
        // sequential computed deletes across a wide object (index rebuild path)
        assert_eq!(
            eval_str(
                "const o={}; for(let i=0;i<50;i++) o['k'+i]=i; \
                 for(let i=0;i<25;i++) delete o['k'+i]; let n=0; for(let k in o) n++; n"
            ),
            "25"
        );
        // survivors keep their values
        assert_eq!(
            eval_str(
                "const o={}; for(let i=0;i<50;i++) o['k'+i]=i; \
                 for(let i=0;i<25;i++) delete o['k'+i]; o['k40']"
            ),
            "40"
        );
    });
}

/// The formatter must preserve the *value* of every template quasi. Cooked
/// quasis are re-escaped on output; before, `\`` printed as a bare backtick
/// (unparseable) and `\\`, `\${`, `\r` silently changed meaning.
#[test]
fn formatter_preserves_template_quasi_values() {
    let programs = [
        r#"`a \` b`"#,
        r#"`back\\slash`"#,
        r#"`not \${a hole}`"#,
        r#"`dollar $ alone and $${1+1}`"#,
        r#"`cr\rlf\n tab\t nul\x00 bel\x07`"#,
        r#"`two\nlines ${"x"} end`"#,
        r#"((s, ...v) => s.join("|"))`p\`q${1}r`"#,
    ];
    for src in programs {
        let formatted = crate::formatter::format_source(src, "t.js").expect("format");
        // Compare through JSON so control characters are visible in a failure.
        let wrap = |s: &str| format!("JSON.stringify({})", s.trim_end().trim_end_matches(';'));
        let want = in_thread({
            let s = wrap(src);
            move || eval_str(&s)
        });
        let got = in_thread({
            let s = wrap(&formatted);
            move || eval_str(&s)
        });
        assert_eq!(
            got, want,
            "value changed by formatting {src:?} -> {formatted:?}"
        );
        // And idempotent: formatting the output again must be a no-op.
        let again = crate::formatter::format_source(&formatted, "t.js").expect("reformat");
        assert_eq!(again, formatted, "not idempotent for {src:?}");
    }
}

/// An empty statement as a loop/if body must survive formatting. It used to
/// print as nothing, leaving `if (c) ` before `else` (unparseable) — the shape
/// rollup's tree-shaker emits, found in axios's published bundles.
#[test]
fn formatter_keeps_empty_statement_bodies_parseable() {
    let programs = [
        "var r = 'x'; if (r === 'y') ; else if (r === 'x') { r = 'ok'; } else { r = 'no'; } r",
        "var n = 0; for (; n < 5; n++); n",
        "var i = 0; while (i++ < 3); i",
        "var i = 0; do ; while (i++ < 3); i",
        "var k = 0; if (k) ; k",
    ];
    for src in programs {
        let formatted = crate::formatter::format_source(src, "t.js").expect("format");
        crate::parser::parse_program(&formatted)
            .unwrap_or_else(|e| panic!("formatted output does not parse: {e}\n{formatted}"));
        let want = in_thread({
            let s = src.to_string();
            move || eval_str(&s)
        });
        let got = in_thread({
            let s = formatted.clone();
            move || eval_str(&s)
        });
        assert_eq!(got, want, "value changed: {src:?} -> {formatted:?}");
    }
}

/// `async` starting a line is a fresh statement; only a newline *between*
/// `async` and `function` makes it an identifier (restricted production).
#[test]
fn async_function_may_start_a_line_after_another_statement() {
    for src in [
        "const x = 1;\nasync function f() {}\nx",
        "var a = 1\nasync function* g() { yield 1 }\na",
        "function o() { let s = r;\n  async function* e() { let t = 0; }\n  return 1; }\no()",
    ] {
        crate::parser::parse_program(&src.replace("r;", "1;"))
            .unwrap_or_else(|e| panic!("should parse: {e}\n{src}"));
    }
    // The restricted production still holds: `async` + newline + `function`
    // is the identifier `async` followed by a function declaration.
    let p = crate::parser::parse_program("var async = 1\nasync\nfunction f() {}\n");
    assert!(p.is_ok(), "ASI case must parse: {:?}", p.err());
}

/// `class extends Base {}` (anonymous class expression) must not treat
/// `extends` as the class name.
#[test]
fn anonymous_class_expression_with_extends() {
    let got = in_thread(|| {
        eval_str(
            "class B { who() { return 'b'; } }\n\
             const C = class extends B { who() { return super.who() + 'c'; } };\n\
             new C().who() + ':' + (C.name === 'C')",
        )
    });
    assert_eq!(got, "bc:true");
}

/// Accessors with computed keys must keep their key through formatting
/// (`get [Symbol.toStringTag]()` used to print as `get ()`).
#[test]
fn formatter_keeps_computed_accessor_keys() {
    let src = "class A { get [Symbol.toStringTag]() { return 'X'; } set [\"k\"](v) {} }";
    let out = crate::formatter::format_source(src, "t.js").expect("format");
    assert!(
        out.contains("get [Symbol.toStringTag]()"),
        "key dropped: {out}"
    );
    assert!(out.contains("set [\"k\"](v)"), "key dropped: {out}");
    crate::parser::parse_program(&out).expect("output must parse");
    let again = crate::formatter::format_source(&out, "t.js").expect("reformat");
    assert_eq!(again, out, "not idempotent");
}

#[test]
fn static_blocks_run_with_class_as_this_in_source_order() {
    let got = in_thread(|| {
        eval_str(
            "class A {\n\
               static x = 1;\n\
               static { this.x = this.x + 1; this.y = 3; }\n\
               static z = this.x + 10;\n\
               static { this.y = this.z; }\n\
             }\n\
             `${A.x},${A.y},${A.z}`",
        )
    });
    assert_eq!(got, "2,12,12");
}

#[test]
fn computed_method_and_static_field_keys_are_evaluated() {
    let got = in_thread(|| {
        eval_str(
            "const k = 'm';\n\
             class A { [k]() { return 1; } static [k + '2'] = 5; get [k + 'g']() { return 7; } }\n\
             const a = new A();\n\
             `${a.m()},${A.m2},${a.mg}`",
        )
    });
    assert_eq!(got, "1,5,7");
}

#[test]
fn generator_resumes_with_sent_value_and_returns_result() {
    let got = in_thread(|| {
        eval_str(
            "function* g() { const x = yield 1; return x + 1; }\n\
             const it = g();\n\
             const a = it.next();\n\
             const b = it.next(41);\n\
             `${a.value},${a.done},${b.value},${b.done}`",
        )
    });
    assert_eq!(got, "1,false,42,true");
}

#[test]
fn yield_star_delegates_and_forwards_sent_values() {
    let got = in_thread(|| {
        eval_str(
            "function* inner() { const x = yield 1; return x + 1; }\n\
             function* outer() { const r = yield* inner(); return r; }\n\
             const it = outer();\n\
             const a = it.next();\n\
             const b = it.next(41);\n\
             `${a.value},${a.done},${b.value},${b.done}`",
        )
    });
    assert_eq!(got, "1,false,42,true");
}

#[test]
fn primitive_proto_getter_uses_wrapper_prototypes() {
    let got = in_thread(|| {
        eval_str(
            "`${(5).__proto__ === Number.prototype},${('').__proto__ === String.prototype},\
             ${(true).__proto__ === Boolean.prototype},${(true).toString()},\
             ${[].hasOwnProperty('length')},${Object.prototype.isPrototypeOf([])}`",
        )
    });
    assert_eq!(got, "true,true,true,true,true,true");
}

#[test]
fn async_and_await_fail_loudly_instead_of_running_wrong() {
    in_thread(|| {
        let e = eval("async function f() {}\nf").unwrap_err();
        assert!(
            e.to_string().contains("async functions are not supported"),
            "{e}"
        );
        let e = eval("async function f() { return await 1; }\nf").unwrap_err();
        assert!(
            e.to_string().contains("async functions are not supported"),
            "{e}"
        );
    });
}

#[test]
fn formatter_round_trips_static_blocks_and_computed_keys() {
    let src = "class A {\n  static x = 1;\n  static {\n    this.y = 2;\n  }\n  [\"m\"]() {\n    return 3;\n  }\n}\n";
    let out = crate::formatter::format_source(src, "t.js").expect("format");
    assert!(out.contains("static {"), "static block dropped: {out}");
    assert!(out.contains("[\"m\"]()"), "computed method dropped: {out}");
    let again = crate::formatter::format_source(&out, "t.js").expect("reformat");
    assert_eq!(again, out, "not idempotent");
}

// ------------------------- lexer/parser regressions -------------------------

fn fmt_roundtrip(src: &str) -> String {
    let out = crate::formatter::format_source(src, "t.js").expect("format");
    let again = crate::formatter::format_source(&out, "t.js").expect("reformat");
    assert_eq!(again, out, "not idempotent");
    out
}

#[test]
fn keyword_property_names_take_division_not_regex() {
    // `a.else / e` — `else` after `.` is a property name; the `/` is division
    // (regression: the nft bundle lexed this as a regex and desynced).
    fmt_roundtrip("const r = a.else / e;\n");
    fmt_roundtrip("const r = x.in / 2;\n");
    fmt_roundtrip("const r = arr.with / 2;\n");
}

#[test]
fn contextual_keywords_can_be_identifiers() {
    // `as`/`from` are import/export clauses only; as bindings, `/` after
    // them is division (regression: pdf.worker's `M + as / 500`).
    fmt_roundtrip("let as = 3;\nconst L = M + as / 500;\n");
    fmt_roundtrip("let from = 1;\nconst x = from / 2;\n");
}

#[test]
fn bigints_beyond_i64_round_trip() {
    let out = fmt_roundtrip("const A = 0xffffffffffffff80n;\nconst B = 9223372036854775808n;\n");
    assert!(
        out.contains("18446744073709551488n"),
        "hex bigint lost: {out}"
    );
    assert!(
        out.contains("9223372036854775808n"),
        "i64-overflow bigint lost: {out}"
    );
}

#[test]
fn with_statement_parses_and_formats() {
    let out = fmt_roundtrip("with (obj) {\n  x = 1;\n}\n");
    assert!(out.contains("with (obj)"), "with dropped: {out}");
    // `with` as a method name (Array.prototype.with) still works.
    fmt_roundtrip("const y = arr.with(0, 1);\n");
    // And the VM rejects `with` with a clear error, not a crash.
    in_thread(|| {
        let e = eval("with ([]) { x = 1; }").unwrap_err();
        assert!(e.to_string().contains("`with` is not supported"), "{e}");
    });
}

#[test]
fn bare_yield_comma_terminates() {
    fmt_roundtrip("function* g() {\n  let [a] = yield, b = 1;\n}\n");
}

#[test]
fn let_of_binding_and_for_of() {
    fmt_roundtrip("let of = 1;\nfor (let of of xs) {\n  use(of);\n}\n");
}

#[test]
fn nbsp_and_ls_ps_are_whitespace() {
    fmt_roundtrip("let\u{a0}a = 1;\n");
    fmt_roundtrip("let a = 1;\u{2028}let b = 2;\n");
}

#[test]
fn escaped_unicode_identifiers() {
    let out = fmt_roundtrip("let \\u0061b = 1;\nlet c\\u{41} = 2;\n");
    assert!(
        out.contains("ab") || out.contains("\\u0061b"),
        "escaped ident lost: {out}"
    );
}

#[test]
fn in_operator_inside_for_head_parens() {
    fmt_roundtrip("for (let x = (\"a\" in o);;) {\n  break;\n}\n");
    fmt_roundtrip("for (const k in o) {\n  use(k);\n}\n");
}

#[test]
fn regex_after_control_header() {
    fmt_roundtrip("if (a) /re/.test(s);\n");
    fmt_roundtrip("while (b) /x/.exec(s);\n");
}

#[test]
fn destructuring_targets_in_for_of_and_assignment() {
    fmt_roundtrip("for (const [a, b] of pairs) {\n  use(a, b);\n}\n");
    fmt_roundtrip("const { x = 1, ...rest } = o;\n");
    fmt_roundtrip("({ a: [b, ...c] } = src);\n");
}

#[cfg(test)]
mod tdz_and_scope_tests {
    use crate::vm::Engine;

    fn run(src: &str) -> f64 {
        let mut e = Engine::new();
        e.ensure_started();
        e.eval_source(src, "<tdz-test>")
            .expect("program must run")
            .as_f64()
    }

    /// The temporal dead zone, for the cases that are decidable at compile time:
    /// a read of a block-scoped name before its declaration in the same function.
    #[test]
    fn reading_a_let_before_its_declaration_throws() {
        assert_eq!(
            run(r#"(function(){ try { let a = a; return 0 } catch (e) { return 1 } })()"#),
            1.0
        );
        assert_eq!(
            run(r#"(function(){ try { { q; let q = 1; } return 0 } catch (e) { return 1 } })()"#),
            1.0
        );
        // The message must name the binding, because "cannot access before
        // initialization" without a name is a riddle.
        assert_eq!(
            run(r#"(function(){ try { let zzz = zzz } catch (e) { return e.message.indexOf("zzz") >= 0 ? 1 : 0 } })()"#),
            1.0
        );
    }

    #[test]
    fn legal_scope_uses_stay_legal() {
        // Reading after the declaration.
        assert_eq!(run(r#"(function(){ let a = 1; return a + 1 })()"#), 2.0);
        // `var` is hoisted and initialised to undefined: no dead zone.
        assert_eq!(
            run(r#"(function(){ var v = v; return v === undefined ? 1 : 0 })()"#),
            1.0
        );
        // A closure created before the declaration reads it later, which is legal.
        assert_eq!(
            run(r#"(function(){ let f = function(){ return x }; let x = 5; return f() })()"#),
            5.0
        );
    }

    /// Arrow functions used to start with no scope at all, so any `let`/`const` or
    /// destructuring in an arrow body panicked the compiler (`Option::unwrap` on a
    /// None scope). `var` worked by accident, which is why it survived.
    #[test]
    fn arrow_bodies_can_declare_bindings() {
        assert_eq!(run(r#"(() => { let z = 3; return z + 1 })()"#), 4.0);
        assert_eq!(run(r#"(() => { const {a, b} = {a: 1, b: 2}; return a + b })()"#), 3.0);
        assert_eq!(run(r#"(() => { var v = 7; return v })()"#), 7.0);
    }
}

#[cfg(test)]
mod strict_assignment_tests {
    use crate::vm::Engine;

    fn run(src: &str) -> String {
        let mut e = Engine::new();
        e.ensure_started();
        let v = e.eval_source(src, "<strict-test>").expect("program must run");
        format!("{:?}", v)
    }

    /// Strict code may not create a global by assignment, and the message names it.
    #[test]
    fn strict_functions_throw_on_undeclared_assignment() {
        let out = run(
            r#"(function(){ "use strict"; try { undeclared_zzz = 1; return "no throw" }
                catch (e) { return e.message } })()"#,
        );
        assert!(out.contains("undeclared_zzz is not defined"), "{out}");
    }

    /// Class bodies are always strict, even without a directive.
    #[test]
    fn class_methods_are_strict() {
        let out = run(
            r#"class C { m(){ try { undeclared_qqq = 1; return "no throw" }
                        catch (e) { return e.message } } }
               new C().m()"#,
        );
        assert!(out.contains("undeclared_qqq is not defined"), "{out}");
    }

    /// Sloppy code keeps the old behaviour: the assignment creates a global.
    #[test]
    fn sloppy_functions_still_create_globals() {
        let out = run(r#"(function(){ sloppy_one = 41; return sloppy_one + 1 })()"#);
        assert!(out.contains("42"), "{out}");
    }


    /// A rejection nobody handled must fail the program. It used to exit 0 with
    /// no output, so a failed async operation looked like a clean run.
    #[test]
    fn unhandled_rejection_fails_the_program() {
        let mut e = Engine::new();
        e.ensure_started();
        let err = e
            .eval_source(r#"Promise.reject(new Error("boom")); 1"#, "<unhandled-test>")
            .expect_err("an unhandled rejection must be an error");
        let message = format!("{err}");
        assert!(message.contains("UnhandledPromiseRejection"), "{message}");
        assert!(message.contains("boom"), "{message}");
    }

    /// A rejection that *is* handled, by either form, stays quiet and returns.
    #[test]
    fn handled_rejection_is_not_reported() {
        for src in [
            r#"Promise.reject(new Error("x")).catch(() => 1); 2"#,
            r#"Promise.reject(new Error("x")).then(null, () => 1); 2"#,
        ] {
            let mut e = Engine::new();
            e.ensure_started();
            e.eval_source(src, "<handled-test>")
                .unwrap_or_else(|err| panic!("handled rejection reported: {err} in {src}"));
        }
    }

    /// The script itself is not strict: node runs `-e` scripts as sloppy code, and a
    /// package manager that made them strict would diverge on one-liners.
    #[test]
    fn scripts_stay_sloppy() {
        let out = run(r#"script_level_name = 7; script_level_name"#);
        assert!(out.contains("7"), "{out}");
    }

    /// Assigning to a name that *does* exist on the global object is fine in strict
    /// code: the reference resolves.
    #[test]
    fn existing_globals_are_writable_in_strict_code() {
        let out = run(
            r#"globalThis.existing_www = 1;
               (function(){ "use strict"; existing_www = 2; return existing_www })()"#,
        );
        assert!(out.contains("2"), "{out}");
    }
}
