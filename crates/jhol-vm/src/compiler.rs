//! AST → bytecode.
//!
//! # Register layout of a frame
//!
//! ```text
//! slot 0      `this`        (non-arrow functions only; the VM copies frame.this in)
//! slot 1      `new.target`  (non-arrow functions only)
//! slot 2..    parameters, then function-scope `var`/`let`/`const` locals,
//!             then evaluation temporaries
//! ```
//!
//! Arrows own no `this`: they resolve it up the enclosing function chain exactly
//! like a captured local, which is why an arrow reads a cell while a normal
//! function reads its own slot.
//!
//! # Calling conventions
//!
//! | op            | register layout                                        |
//! |---------------|--------------------------------------------------------|
//! | `Call`        | `[callee, arg0..argN]` → `this = undefined`            |
//! | `CallMethod`  | `[receiver, arg0..argN]`, key from `ics[ip].key`       |
//! | `CallThis`    | `[this, callee, arg0..argN]`                           |
//! | `Construct`   | `[callee, arg0..argN]`, `this` = fresh object         |
//! | `NewArray`    | `[elem0..elemN]`, result written at `dst + N`          |
//!
//! # Closures
//!
//! A local is boxed into a `JCell` at the moment the first closure that
//! references it is compiled (`CreateCell`, emitted immediately before
//! `NewClosure`); from then on that slot is read/written with `CellGet` /
//! `CellSet` in this frame too. Nothing is paid at function entry, so functions
//! that capture nothing are exactly as cheap as top-level code.

use crate::arena;
use crate::ast::*;
use crate::bytecode::*;
use crate::objects::PropKey;
use crate::value::Value;

/// Key for the iterator protocol. A well-known string rather than a real
/// `Symbol`: only this engine looks it up, so it costs a pointer compare.
pub const ITER_KEY: &str = "@@iterator";
pub const ENUM_KEYS: &str = "__jhol_enum_keys";
pub const VALUES: &str = "__jhol_values";
pub const REST_OBJ: &str = "__jhol_rest_object";

const THIS_SLOT: u32 = 0;
const NEWTARGET_SLOT: u32 = 1;

#[derive(Clone, Copy, Debug)]
struct Binding {
    slot: u32,
    captured: bool,
}

#[derive(Debug, Default)]
struct Scope {
    names: Vec<(String, Binding)>,
}

#[derive(Clone, Debug)]
struct LabelCtx {
    name: Option<String>,
    breaks: Vec<usize>,
    continues: Vec<usize>,
    cont_ip: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FnCtx {
    Script,
    Func,
    Method,
    /// A method inside a `class` body. Class bodies are always strict, unlike
    /// object-literal methods, so the two need separate contexts.
    ClassMethod,
    Ctor,
    DerivedCtor,
    Arrow,
}

#[derive(Clone, Copy, Debug)]
struct UpvalDesc {
    from_local: bool,
    index: u32,
}

#[allow(dead_code)]
struct FnState {
    /// Exception handlers belong to this function, never its lexical parent.
    handler_stack: Vec<u32>,
    proto: FnProto,
    scopes: Vec<Scope>,
    parent: Option<usize>,
    upvals: Vec<UpvalDesc>,
    captured: Vec<bool>,
    cellified: Vec<bool>,
    nparams: usize,
    locals_top: usize,
    temps_top: usize,
    /// High-water mark of every register index ever touched. `free_temps_to`
    /// can lower `temps_top`, so the frame size must be tracked separately.
    max_slots: usize,
    labels: Vec<LabelCtx>,
    ctx: FnCtx,
    is_async: bool,
    is_generator: bool,
    /// Local slots currently in string-accumulator mode: `s += x` on them emits
    /// `AppendStr` into a frame buffer instead of building a new string. The
    /// compiler knows every use of a local, so it can flush before any read and
    /// never has to reason about aliasing.
    accums: std::collections::HashSet<u32>,
    /// Slots whose value is *known* to be a string, so `+=` on them is string
    /// concatenation rather than numeric addition. Without this, `sum += n`
    /// would accumulate as text and `console.log(sum)` would print "0n".
    accum_ok: std::collections::HashSet<u32>,
}

impl FnState {
    #[allow(dead_code)]
    fn is_root(&self) -> bool {
        self.parent.is_none()
    }
}

pub struct Compiler {
    states: Vec<FnState>,
    /// `let`/`const` names declared *later* in each enclosing block, with the
    /// function-state index they belong to. A read of such a name before its
    /// declaration must throw instead of silently reading a global — the part of
    /// the temporal dead zone that is decidable at compile time.
    pending_lexical: Vec<(usize, std::collections::HashSet<String>)>,
    protos: Vec<FnProto>,
    /// A label handed down to the next loop construct, so `continue <label>`
    /// reaches the loop's own continue point rather than its end.
    pending_label: Option<String>,
    /// Hidden binding names of the classes being compiled, innermost last, each
    /// with whether the member being compiled is `static`. `super.x` resolves
    /// through the *home* class (`Home.prototype.__proto__`, or `Home.__proto__`
    /// for statics) instead of the receiver's `__proto__`: the latter is the
    /// most-derived class and recursed forever in any method that calls
    /// `super.m()` (found 2026-10-24; broken at HEAD too).
    home_stack: Vec<(String, bool)>,
    class_serial: u32,
    /// Statements of the `finally` blocks currently being compiled, innermost
    /// last. A `return` inside a `try` has to run them before it leaves, which
    /// is what JS does on abrupt completion.
    finally_stack: Vec<Vec<Stmt>>,
    /// Depth of finally bodies being emitted right now, so a `return` inside a
    /// `finally` cannot recurse into the same block.
    in_finally: u32,
    /// Binding name offered to the *directly* initialising anonymous function,
    /// arrow or class (`const f = () => 1` names it "f"). Consumed by the first
    /// such expression compiled and cleared on any other expression, so it
    /// cannot leak into a nested function.
    name_hint: Option<String>,
}

pub fn compile(prog: &Func, name: &str) -> Result<Module, String> {
    let mut c = Compiler {
        states: Vec::new(),
        protos: Vec::new(),
        pending_label: None,
        home_stack: Vec::new(),
        class_serial: 0,
        name_hint: None,
        finally_stack: Vec::new(),
        in_finally: 0,
        pending_lexical: Vec::new(),
    };
    c.push_fn(prog, FnCtx::Script, Some(name.to_string()));
    // A module entry can carry parameters (CommonJS wrappers are compiled
    // through this path). Bind them exactly as `compile_fn` does — without
    // this, the parameter names resolved as GLOBALS and a module body saw
    // `undefined` for `module`/`exports`/`require`.
    if !prog.params.is_empty() {
        let base: u32 = if prog.is_arrow { 0 } else { 2 };
        for _ in prog.params.iter() {
            c.add_local("\u{0}param");
        }
        for (i, p) in prog.params.iter().enumerate() {
            let slot = base + i as u32;
            if let Target::Simple(n) = &p.target {
                c.cur().scopes.last_mut().unwrap().names.push((
                    n.clone(),
                    Binding {
                        slot,
                        captured: false,
                    },
                ));
            }
        }
    }
    let mut hoisted: Vec<String> = Vec::new();
    c.collect_vars(&prog.body, &mut hoisted);
    for h in &hoisted {
        if c.find_binding(h).is_none() {
            let s = c.add_local(h);
            c.emit(Op::LoadUndefined, s, 0, 0);
        }
    }
    c.hoist_functions(&prog.body)?;
    // The value of a script is the value of its last expression statement, so
    // that one is compiled into a dedicated completion register.
    let last_expr = prog
        .body
        .iter()
        .rposition(|s| matches!(s, Stmt::Expr(_) | Stmt::Try(..) | Stmt::If(..)));
    let done = c.add_local("\u{0}completion");
    c.emit(Op::LoadUndefined, done, 0, 0);
    for (i, s) in prog.body.iter().enumerate() {
        if Some(i) == last_expr {
            c.completion_value(s, done)?;
            continue;
        }
        c.stmt(s)?;
    }
    c.emit(Op::Return, done, 0, 0);
    c.finish_fn();
    // Nested functions are compiled (and pushed) before their parent, so the
    // top-level proto is always the last one.
    let entry = (c.protos.len() - 1) as u32;
    let protos: Vec<_> = c.protos.into_iter().map(std::sync::Arc::new).collect();
    Ok(Module {
        protos,
        entry,
        name: name.to_string(),
    })
}

// ---------------------------------------------------------------------------
// scaffolding
// ---------------------------------------------------------------------------

impl Compiler {
    fn cur(&mut self) -> &mut FnState {
        self.states.last_mut().unwrap()
    }

    fn emit(&mut self, op: Op, a: u32, b: u32, c: u32) -> usize {
        let fs = self.states.last_mut().unwrap();
        fs.proto.code.push(Instr::new(op, a, b, c));
        fs.proto.ics.push(Ic::default());
        fs.proto.code.len() - 1
    }

    fn here(&self) -> usize {
        self.states.last().unwrap().proto.code.len()
    }

    fn patch_to(&mut self, at: usize, target: usize) {
        self.cur().proto.code[at].c = target as u32;
    }

    fn set_ic_key(&mut self, at: usize, key: PropKey) {
        let fs = self.states.last_mut().unwrap();
        fs.proto.ics[at].key = key;
    }

    fn alloc_temp(&mut self) -> u32 {
        let fs = self.states.last_mut().unwrap();
        if fs.temps_top < fs.locals_top {
            fs.temps_top = fs.locals_top;
        }
        let t = fs.temps_top as u32;
        fs.temps_top += 1;
        if fs.temps_top > fs.max_slots {
            fs.max_slots = fs.temps_top;
        }
        t
    }

    /// Reserve `n` contiguous temps. Call arguments have to be adjacent — the
    /// VM reads them as `arg_base..arg_base + n` — so they are claimed as one
    /// block rather than one `alloc_temp()` at a time.
    fn alloc_temps(&mut self, n: u32) -> u32 {
        let base = self.alloc_temp();
        for _ in 1..n {
            self.alloc_temp();
        }
        base
    }

    fn free_temps_to(&mut self, base: u32) {
        let fs = self.states.last_mut().unwrap();
        fs.temps_top = (base as usize).max(fs.locals_top);
    }

    fn add_local(&mut self, name: &str) -> u32 {
        let fs = self.states.last_mut().unwrap();
        if fs.temps_top < fs.locals_top {
            fs.temps_top = fs.locals_top;
        }
        let slot = fs.locals_top as u32;
        fs.locals_top += 1;
        if fs.locals_top > fs.max_slots {
            fs.max_slots = fs.locals_top;
        }
        if fs.captured.len() <= slot as usize {
            fs.captured.resize(slot as usize + 1, false);
            fs.cellified.resize(slot as usize + 1, false);
        }
        if fs.scopes.is_empty() {
            fs.scopes.push(Scope::default());
        }
        fs.scopes.last_mut().unwrap().names.push((
            name.to_string(),
            Binding {
                slot,
                captured: false,
            },
        ));
        slot
    }

    fn push_scope(&mut self) {
        self.cur().scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.cur().scopes.pop();
    }

    fn lookup_in(&self, idx: usize, name: &str) -> Option<Binding> {
        for sc in self.states[idx].scopes.iter().rev() {
            for (n, b) in sc.names.iter().rev() {
                if n == name {
                    return Some(*b);
                }
            }
        }
        None
    }

    fn find_binding(&self, name: &str) -> Option<Binding> {
        self.lookup_in(self.states.len() - 1, name)
    }

    fn resolve_upval(&mut self, fs_idx: usize, name: &str) -> Option<u32> {
        let parent = self.states[fs_idx].parent?;
        if let Some(b) = self.lookup_in(parent, name) {
            if !b.captured {
                let ps = &mut self.states[parent];
                if ps.captured.len() <= b.slot as usize {
                    ps.captured.resize(b.slot as usize + 1, false);
                    ps.cellified.resize(b.slot as usize + 1, false);
                }
                ps.captured[b.slot as usize] = true;
                let newb = Binding {
                    slot: b.slot,
                    captured: true,
                };
                for sc in ps.scopes.iter_mut().rev() {
                    let mut done = false;
                    for (n, bb) in sc.names.iter_mut().rev() {
                        if n == name {
                            *bb = newb;
                            done = true;
                            break;
                        }
                    }
                    if done {
                        break;
                    }
                }
            }
            return Some(self.push_upval(fs_idx, b.slot, true));
        }
        if let Some(u) = self.resolve_upval(parent, name) {
            return Some(self.push_upval(fs_idx, u, false));
        }
        None
    }

    fn push_upval(&mut self, fs_idx: usize, index: u32, from_local: bool) -> u32 {
        // Reads and writes of one captured binding share one descriptor/cell.
        if let Some(slot) = self.states[fs_idx]
            .upvals
            .iter()
            .position(|up| up.index == index && up.from_local == from_local)
        {
            return slot as u32;
        }
        let idx = self.states[fs_idx].upvals.len() as u32;
        self.states[fs_idx]
            .upvals
            .push(UpvalDesc { from_local, index });
        idx
    }

    fn push_fn(&mut self, f: &Func, ctx: FnCtx, name: Option<String>) {
        let parent = self.states.len().checked_sub(1);
        let mut fs = FnState {
            handler_stack: Vec::new(),
            proto: FnProto::new(name.as_deref().unwrap_or("")),
            scopes: Vec::new(),
            parent,
            upvals: Vec::new(),
            captured: Vec::new(),
            cellified: Vec::new(),
            nparams: f.params.len(),
            locals_top: 0,
            temps_top: 0,
            max_slots: 0,
            labels: Vec::new(),
            ctx,
            is_async: f.is_async,
            is_generator: f.is_generator,
            accums: std::collections::HashSet::new(),
            accum_ok: std::collections::HashSet::new(),
        };
        fs.proto.is_arrow = f.is_arrow;
        // Strictness is a property of the *function*, not a global switch.
        // Classes, methods and constructors are always strict; a plain function
        // is strict only when its body opens with the "use strict" directive.
        // Everything else runs sloppy, which is how node runs CommonJS `.js`
        // files — the difference is visible in `this` (globalThis vs undefined).
        let has_directive = match f.body.first() {
            Some(Stmt::Expr(e)) => matches!(e, Expr::Str(s) if s == "use strict"),
            _ => false,
        };
        fs.proto.is_strict = matches!(
            ctx,
            FnCtx::ClassMethod | FnCtx::Ctor | FnCtx::DerivedCtor | FnCtx::Script
        ) || has_directive;
        fs.proto.is_script = matches!(ctx, FnCtx::Script);
        // `async` rides the generator machinery: the body compiles as a generator,
        // `await e` becomes `yield e`, and the call site drives it. Nothing is
        // transpiled to callbacks — suspension is the engine's own.
        fs.proto.is_generator = f.is_generator || f.is_async;
        fs.proto.is_async = f.is_async;
        // Every function starts with a scope, arrows included. An arrow has no own
        // `this`/`new_target` (both are lexical, resolved as upvalues) so it gets an
        // empty one — but it must exist: `declare_pattern` assumes a scope is present,
        // and an arrow body declaring `let`/`const` or destructuring panicked with
        // `Option::unwrap()` on a `None` scope until this was fixed. `var` worked by
        // accident, which is why the gap survived.
        let mut scope = Scope {
            names: Vec::new(),
        };
        if !f.is_arrow {
            scope.names.push((
                "\u{0}this".into(),
                Binding {
                    slot: THIS_SLOT,
                    captured: false,
                },
            ));
            scope.names.push((
                "\u{0}newtarget".into(),
                Binding {
                    slot: NEWTARGET_SLOT,
                    captured: false,
                },
            ));
            fs.locals_top = 2;
            fs.captured = vec![false, false];
            fs.cellified = vec![false, false];
        }
        fs.scopes.push(scope);
        self.states.push(fs);
    }

    fn finish_fn(&mut self) {
        let fs = self.states.pop().unwrap();
        let mut proto = fs.proto;
        proto.local_cells = fs
            .captured
            .iter()
            .enumerate()
            .filter_map(|(i, &captured)| captured.then_some(i as u32))
            .collect();
        proto.nslots = fs.max_slots.max(fs.locals_top).max(fs.temps_top).max(2);
        proto.nparams = fs.nparams;
        proto.nupvals = fs.upvals.len();
        proto.upval_desc = fs.upvals.iter().map(|u| (u.from_local, u.index)).collect();
        self.protos.push(proto);
    }

    /// Emit `CreateCell` for every local of the current frame that a nested
    /// closure captured and that has not been boxed yet.
    fn cellify_pending(&mut self) {
        let fs = self.states.last_mut().unwrap();
        let n = fs.captured.len();
        let mut ops: Vec<u32> = Vec::new();
        for slot in 0..n {
            if fs.captured[slot] && !fs.cellified[slot] {
                fs.cellified[slot] = true;
                ops.push(slot as u32);
            }
        }
        for slot in ops {
            fs.proto.code.push(Instr::new(Op::CreateCell, slot, 0, 0));
            fs.proto.ics.push(Ic::default());
        }
    }

    /// Compile a nested function and return its index in the module's proto list.
    fn compile_fn(&mut self, f: &Func, ctx: FnCtx, name: Option<String>) -> Result<u32, String> {
        self.push_fn(f, ctx, name);
        let base: u32 = if f.is_arrow { 0 } else { 2 };
        // Reserve one slot per parameter, in order.
        for p in f.params.iter() {
            self.add_local("\u{0}param");
            let _ = p;
        }
        // Simple parameters take their real names.
        for (i, p) in f.params.iter().enumerate() {
            let slot = base + i as u32;
            if let Target::Simple(n) = &p.target {
                self.cur().scopes.last_mut().unwrap().names.push((
                    n.clone(),
                    Binding {
                        slot,
                        captured: false,
                    },
                ));
            }
        }
        // Parameter defaults.
        for (i, p) in f.params.iter().enumerate() {
            if let Some(def) = &p.default {
                let slot = base + i as u32;
                let cur = self.alloc_temp();
                self.emit(Op::GetLocal, cur, slot, 0);
                // `const_undef` returns a *constant index*, not a register. It
                // has to be loaded before the comparison; passing it straight to
                // StrictEq compared the parameter against whatever sat in the
                // register with that number, so a default fired for a parameter
                // that had an argument and was skipped for one that did not.
                let und = self.alloc_temp();
                let ku = self.const_undef();
                self.emit(Op::LoadK, und, ku, 0);
                let cmp = self.alloc_temp();
                self.emit(Op::StrictEq, cmp, cur, und);
                let skip = self.emit(Op::JumpIfFalse, cmp, 0, 0);
                let dv = self.alloc_temp();
                self.expr(def, dv)?;
                self.emit(Op::SetLocal, slot, dv, 0);
                self.free_temps_to(cur);
                let after = self.here();
                self.patch_to(skip, after);
            }
        }
        // Rest parameters.
        for (i, p) in f.params.iter().enumerate() {
            if !p.rest {
                continue;
            }
            let slot = base + i as u32;
            let dst = self.alloc_temp();
            self.emit(Op::BuildRest, dst, i as u32, 0);
            self.emit(Op::SetLocal, slot, dst, 0);
            self.free_temps_to(dst);
            if let Target::Simple(n) = &p.target {
                self.cur().scopes.last_mut().unwrap().names.push((
                    n.clone(),
                    Binding {
                        slot,
                        captured: false,
                    },
                ));
            }
        }
        // Destructuring parameters.
        for (i, p) in f.params.iter().enumerate() {
            if matches!(p.target, Target::Simple(_)) {
                continue;
            }
            let slot = base + i as u32;
            let t = self.alloc_temp();
            self.emit(Op::GetLocal, t, slot, 0);
            self.bind(&p.target, t)?;
            self.free_temps_to(t);
        }

        let mut hoisted: Vec<String> = Vec::new();
        self.collect_vars(&f.body, &mut hoisted);
        for h in &hoisted {
            if self.find_binding(h).is_none() {
                let s = self.add_local(h);
                self.emit(Op::LoadUndefined, s, 0, 0);
            }
        }
        self.hoist_functions(&f.body)?;
        match &f.expr_body {
            Some(e) => {
                let t = self.alloc_temp();
                self.expr(e, t)?;
                self.emit(Op::Return, t, 0, 0);
                self.free_temps_to(t);
            }
            None => {
                for st in &f.body {
                    self.stmt(st)?;
                }
                // An explicit `return` already left the function. A second
                // `Return this` is dead and keeps the self-recursion compiler
                // from seeing a numeric body.
                let already = self
                    .cur()
                    .proto
                    .code
                    .last()
                    .is_some_and(|i| i.op == Op::Return);
                if !already {
                    self.emit(Op::Return, THIS_SLOT, 0, 0);
                }
            }
        }
        self.finish_fn();
        Ok(self.protos.len() as u32 - 1)
    }

    fn new_closure(
        &mut self,
        f: &Func,
        ctx: FnCtx,
        name: Option<String>,
        dst: u32,
    ) -> Result<(), String> {
        self.cellify_pending();
        let pidx = self.compile_fn(f, ctx, name)?;
        let k = self.const_fn(pidx);
        self.emit(Op::NewClosure, dst, k, 0);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // constants
    // -----------------------------------------------------------------------

    fn const_empty(&mut self) -> u32 {
        let fs = self.states.last_mut().unwrap();
        for (i, c) in fs.proto.consts.iter().enumerate() {
            if matches!(c, ConstVal::Empty) {
                return i as u32;
            }
        }
        fs.proto.consts.push(ConstVal::Empty);
        (fs.proto.consts.len() - 1) as u32
    }

    fn const_num(&mut self, n: f64) -> u32 {
        let fs = self.states.last_mut().unwrap();
        for (i, c) in fs.proto.consts.iter().enumerate() {
            if let ConstVal::Num(x) = c {
                if x.to_bits() == n.to_bits() {
                    return i as u32;
                }
            }
        }
        fs.proto.consts.push(ConstVal::Num(n));
        fs.proto.consts.len() as u32 - 1
    }

    fn const_atom(&mut self, v: Value) -> u32 {
        let fs = self.states.last_mut().unwrap();
        for (i, c) in fs.proto.consts.iter().enumerate() {
            if let ConstVal::Str(x) = c {
                if *x == v {
                    return i as u32;
                }
            }
        }
        fs.proto.consts.push(ConstVal::Str(v));
        fs.proto.consts.len() as u32 - 1
    }

    fn const_str(&mut self, s: &str) -> u32 {
        let atom = arena::active().intern_atom(s);
        self.const_atom(atom)
    }

    fn const_fn(&mut self, idx: u32) -> u32 {
        let fs = self.states.last_mut().unwrap();
        fs.proto.consts.push(ConstVal::Fn(idx));
        fs.proto.consts.len() as u32 - 1
    }

    /// Intern a literal's static key list as a const-pool entry for
    /// `Op::NewObjectLit`.
    fn const_keys(&mut self, keys: Vec<PropKey>) -> u32 {
        let fs = self.states.last_mut().unwrap();
        fs.proto
            .consts
            .push(ConstVal::Keys(keys.into_boxed_slice()));
        fs.proto.consts.len() as u32 - 1
    }

    fn const_undef(&mut self) -> u32 {
        let fs = self.states.last_mut().unwrap();
        for (i, c) in fs.proto.consts.iter().enumerate() {
            if matches!(c, ConstVal::Undefined) {
                return i as u32;
            }
        }
        fs.proto.consts.push(ConstVal::Undefined);
        fs.proto.consts.len() as u32 - 1
    }

    fn key_of(&mut self, s: &str) -> PropKey {
        let sym = arena::active().intern_sym(s);
        PropKey::Sym(sym)
    }
}

// ---------------------------------------------------------------------------
// statements
// ---------------------------------------------------------------------------

impl Compiler {
    /// Emit `slot += <operand>` using the tightest opcode the operand allows:
    /// a plain local or a numeric literal needs no scratch register at all.
    fn emit_local_add_operand(
        &mut self,
        slot: u32,
        value: &Expr,
        _tmp: u32,
    ) -> Result<bool, String> {
        match value {
            Expr::Num(n) => {
                let k = self.const_num(*n);
                if k <= 0x00FF_FFFF {
                    let _ = k;
                }
                self.emit(Op::LocalAddK, slot, k, 0);
                return Ok(true);
            }
            Expr::Ident(name) => {
                if let Some(b) = self.find_binding(name) {
                    if !b.captured {
                        self.emit(Op::LocalAdd, slot, b.slot, 0);
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
            _ => Ok(false),
        }
    }

    /// `x += <number literal>` (or `x++`) on an uncaptured local, folded
    /// together with the loop's back-jump. Returns `false` for anything else so
    /// the caller keeps the generic update + `Jump` sequence.
    fn try_fused_update_jump(&mut self, u: &Expr, top: u32) -> Result<bool, String> {
        let (slot, k) = match u {
            Expr::Assign(op, target, value) if *op == crate::ast::AssignOp::AddAssign => {
                let name = match target.as_ref() {
                    Target::Simple(n) => n,
                    _ => return Ok(false),
                };
                let k = match value.as_ref() {
                    Expr::Num(n) => *n,
                    _ => return Ok(false),
                };
                match self.find_binding(name) {
                    Some(b) if !b.captured => {
                        if self.is_accum(b.slot) || self.is_accum_ok(b.slot) {
                            return Ok(false);
                        }
                        (b.slot, k)
                    }
                    _ => return Ok(false),
                }
            }
            Expr::Update(_, crate::ast::UpdOp::Inc, inner) => {
                let name = match inner.as_ref() {
                    Expr::Ident(n) => n,
                    _ => return Ok(false),
                };
                match self.find_binding(name) {
                    Some(b) if !b.captured => {
                        if self.is_accum(b.slot) || self.is_accum_ok(b.slot) {
                            return Ok(false);
                        }
                        (b.slot, 1.0f64)
                    }
                    _ => return Ok(false),
                }
            }
            _ => return Ok(false),
        };
        let kidx = self.const_num(k);
        if kidx > 0x00FF_FFFF {
            return Ok(false);
        }
        self.emit(Op::LocalAddKJ, slot, kidx, top);
        Ok(true)
    }

    /// Statement position `x += y` or `i++` where the result is thrown away.
    ///
    /// Compiling these through `expr` loads the value back out of the local so
    /// an expression consumer can read it — but a bare expression statement
    /// has no consumer, so that load only ever writes a dead register. Inside
    /// a loop that is one wasted instruction per iteration, twice.
    ///
    /// Returns `false` for anything not exactly one of these shapes, so the
    /// caller falls back to the normal path; the accumulator bookkeeping is
    /// deliberately not duplicated here (slots that ever hold a string fall
    /// back to `expr`, which routes through `load_name` and the buffer).
    fn try_discard_update(&mut self, e: &Expr) -> Result<bool, String> {
        match e {
            Expr::Assign(op, target, value) if *op == crate::ast::AssignOp::AddAssign => {
                let name = match target.as_ref() {
                    Target::Simple(n) => n,
                    _ => return Ok(false),
                };
                let slot = match self.find_binding(name) {
                    Some(b) if !b.captured => b.slot,
                    _ => return Ok(false),
                };
                if self.is_accum(slot) || self.is_accum_ok(slot) {
                    return Ok(false);
                }
                // `sum += i` and `x += 1` need no scratch register: the source
                // is either a local already or a constant in the pool.
                let scratch = self.alloc_temp();
                if self.emit_local_add_operand(slot, value, scratch)? {
                    self.free_temps_to(scratch);
                    return Ok(true);
                }
                self.expr(value, scratch)?;
                self.emit(Op::LocalAdd, slot, scratch, 0);
                self.free_temps_to(scratch);
                Ok(true)
            }
            Expr::Update(_, crate::ast::UpdOp::Inc, inner) => {
                let name = match inner.as_ref() {
                    Expr::Ident(n) => n,
                    _ => return Ok(false),
                };
                let slot = match self.find_binding(name) {
                    Some(b) if !b.captured => b.slot,
                    _ => return Ok(false),
                };
                if self.is_accum(slot) || self.is_accum_ok(slot) {
                    return Ok(false);
                }
                let k = self.const_num(1.0);
                self.emit(Op::LocalAddK, slot, k, 0);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Fuse `local < K` (and `>`, `<=`, `>=`) into one conditional branch.
    ///
    /// Loop tests compiled to four instructions — load the local, load the
    /// constant, compare, branch — where only the branch is observable. This
    /// shape-matches the test and emits `JumpLocalCmp` directly, returning the
    /// index of the jump so the caller can patch its target. Anything that is
    /// not exactly an uncaptured local compared with a numeric literal (side
    /// effects, captured variables, accumulators, reversed operands) returns
    /// `None` and the caller uses the generic path unchanged.
    fn try_fused_test(&mut self, cond: &Expr) -> Option<usize> {
        let (op, lhs, rhs) = match cond {
            Expr::Binary(op, l, r) => (*op, l, r),
            _ => return None,
        };
        let code = match op {
            BinOp::Lt => 0u32,
            BinOp::Gt => 1,
            BinOp::Le => 2,
            BinOp::Ge => 3,
            _ => return None,
        };
        let name = match lhs.as_ref() {
            Expr::Ident(n) => n,
            _ => return None,
        };
        let slot = match self.find_binding(name) {
            Some(b) if !b.captured => b.slot,
            _ => return None,
        };
        if self.is_accum(slot) || self.is_accum_ok(slot) {
            return None;
        }
        let kidx = match rhs.as_ref() {
            Expr::Num(n) => self.const_num(*n),
            _ => return None,
        };
        if kidx > 0x00FF_FFFF {
            return None;
        }
        Some(self.emit(Op::JumpLocalCmp, slot, (code << 24) | kidx, 0))
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), String> {
        match s {
            Stmt::Raw(_) => {
                // Module declarations (`export`/`import`) are not executed.
            }
            Stmt::Comment { .. } => {
                // Comments exist only for the formatter.
            }
            Stmt::Empty => {}
            Stmt::Expr(e) => {
                // Not the completion statement (that goes through
                // `completion_value`), so the value is discarded.
                let t = self.alloc_temp();
                if !self.try_discard_update(e)? {
                    self.expr(e, t)?;
                }
                self.free_temps_to(t);
            }
            Stmt::Var(kind, decls) => {
                let fs = self.states.len() - 1;
                for (t, init) in decls {
                    let lexical = matches!(kind, VarKind::Let | VarKind::Const);
                    let simple = match t {
                        Target::Simple(n) => Some(n.clone()),
                        _ => None,
                    };
                    // The initializer is compiled *inside* `declare`, so the name must
                    // be in the dead zone for its duration: `let x = x` reads x before
                    // any value exists.
                    if lexical {
                        if let Some(n) = &simple {
                            if let Some((_, set)) = self
                                .pending_lexical
                                .iter_mut()
                                .rev()
                                .find(|(owner, _)| *owner == fs)
                            {
                                set.insert(n.clone());
                            }
                        }
                    }
                    let declared = self.declare(*kind, t, init);
                    // Reaching the end of the declaration ends the dead zone.
                    if lexical {
                        if let Some(n) = &simple {
                            if let Some((_, set)) = self
                                .pending_lexical
                                .iter_mut()
                                .rev()
                                .find(|(owner, _)| *owner == fs)
                            {
                                set.remove(n.as_str());
                            }
                        }
                    }
                    declared?;
                }
            }
            Stmt::Block(body) => {
                self.push_scope();
                // `var` belongs to the function scope and was hoisted at entry.
                // Rebinding it here would hide the initialized slot after this block.
                self.hoist_functions(body)?;
                // Names this block declares with `let`/`const` are in their temporal
                // dead zone until their declaration is reached. Pre-scanning them lets
                // a read before that point throw instead of silently reading a global
                // — the statically decidable part of the TDZ.
                let mut pending = std::collections::HashSet::new();
                for st in body {
                    if let Stmt::Var(kind, decls) = st {
                        if matches!(kind, VarKind::Let | VarKind::Const) {
                            for (t, _) in decls {
                                if let Target::Simple(n) = t {
                                    pending.insert(n.clone());
                                }
                            }
                        }
                    }
                }
                let fs = self.states.len() - 1;
                self.pending_lexical.push((fs, pending));
                for s in body {
                    self.stmt(s)?;
                }
                self.pending_lexical.pop();
                self.pop_scope();
            }
            Stmt::If(c, a, b) => {
                let t = self.alloc_temp();
                self.expr(c, t)?;
                let else_ip = self.emit(Op::JumpIfFalse, t, 0, 0);
                self.free_temps_to(t);
                self.push_scope();
                self.stmt(a)?;
                self.pop_scope();
                match b {
                    Some(b) => {
                        let end_ip = self.emit(Op::Jump, 0, 0, 0);
                        // The else target is the instruction after that jump, and
                        // it must be patched exactly once.
                        let mid = self.here();
                        self.patch_to(else_ip, mid);
                        self.push_scope();
                        self.stmt(b)?;
                        self.pop_scope();
                        let end = self.here();
                        self.patch_to(end_ip, end);
                    }
                    None => {
                        let end = self.here();
                        self.patch_to(else_ip, end);
                    }
                }
            }
            Stmt::With(..) => {
                return Err(
                    "SyntaxError: `with` is not supported: JholVM has no dynamic scope".into(),
                )
            }
            Stmt::While(c, body) => {
                let lbl = self.pending_label.take();
                self.push_label(lbl);
                let top = self.here();
                let exit = match self.try_fused_test(c) {
                    Some(j) => j,
                    None => {
                        let t = self.alloc_temp();
                        self.expr(c, t)?;
                        let e = self.emit(Op::JumpIfFalse, t, 0, 0);
                        self.free_temps_to(t);
                        e
                    }
                };
                self.push_scope();
                self.stmt(body)?;
                self.pop_scope();
                let cont = self.here();
                self.emit(Op::Jump, 0, 0, top as u32);
                let end = self.here();
                self.patch_to(exit, end);
                self.cur().labels.last_mut().unwrap().cont_ip = cont;
                self.close_label(end, cont);
            }
            Stmt::DoWhile(body, c) => {
                let lbl = self.pending_label.take();
                self.push_label(lbl);
                let top = self.here();
                self.push_scope();
                self.stmt(body)?;
                self.pop_scope();
                let cont = self.here();
                let t = self.alloc_temp();
                self.expr(c, t)?;
                self.emit(Op::JumpIfTrue, t, 0, top as u32);
                self.free_temps_to(t);
                let end = self.here();
                self.close_label(end, cont);
            }
            Stmt::For(init, test, update, body) => {
                self.push_scope();
                if let Some(i) = init {
                    self.stmt(i)?;
                }
                // Now the loop variable exists: mark it captured so the *test*
                // also goes through the cell.
                let rebound = self.captured_loop_slots(init.as_deref(), body);
                // Two boxed-loop designs, by declaration kind:
                //
                // * `var` (node semantics): ONE binding shared by every
                //   iteration and every closure. Box it ONCE here — CreateCell
                //   reads the plain slot the init just wrote and replaces it
                //   with a cell holding that value — then the captured binding
                //   routes every test/body/update access through the cell, and
                //   all closures capture the same cell. No shadow, no rebind:
                //   a per-iteration cell would be `let` semantics.
                // * `let`/`const` (block semantics): a fresh binding per
                //   iteration. Shadow register + `RebindCell` at the head gives
                //   each closure its own cell, which is the node behaviour.
                //
                // The boxed value is created AFTER the init has written the
                // plain slot (the earlier "cell before any plain-slot access"
                // panics came from boxing an unwritten slot).
                let var_init = matches!(
                    init.as_deref(),
                    Some(crate::ast::Stmt::Var(crate::ast::VarKind::Var, _))
                );
                let shadow: Vec<(String, u32, u32)> = if var_init {
                    for (_, slot) in rebound.iter() {
                        self.emit(Op::CreateCell, *slot, 0, 0);
                    }
                    Vec::new()
                } else {
                    rebound
                        .iter()
                        .map(|(name, slot)| {
                            let u = self.add_local("\u{0}loopval");
                            self.emit(Op::Move, u, *slot, 0);
                            (name.clone(), *slot, u)
                        })
                        .collect()
                };
                let lbl = self.pending_label.take();
                self.push_label(lbl);
                // The rebind is the loop head: jumping back to `top` gives every
                // iteration a fresh binding built from the shadow register.
                let top = self.here();
                for (_, slot, u) in shadow.iter() {
                    self.emit(Op::RebindCell, *slot, *u, 0);
                }
                let exit = match test {
                    Some(c) => match self.try_fused_test(c) {
                        Some(j) => Some(j),
                        None => {
                            let t = self.alloc_temp();
                            self.expr(c, t)?;
                            let e = self.emit(Op::JumpIfFalse, t, 0, 0);
                            self.free_temps_to(t);
                            Some(e)
                        }
                    },
                    None => None,
                };
                self.push_scope();
                self.stmt(body)?;
                self.pop_scope();
                let cont = self.here();
                let mut fused_done = false;
                if let Some(u) = update {
                    // Shadow each loop variable with its plain register, so the
                    // update expression writes the value the next rebind boxes
                    // instead of mutating the cell a closure already captured.
                    self.push_scope();
                    for (name, _slot, sr) in shadow.iter() {
                        let n = self.states.len() - 1;
                        let binding = Binding {
                            slot: *sr,
                            captured: false,
                        };
                        self.states[n]
                            .scopes
                            .last_mut()
                            .unwrap()
                            .names
                            .push((name.clone(), binding));
                    }
                    // Fold `x += 1; jump top` into one instruction when the
                    // update is a plain local increment — which is exactly the
                    // counted loop. It has to be tried BEFORE the update is
                    // compiled normally, or the increment gets emitted twice.
                    let mut fused = self.try_fused_update_jump(u, top as u32)?;
                    if !fused {
                        // The update's value feeds nobody — the loop only wants
                        // the side effect — so `i++` here must not load a result.
                        fused = false;
                        if !self.try_discard_update(u)? {
                            let t = self.alloc_temp();
                            self.expr(u, t)?;
                            self.free_temps_to(t);
                        }
                    }
                    self.pop_scope();
                    if !fused {
                        self.emit(Op::Jump, 0, 0, top as u32);
                    }
                    fused_done = true;
                }
                if !fused_done {
                    self.emit(Op::Jump, 0, 0, top as u32);
                }
                let end = self.here();
                if let Some(e) = exit {
                    self.patch_to(e, end);
                }
                self.close_label(end, cont);
                self.pop_scope();
            }
            Stmt::ForInOf {
                kind,
                decl,
                expr,
                body,
            } => self.for_in_of(*kind, decl.as_ref(), expr, body)?,
            Stmt::Return(e) => match e {
                Some(e) => {
                    let t = self.alloc_temp();
                    self.expr(e, t)?;
                    self.emit_return_with_handlers(t);
                    self.free_temps_to(t);
                }
                None => {
                    self.emit_return_with_handlers(THIS_SLOT);
                }
            },
            Stmt::Break(l) => {
                let ip = self.emit(Op::Jump, 0, 0, 0);
                let idx = self.find_label(l.as_deref());
                self.cur().labels[idx].breaks.push(ip);
            }
            Stmt::Continue(l) => {
                let ip = self.emit(Op::Jump, 0, 0, 0);
                let idx = self.find_label(l.as_deref());
                self.cur().labels[idx].continues.push(ip);
            }
            Stmt::Throw(e) => {
                let t = self.alloc_temp();
                self.expr(e, t)?;
                self.emit(Op::Throw, t, 0, 0);
                self.free_temps_to(t);
            }
            Stmt::Try(body, catch, finally) => {
                self.try_stmt(
                    std::slice::from_ref(body.as_ref()),
                    catch.as_ref(),
                    finally.as_deref(),
                )?;
            }
            Stmt::Func(_) => {}
            Stmt::Class(c) => {
                // Bind the name *before* the compile: the class body (and a
                // following `extends`) resolves it by local slot, and the slot
                // has to exist before the body's own temporaries are handed out.
                if let Some(n) = &c.name {
                    if self.find_binding(n).is_none() {
                        self.add_local(n);
                    }
                }
                let t = self.alloc_temp();
                self.class_def(c, t)?;
                if let Some(n) = &c.name {
                    self.bind(&Target::Simple(n.clone()), t)?;
                    // Also publish it globally so `extends` can find it.
                    let gk = self.const_str(n);
                    self.emit(Op::DefGlobal, gk, t, 0);
                }
                self.free_temps_to(t);
            }
            Stmt::Labeled(name, inner) => {
                // `for (…) { continue name }` must reach the loop's own continue
                // point, so the label is handed to the loop rather than becoming
                // a label of its own (which would patch it to the loop's end).
                self.pending_label = Some(name.clone());
                self.stmt(inner)?;
                self.pending_label = None;
                if self.cur().labels.is_empty() {
                    let end = self.here();
                    self.close_label(end, end);
                }
            }
            Stmt::Switch(disc, cases) => self.switch_stmt(disc, cases)?,
        }
        Ok(())
    }

    /// `return` must unwind the runtime handler stack, so pop every handler still
    /// live in this function first.
    fn emit_return_with_handlers(&mut self, src: u32) {
        // Abrupt completion: every enclosing `finally` runs before the return,
        // innermost first. Without this, `try { return x } finally { … }`
        // skipped the finally block entirely.
        if self.in_finally == 0 && !self.finally_stack.is_empty() {
            let pending: Vec<Vec<Stmt>> = self.finally_stack.clone();
            for block in pending.iter().rev() {
                self.in_finally += 1;
                self.push_scope();
                for st in block {
                    if let Err(_e) = self.stmt(st) {
                        // Finally bodies were already compiled at their site; a
                        // duplicate emission failing here would be an internal
                        // error, and swallowing it is better than emitting a
                        // return without its finally.
                        break;
                    }
                }
                self.pop_scope();
                self.in_finally -= 1;
            }
        }
        // Pop only the handlers that are still live *right here*; a try body's
        // `return` has already popped its own.
        let live = self.cur().handler_stack.len();
        for _ in 0..live {
            self.emit(Op::PopTry, 0, 0, 0);
        }
        self.emit(Op::Return, src, 0, 0);
    }

    /// Slots of `let`/`const` names declared in a `for` initialiser. These get a
    /// fresh cell per iteration, which is what makes `let` in a loop capture
    /// per-iteration values instead of one shared final value.
    fn loop_cell_slots(&mut self, init: Option<&Stmt>) -> Vec<(String, u32)> {
        let mut out = Vec::new();
        if let Some(Stmt::Var(kind, decls)) = init {
            // `var` loops are the node-semantics case this whole mechanism
            // exists for: a closure capturing a `var` shares ONE binding, and
            // the binding must be boxed before the loop head so the fused
            // test never compares a cell pointer as a number. Excluding `var`
            // here meant the pre-pass returned empty for `var` loops, the
            // shadow/rebind machinery never engaged, and the body's own
            // cellification then desynchronised from the raw slot (the loop
            // exited after one iteration, or closures saw garbage).
            let _ = kind;
            for (target, _) in decls {
                if let Target::Simple(n) = target {
                    if let Some(b) = self.find_binding(n) {
                        out.push((n.clone(), b.slot));
                    }
                }
            }
        }
        out
    }

    /// Loop variables that a *nested function* actually references.
    ///
    /// These are the only ones that need a fresh cell per iteration. Rebinding
    /// unconditionally allocated a `JCell` on every pass, which made a plain
    /// 50M-iteration counting loop spend its whole life in the allocator.
    /// Everything else stays a plain local on the fast path.
    fn captured_loop_slots(&mut self, init: Option<&Stmt>, body: &Stmt) -> Vec<(String, u32)> {
        let names = self.loop_cell_slots(init);
        if names.is_empty() {
            return names;
        }
        let mut out = Vec::new();
        for (n, slot) in names {
            if !stmt_captures(&n, body) {
                continue;
            }
            if let Some(b) = self.find_binding(&n) {
                if !b.captured {
                    self.mark_captured(&n);
                }
            }
            out.push((n, slot));
        }
        out
    }

    /// Flip the matching binding in the enclosing scope to `captured`, so every
    /// later access goes through the cell.
    fn mark_captured(&mut self, name: &str) {
        let n = self.states.len() - 1;
        for sc in self.states[n].scopes.iter_mut().rev() {
            for (nm, b) in sc.names.iter_mut().rev() {
                if nm == name {
                    b.captured = true;
                    return;
                }
            }
        }
    }

    fn push_label(&mut self, name: Option<String>) {
        self.cur().labels.push(LabelCtx {
            name,
            breaks: Vec::new(),
            continues: Vec::new(),
            cont_ip: 0,
        });
    }

    fn find_label(&mut self, name: Option<&str>) -> usize {
        let n = self.states.len() - 1;
        let len = self.states[n].labels.len();
        let last = len - 1;
        match name {
            None => last,
            Some(want) => {
                for i in (0..len).rev() {
                    if self.states[n].labels[i].name.as_deref() == Some(want) {
                        return i;
                    }
                }
                last
            }
        }
    }

    fn close_label(&mut self, _end: usize, cont: usize) {
        // A labelled non-loop statement has already consumed the label, so an
        // empty stack here simply means "nothing to patch".
        let Some(l) = self.cur().labels.pop() else {
            return;
        };
        let end = self.here();
        for ip in l.breaks {
            self.patch_to(ip, end);
        }
        for ip in l.continues {
            self.patch_to(ip, cont);
        }
    }

    fn try_stmt(
        &mut self,
        body: &[Stmt],
        catch: Option<&(Target, Box<Stmt>)>,
        finally: Option<&Stmt>,
    ) -> Result<(), String> {
        let fin_stmts: Vec<Stmt> = finally
            .map(|f| match f {
                Stmt::Block(v) => v.clone(),
                other => vec![other.clone()],
            })
            .unwrap_or_default();
        if !fin_stmts.is_empty() {
            self.finally_stack.push(fin_stmts);
        }
        let run_finally = |c: &mut Compiler| -> Result<(), String> {
            if let Some(f) = finally {
                c.in_finally += 1;
                c.push_scope();
                let r = c.stmt(f);
                c.pop_scope();
                c.in_finally -= 1;
                r?;
            }
            Ok(())
        };

        let exn = self.alloc_temp();
        self.free_temps_to(exn);
        let catch_ip = self.emit(Op::PushTry, 0, exn, 0);
        self.cur().handler_stack.push(catch_ip as u32);
        self.push_scope();
        for s in body {
            self.stmt(s)?;
        }
        self.pop_scope();
        self.emit(Op::PopTry, 0, 0, 0);
        self.cur().handler_stack.pop();
        run_finally(self)?;
        let end_ip = self.emit(Op::Jump, 0, 0, 0);

        let handler = self.here();
        self.patch_to(catch_ip, handler);
        // On the exception path the catch clause runs first, then the finally
        // block, matching JS evaluation order.
        match catch {
            None => {
                // No catch: `finally` still runs, then the exception is
                // rethrown. Emitting Throw first made the finally block dead
                // code, so `try { throw x } finally { … }` never ran it.
                // Reserve one temp so the finally body cannot reuse `exn`.
                let _reserved = self.alloc_temp();
                run_finally(self)?;
                self.emit(Op::Throw, exn, 0, 0);
            }
            Some((param, cbody)) => {
                self.push_scope();
                // The catch parameter is a fresh local in the handler's scope.
                self.declare_pattern(VarKind::Let, param);
                self.bind(param, exn)?;
                self.stmt(cbody)?;
                self.pop_scope();
                run_finally(self)?;
            }
        }
        if !self.finally_stack.is_empty() {
            self.finally_stack.pop();
        }
        let end = self.here();
        self.patch_to(end_ip, end);
        Ok(())
    }

    fn switch_stmt(
        &mut self,
        disc: &Expr,
        cases: &[(Option<Expr>, Vec<Stmt>)],
    ) -> Result<(), String> {
        let d = self.alloc_temp();
        self.expr(disc, d)?;
        self.push_label(None);

        // Phase 1 — every *comparison*, first. A matching case then enters its
        // body directly; later cases fall into it rather than being re-tested.
        let mut entry: Vec<usize> = Vec::new();
        for (test, _) in cases.iter() {
            let test = match test {
                Some(t) => t,
                None => continue,
            };
            let tt = self.alloc_temp();
            self.expr(test, tt)?;
            self.emit(Op::StrictEq, tt, d, tt);
            let skip = self.emit(Op::JumpIfFalse, tt, 0, 0);
            self.free_temps_to(tt);
            entry.push(self.emit(Op::Jump, 0, 0, 0));
            let next = self.here();
            self.patch_to(skip, next);
        }
        // Nothing matched: the default case, or the end of the switch.
        let no_match = self.emit(Op::Jump, 0, 0, 0);

        // Phase 2 — bodies in source order, each falling through to the next.
        let mut body_starts: Vec<usize> = Vec::with_capacity(cases.len());
        for (_, body) in cases.iter() {
            body_starts.push(self.here());
            self.push_scope();
            for st in body {
                self.stmt(st)?;
            }
            self.pop_scope();
        }
        let end = self.here();
        let default_idx = cases.iter().position(|(t, _)| t.is_none());
        let mut k = 0usize;
        for (idx, (test, _)) in cases.iter().enumerate() {
            if test.is_some() {
                self.patch_to(entry[k], body_starts[idx]);
                k += 1;
            }
        }
        match default_idx {
            Some(di) => self.patch_to(no_match, body_starts[di]),
            None => self.patch_to(no_match, end),
        }
        self.free_temps_to(d);
        self.close_label(end, end);
        Ok(())
    }

    /// `yield* expr`: delegate to another iterator, forwarding each
    /// `next(sent)` value. The expression value is the delegate's final
    /// `.value` once it reports `done`.
    fn compile_yield_star(&mut self, inner: &Expr, dst: u32) -> Result<(), String> {
        let obj = self.alloc_temp();
        self.expr(inner, obj)?;
        let iter = self.emit_get_iterator(obj)?;
        let sent = self.alloc_temp();
        self.emit(Op::LoadUndefined, sent, 0, 0);
        let top = self.here();
        self.push_label(None);
        // `CallThis` layout: `[this, callee, args...]`.
        let base = self.alloc_temp();
        let callee = self.alloc_temp();
        let arg = self.alloc_temp();
        let res = self.alloc_temp();
        self.emit(Op::Move, base, iter, 0);
        let next_key = self.key_of("next");
        let nk = self.const_str("next");
        let nip = self.emit(Op::GetProp, callee, iter, nk);
        self.set_ic_key(nip, next_key);
        self.emit(Op::Move, arg, sent, 0);
        self.emit(Op::CallThis, res, base, 1);
        let done = self.alloc_temp();
        let done_key = self.key_of("done");
        let dk = self.const_str("done");
        let dip = self.emit(Op::GetProp, done, res, dk);
        self.set_ic_key(dip, done_key);
        let exit = self.emit(Op::JumpIfTrue, done, 0, 0);
        let val = self.alloc_temp();
        let value_key = self.key_of("value");
        let vk = self.const_str("value");
        let vip = self.emit(Op::GetProp, val, res, vk);
        self.set_ic_key(vip, value_key);
        // After resume the VM puts the caller's sent value in `val`'s slot.
        self.emit(Op::Yield, val, 0, 0);
        self.emit(Op::Move, sent, val, 0);
        self.emit(Op::Jump, 0, 0, top as u32);
        let end = self.here();
        self.patch_to(exit, end);
        self.close_label(end, top);
        let out = self.alloc_temp();
        let outk = self.const_str("value");
        let oip = self.emit(Op::GetProp, out, res, outk);
        self.set_ic_key(oip, value_key);
        self.emit(Op::Move, dst, out, 0);
        self.free_temps_to(obj);
        Ok(())
    }

    /// `obj[Symbol.iterator]()` or the legacy `values()` fallback. Leaves
    /// `iter` as the top live temp.
    fn emit_get_iterator(&mut self, obj: u32) -> Result<u32, String> {
        let it_fn = self.alloc_temp();
        let iter_key = self.key_of(ITER_KEY);
        let ik = self.const_str(ITER_KEY);
        let ip = self.emit(Op::GetProp, it_fn, obj, ik);
        self.set_ic_key(ip, iter_key);
        let undef = self.alloc_temp();
        self.emit(Op::LoadUndefined, undef, 0, 0);
        let _eq = self.emit(Op::StrictEq, undef, it_fn, undef);
        let skip = self.emit(Op::JumpIfFalse, undef, 0, 0);
        let vk = self.const_str(VALUES);
        self.emit(Op::GetGlobal, it_fn, vk, 0);
        let joined = self.here();
        // Only the conditional jump is a branch. Patching the `StrictEq`
        // above would overwrite one of its register operands with this
        // jump target, and the VM would then read a bogus register.
        self.patch_to(skip, joined);

        let base = self.alloc_temp();
        let callee = self.alloc_temp();
        let iter = self.alloc_temp();
        self.emit(Op::Move, base, obj, 0);
        self.emit(Op::Move, callee, it_fn, 0);
        self.emit(Op::CallThis, iter, base, 0);
        self.free_temps_to(iter + 1);
        Ok(iter)
    }

    fn for_in_of(
        &mut self,
        kind: ForKind,
        decl: Option<&(Option<VarKind>, Target)>,
        expr: &Expr,
        body: &Stmt,
    ) -> Result<(), String> {
        let pending = self.pending_label.take();
        match kind {
            // The engine has no async/await: fail loudly rather than running
            // the loop as if it were a plain for-of (silent wrong behavior).
            ForKind::AwaitOf => {
                return Err(
                    "SyntaxError: `for await (… of …)` is not supported: JholVM has no async/await"
                        .into(),
                );
            }
            ForKind::Of => {
                if let Some(n) = pending {
                    self.push_label(Some(n));
                }
                let obj = self.alloc_temp();
                self.expr(expr, obj)?;
                let iter = self.emit_get_iterator(obj)?;

                self.push_label(None);
                let top = self.here();
                let nb = self.alloc_temp(); // this
                let nc = self.alloc_temp(); // callee
                let res = self.alloc_temp();
                self.emit(Op::Move, nb, iter, 0);
                self.emit(Op::Move, nc, iter, 0);
                let next_key = self.key_of("next");
                let nk = self.const_str("next");
                let nip = self.emit(Op::GetProp, nc, iter, nk);
                self.set_ic_key(nip, next_key);
                self.emit(Op::CallThis, res, nb, 0);
                let done = self.alloc_temp();
                let done_key = self.key_of("done");
                let dk = self.const_str("done");
                let dip = self.emit(Op::GetProp, done, res, dk);
                self.set_ic_key(dip, done_key);
                let exit = self.emit(Op::JumpIfTrue, done, 0, 0);
                let val = self.alloc_temp();
                let value_key = self.key_of("value");
                let vk2 = self.const_str("value");
                let vip = self.emit(Op::GetProp, val, res, vk2);
                self.set_ic_key(vip, value_key);
                // Keep the per-iteration registers (and the live iterator below
                // them) above the loop body's temporaries.
                self.free_temps_to(val + 1);

                self.push_scope();
                self.bind_loop_var(decl, val)?;
                self.stmt(body)?;
                self.pop_scope();
                self.emit(Op::Jump, 0, 0, top as u32);
                let end = self.here();
                self.patch_to(exit, end);
                self.close_label(end, top);
                self.free_temps_to(iter + 1);
            }
            ForKind::In => {
                // Reserve the loop variable's slot BEFORE the working temps.
                // `add_local` hands out `locals_top`, which sits *below* the
                // temps — allocating it after the temps would make the loop
                // variable's slot overlap with one of them, corrupting the
                // loop state on the first `SetLocal`.
                self.push_scope();
                let mut loop_var_slot: Option<u32> = None;
                if let Some((Some(kind), Target::Simple(name))) = decl {
                    loop_var_slot = Some(if *kind == VarKind::Var {
                        self.find_binding(name)
                            .map(|b| b.slot)
                            .unwrap_or_else(|| self.add_local(name))
                    } else {
                        self.add_local(name)
                    });
                }
                let obj = self.alloc_temp();
                self.expr(expr, obj)?;
                let g = self.alloc_temp();
                let ek = self.const_str(ENUM_KEYS);
                self.emit(Op::GetGlobal, g, ek, 0);
                let callee = self.alloc_temp();
                let arg = self.alloc_temp();
                let keys = self.alloc_temp();
                self.emit(Op::Move, callee, g, 0);
                self.emit(Op::Move, arg, obj, 0);
                self.emit(Op::Call, keys, callee, 1);
                self.free_temps_to(keys + 1);

                self.push_label(None);
                let idx = self.alloc_temp();
                // Start at 0: `undefined < len` is false, which exited the
                // loop before its first iteration.
                let zero = self.const_num(0.0);
                self.emit(Op::LoadK, idx, zero, 0);
                let one = self.alloc_temp();
                let one_k = self.const_num(1.0);
                self.emit(Op::LoadK, one, one_k, 0);
                let top = self.here();
                let len = self.alloc_temp();
                let length_key = self.key_of("length");
                let lk = self.const_str("length");
                let lip = self.emit(Op::GetProp, len, keys, lk);
                self.set_ic_key(lip, length_key);
                let cmp = self.alloc_temp();
                self.emit(Op::Lt, cmp, idx, len);
                let exit = self.emit(Op::JumpIfFalse, cmp, 0, 0);
                let k = self.alloc_temp();
                self.emit(Op::GetIndex, k, keys, idx);
                self.free_temps_to(k + 1);
                // The loop variable's slot was reserved above; store the key.
                if let Some(slot) = loop_var_slot {
                    self.emit(Op::SetLocal, slot, k, 0);
                } else if let Some((_, target)) = decl {
                    self.bind(target, k)?;
                }
                self.stmt(body)?;
                self.pop_scope();
                let cont = self.here();
                self.emit(Op::Add, idx, idx, one);
                self.emit(Op::Jump, 0, 0, top as u32);
                let end = self.here();
                self.patch_to(exit, end);
                self.close_label(end, cont);
                self.free_temps_to(keys + 1);
            }
        }
        Ok(())
    }

    /// Reserve the slot a simple loop variable will bind to, without emitting
    /// a store. Used when the slot has to exist before other registers.
    #[allow(dead_code)]
    fn bind_loop_slot(&mut self, decl: Option<&(VarKind, Target)>) -> u32 {
        match decl {
            Some((_, Target::Simple(name))) => self.add_local(name),
            _ => 0,
        }
    }

    fn bind_loop_var(
        &mut self,
        decl: Option<&(Option<VarKind>, Target)>,
        src: u32,
    ) -> Result<(), String> {
        match decl {
            None => Ok(()),
            // Bare `for (x of e)`: assign through normal target resolution so
            // an existing binding is updated instead of shadowed.
            Some((None, target)) => self.bind(target, src),
            Some((Some(VarKind::Var), Target::Simple(name))) => {
                let slot = self
                    .find_binding(name)
                    .map(|b| b.slot)
                    .unwrap_or_else(|| self.add_local(name));
                self.emit(Op::SetLocal, slot, src, 0);
                Ok(())
            }
            Some((Some(_), Target::Simple(name))) => {
                let slot = self.add_local(name);
                self.emit(Op::SetLocal, slot, src, 0);
                Ok(())
            }
            Some((_, target)) => self.bind(target, src),
        }
    }

    fn hoist_functions(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        // Lexical names must exist before compiling hoisted closures, including
        // references to declarations initialized later in this block.
        for st in stmts {
            if let Stmt::Var(kind, decls) = st {
                if *kind != VarKind::Var {
                    for (target, _) in decls {
                        self.declare_pattern(*kind, target);
                    }
                }
            }
        }
        // Bindings first, so a function can be called before its declaration line.
        for st in stmts {
            if let Stmt::Func(f) = st {
                if let Some(name) = f.name.clone() {
                    self.add_local(&name);
                }
            }
        }
        for st in stmts {
            if let Stmt::Func(f) = st {
                self.emit_function_binding(f)?;
            }
        }
        Ok(())
    }

    fn emit_function_binding(&mut self, f: &Func) -> Result<(), String> {
        let name = match &f.name {
            Some(n) => n.clone(),
            None => return Ok(()),
        };
        let dst = self.alloc_temp();
        self.cellify_pending();
        let pidx = self.compile_fn(f, FnCtx::Func, Some(name.clone()))?;
        let k = self.const_fn(pidx);
        self.emit(Op::NewClosure, dst, k, 0);
        let slot = self.find_binding(&name).map(|b| b.slot).unwrap_or(u32::MAX);
        let captured = self
            .find_binding(&name)
            .map(|b| b.captured)
            .unwrap_or(false);
        if slot != u32::MAX {
            // If the body referenced this declaration, the slot already holds a
            // `JCell` (created when the closure captured it), so the binding has
            // to be written through the cell or the closure would see `undefined`.
            if captured {
                self.emit(Op::CellSet, slot, dst, 0);
            } else {
                self.emit(Op::SetLocal, slot, dst, 0);
            }
        } else {
            let kk = self.const_str(&name);
            self.emit(Op::DefGlobal, kk, dst, 0);
        }
        self.free_temps_to(dst);
        Ok(())
    }

    fn collect_vars(&self, stmts: &[Stmt], out: &mut Vec<String>) {
        for s in stmts {
            match s {
                Stmt::Var(k, decls) => {
                    if *k == VarKind::Var {
                        for (t, _) in decls {
                            collect_pattern_names(t, out);
                        }
                    }
                }
                Stmt::Block(b) => self.collect_vars(b, out),
                Stmt::If(_, a, b) => {
                    self.collect_vars(std::slice::from_ref(a.as_ref()), out);
                    if let Some(b) = b {
                        self.collect_vars(std::slice::from_ref(b.as_ref()), out);
                    }
                }
                Stmt::While(_, b) | Stmt::With(_, b) => {
                    self.collect_vars(std::slice::from_ref(b.as_ref()), out)
                }
                Stmt::DoWhile(b, _) => self.collect_vars(std::slice::from_ref(b.as_ref()), out),
                Stmt::For(i, _, _, b) => {
                    if let Some(i) = i {
                        self.collect_vars(std::slice::from_ref(i.as_ref()), out);
                    }
                    self.collect_vars(std::slice::from_ref(b.as_ref()), out);
                }
                Stmt::ForInOf { decl, body, .. } => {
                    if let Some((Some(VarKind::Var), target)) = decl {
                        collect_pattern_names(target, out);
                    }
                    self.collect_vars(std::slice::from_ref(body.as_ref()), out);
                }
                Stmt::Labeled(_, b) => self.collect_vars(std::slice::from_ref(b.as_ref()), out),
                Stmt::Try(b, c, f) => {
                    self.collect_vars(std::slice::from_ref(b.as_ref()), out);
                    if let Some((_, cb)) = c {
                        self.collect_vars(std::slice::from_ref(cb.as_ref()), out);
                    }
                    if let Some(fb) = f {
                        self.collect_vars(std::slice::from_ref(fb.as_ref()), out);
                    }
                }
                Stmt::Switch(_, cases) => {
                    for (_, body) in cases {
                        self.collect_vars(body, out);
                    }
                }
                _ => {}
            }
        }
    }
}

fn collect_pattern_names(t: &Target, out: &mut Vec<String>) {
    match t {
        Target::Simple(n) => {
            if !n.is_empty() {
                out.push(n.clone());
            }
        }
        Target::Array(items) => {
            for i in items.iter().flatten() {
                collect_pattern_names(i, out);
            }
        }
        Target::Object(fs) => {
            for f in fs {
                if let Some(t) = &f.target {
                    collect_pattern_names(t, out);
                }
            }
        }
        Target::WithDefault(t, _) => collect_pattern_names(t, out),
        Target::Rest { target, .. } => collect_pattern_names(target, out),
        Target::Member(_) => {}
    }
}

// ---------------------------------------------------------------------------
// declarations & assignment targets
// ---------------------------------------------------------------------------

impl Compiler {
    /// Reserve a local slot for every name bound by `target`, so a destructuring
    /// declaration binds locals rather than falling through to globals.
    fn declare_pattern(&mut self, kind: VarKind, target: &Target) {
        match target {
            Target::Simple(n) => {
                if (kind == VarKind::Var && self.find_binding(n).is_none())
                    || (kind != VarKind::Var
                        && !self
                            .cur()
                            .scopes
                            .last()
                            .unwrap()
                            .names
                            .iter()
                            .any(|(name, _)| name == n))
                {
                    self.add_local(n);
                }
            }
            Target::Array(items) => {
                for it in items.iter().flatten() {
                    self.declare_pattern(kind, it);
                }
            }
            Target::Object(fields) => {
                for f in fields {
                    match &f.target {
                        Some(t) => self.declare_pattern(kind, t),
                        None => {
                            if let MemberKey::Dot(n) = &f.key {
                                if (kind == VarKind::Var && self.find_binding(n).is_none())
                                    || (kind != VarKind::Var
                                        && !self
                                            .cur()
                                            .scopes
                                            .last()
                                            .unwrap()
                                            .names
                                            .iter()
                                            .any(|(name, _)| name == n))
                                {
                                    self.add_local(n);
                                }
                            }
                        }
                    }
                }
            }
            Target::WithDefault(t, _) => self.declare_pattern(kind, t),
            Target::Rest { target, .. } => self.declare_pattern(kind, target),
            Target::Member(_) => {}
        }
    }

    fn declare(
        &mut self,
        kind: VarKind,
        target: &Target,
        init: &Option<Expr>,
    ) -> Result<(), String> {
        self.declare_pattern(kind, target);
        // A string-literal initializer makes the local eligible for the
        // `+=` string accumulator. Anything else keeps the generic path, so
        // `let n = 0; n += x` still does numeric addition.
        if let (Target::Simple(n), Some(Expr::Str(_))) = (target, init.as_ref()) {
            if let Some(b) = self.find_binding(n) {
                if !b.captured {
                    self.set_accum_ok(b.slot, true);
                }
            }
        }
        match init {
            Some(e) => {
                let t = self.alloc_temp();
                // NamedEvaluation: only a bare anonymous function/arrow/class
                // initialiser takes the binding's name.
                self.name_hint = match (target, e) {
                    (Target::Simple(n), Expr::Func(f)) | (Target::Simple(n), Expr::Arrow(f))
                        if f.name.is_none() =>
                    {
                        Some(n.clone())
                    }
                    (Target::Simple(n), Expr::Class(c)) if c.name.is_none() => Some(n.clone()),
                    _ => None,
                };
                self.expr(e, t)?;
                self.name_hint = None;
                self.bind(target, t)?;
                self.free_temps_to(t);
            }
            None => {
                if matches!(target, Target::Simple(_)) {
                    return Ok(());
                }
                let _ = &init;
                let t = self.alloc_temp();
                self.emit(Op::LoadUndefined, t, 0, 0);
                self.bind(target, t)?;
                self.free_temps_to(t);
            }
        }
        Ok(())
    }

    /// Compile `s` into `dst` and return true when it produced a completion
    /// value. Expression statements do; so do `try`/`catch` and `if` (the value
    /// of whichever branch ran).
    fn completion_value(&mut self, s: &Stmt, dst: u32) -> Result<bool, String> {
        match s {
            Stmt::Expr(e) => {
                self.expr(e, dst)?;
                Ok(true)
            }
            Stmt::If(c, a, b) => {
                let t = self.alloc_temp();
                self.expr(c, t)?;
                let else_ip = self.emit(Op::JumpIfFalse, t, 0, 0);
                self.free_temps_to(t);
                self.push_scope();
                let ok = self.completion_value(a, dst)?;
                self.pop_scope();
                let end_ip = if let Some(b) = b {
                    let e = self.emit(Op::Jump, 0, 0, 0);
                    let mid = self.here();
                    self.patch_to(else_ip, mid);
                    self.push_scope();
                    let ok = self.completion_value(b, dst)?;
                    self.pop_scope();
                    Some((e, ok))
                } else {
                    None
                };
                let end = self.here();
                self.patch_to(else_ip, end);
                if let Some((e, _)) = end_ip {
                    self.patch_to(e, end);
                }
                Ok(ok)
            }
            Stmt::Try(body, catch, finally) => {
                let fin_stmts: Vec<Stmt> = finally
                    .as_deref()
                    .map(|f| match f {
                        Stmt::Block(v) => v.clone(),
                        other => vec![other.clone()],
                    })
                    .unwrap_or_default();
                if !fin_stmts.is_empty() {
                    self.finally_stack.push(fin_stmts);
                }
                // Compile the try body, then (if it throws) the catch clause,
                // both writing their value into `dst`.
                let exn = self.alloc_temp();
                self.free_temps_to(exn);
                let catch_ip = self.emit(Op::PushTry, 0, exn, 0);
                self.cur().handler_stack.push(catch_ip as u32);
                self.push_scope();
                // `body` is normally a Block; unwrap it so the *last* clause is
                // the one whose value becomes the statement's completion value.
                let body_stmts: &[Stmt] = match body.as_ref() {
                    Stmt::Block(b) => b,
                    other => std::slice::from_ref(other),
                };
                let mut produced = false;
                let n = body_stmts.len();
                for (i, st) in body_stmts.iter().enumerate() {
                    if i + 1 == n {
                        produced = self.completion_value(st, dst)?;
                    } else {
                        self.stmt(st)?;
                    }
                }
                self.pop_scope();
                self.emit(Op::PopTry, 0, 0, 0);
                self.cur().handler_stack.pop();
                // `finally` runs on the normal path too. This arm used to drop
                // it, so a try/finally that was the last statement of its block
                // silently skipped the finally block.
                let run_finally = |c: &mut Compiler| -> Result<(), String> {
                    if let Some(f) = finally {
                        c.in_finally += 1;
                        c.push_scope();
                        let r = c.stmt(f);
                        c.pop_scope();
                        c.in_finally -= 1;
                        r?;
                    }
                    Ok(())
                };
                run_finally(self)?;
                let end_ip = self.emit(Op::Jump, 0, 0, 0);
                let handler = self.here();
                self.patch_to(catch_ip, handler);
                if let Some((param, cbody)) = catch {
                    self.push_scope();
                    self.declare_pattern(VarKind::Let, param);
                    self.bind(param, exn)?;
                    self.completion_value(&cbody, dst)?;
                    self.pop_scope();
                    run_finally(self)?;
                } else {
                    // Rethrow only after `finally`, and keep `exn` alive across
                    // the finally body's own temporaries.
                    let _reserved = self.alloc_temp();
                    run_finally(self)?;
                    self.emit(Op::Throw, exn, 0, 0);
                }
                if !self.finally_stack.is_empty() {
                    self.finally_stack.pop();
                }
                let end = self.here();
                self.patch_to(end_ip, end);
                Ok(produced)
            }
            Stmt::Block(b) => {
                self.push_scope();
                self.hoist_functions(b)?;
                let result = if let Some((last, prefix)) = b.split_last() {
                    for st in prefix {
                        self.stmt(st)?;
                    }
                    self.completion_value(last, dst)?
                } else {
                    false
                };
                self.pop_scope();
                Ok(result)
            }
            _ => {
                self.stmt(s)?;
                Ok(false)
            }
        }
    }

    fn store_local(&mut self, name: &str, src: u32) {
        if let Some(b) = self.find_binding(name) {
            // A plain assignment to the variable ends accumulator mode.
            if !b.captured && self.is_accum(b.slot) {
                self.emit(Op::FlushStr, b.slot, 0, 0);
                self.set_accum(b.slot, false);
            }
            if b.captured {
                self.emit(Op::CellSet, b.slot, src, 0);
            } else {
                self.emit(Op::SetLocal, b.slot, src, 0);
            }
        } else if let Some(u) = self.resolve_upval(self.states.len() - 1, name) {
            self.emit(Op::SetUpval, u, src, 0);
        } else {
            let k = self.const_str(name);
            self.emit(Op::DefGlobal, k, src, 0);
        }
    }

    fn bind(&mut self, target: &Target, src: u32) -> Result<(), String> {
        match target {
            Target::Simple(name) => {
                self.store_local(name, src);
            }
            Target::Member(e) => {
                self.assign_to(e, src)?;
            }
            Target::Array(items) => {
                // Destructuring follows the iterator protocol, so a Set, Map entry
                // list, generator or string binds its elements instead of nothing.
                // `__jhol_iter_to_array` returns a real array unchanged, so the
                // common `const [a, b] = arr` still costs no allocation.
                let src = if items.is_empty() {
                    src
                } else {
                    let converted = self.emit_native("__jhol_iter_to_array", &[src]);
                    let holder = self.alloc_temp();
                    self.emit(Op::Move, holder, converted, 0);
                    holder
                };
                for (i, item) in items.iter().enumerate() {
                    let item = match item {
                        Some(i) => i,
                        None => continue,
                    };
                    // `...rest` has no fixed index: it takes the whole tail.
                    if let Target::Rest { from, target } = item {
                        let from_v = self.alloc_temp();
                        let k = self.const_num(*from as f64);
                        self.emit(Op::LoadK, from_v, k, 0);
                        let rv = self.emit_native("__jhol_rest_array", &[src, from_v]);
                        let r = self.alloc_temp();
                        self.emit(Op::Move, r, rv, 0);
                        self.bind(target, r)?;
                        self.free_temps_to(from_v);
                        continue;
                    }
                    let idx = self.alloc_temp();
                    let n = self.const_num(i as f64);
                    self.emit(Op::LoadK, idx, n, 0);
                    let tmp = self.alloc_temp();
                    self.emit(Op::GetIndex, tmp, src, idx);
                    if let Target::WithDefault(inner, def) = item {
                        self.bind_default(inner, def, tmp)?;
                    } else {
                        self.bind(item, tmp)?;
                    }
                    self.free_temps_to(idx);
                }
            }
            Target::Object(fields) => {
                for f in fields {
                    if f.rest {
                        // `{a, ...rest}` must exclude the names bound by the
                        // other fields. The call used to pass only the source,
                        // so every property landed in `rest` as well.
                        let excluded: Vec<String> = fields
                            .iter()
                            .filter(|o| !o.rest)
                            .filter_map(|o| match &o.key {
                                MemberKey::Dot(n) => Some(n.clone()),
                                // `{a: x, ...r}` keys a literal string key the
                                // same way a dot key is; only computed keys
                                // cannot be known at compile time.
                                MemberKey::Computed(e) => match e.as_ref() {
                                    Expr::Str(n) => Some(n.clone()),
                                    _ => None,
                                },
                            })
                            .collect();
                        let g = self.alloc_temp();
                        let gk = self.const_str(REST_OBJ);
                        self.emit(Op::GetGlobal, g, gk, 0);
                        // Call arguments must be contiguous starting at
                        // `callee + 1`, so they are allocated before the result
                        // temp: source first, then the excluded names.
                        let callee = self.alloc_temp();
                        self.emit(Op::Move, callee, g, 0);
                        let first = self.alloc_temp();
                        self.emit(Op::Move, first, src, 0);
                        for name in &excluded {
                            let t = self.alloc_temp();
                            let k = self.const_str(name);
                            self.emit(Op::LoadK, t, k, 0);
                        }
                        let rest = self.alloc_temp();
                        self.emit(Op::Call, rest, callee, 1 + excluded.len() as u32);
                        self.free_temps_to(g);
                        if let Some(t) = &f.target {
                            self.bind(t, rest)?;
                        }
                        continue;
                    }
                    let keytmp = match &f.key {
                        MemberKey::Computed(e) => {
                            let kt = self.alloc_temp();
                            self.expr(e, kt)?;
                            Some(kt)
                        }
                        MemberKey::Dot(_) => None,
                    };
                    let tmp = self.alloc_temp();
                    match &f.key {
                        MemberKey::Computed(e) => {
                            let _ = e;
                            self.emit(Op::GetIndex, tmp, src, keytmp.unwrap());
                        }
                        MemberKey::Dot(name) => {
                            let k = self.const_str(name);
                            let ip = self.emit(Op::GetProp, tmp, src, k);
                            let __k = self.key_of(name);
                            self.set_ic_key(ip, __k);
                        }
                    }
                    let sub = match &f.target {
                        Some(t) => t.as_ref().clone(),
                        None => match &f.key {
                            MemberKey::Dot(n) => Target::Simple(n.clone()),
                            MemberKey::Computed(_) => Target::Simple(String::new()),
                        },
                    };
                    // `{a = 1}` binds the default when the source is undefined.
                    match &f.default {
                        Some(d) => self.bind_default(&sub, d, tmp)?,
                        None => self.bind(&sub, tmp)?,
                    }
                    self.free_temps_to(tmp);
                }
            }
            Target::WithDefault(inner, def) => {
                self.bind_default(inner, def, src)?;
            }
            // `...rest` collects the tail at runtime and binds it like any
            // other pattern target.
            Target::Rest { from, target } => {
                let from_v = self.alloc_temp();
                let k = self.const_num(*from as f64);
                self.emit(Op::LoadK, from_v, k, 0);
                let rv = self.emit_native("__jhol_rest_array", &[src, from_v]);
                let r = self.alloc_temp();
                self.emit(Op::Move, r, rv, 0);
                self.bind(target, r)?;
            }
        }
        Ok(())
    }

    fn bind_default(&mut self, inner: &Target, def: &Expr, src: u32) -> Result<(), String> {
        let dv = self.alloc_temp();
        self.expr(def, dv)?;
        let und = self.alloc_temp();
        let ku = self.const_undef();
        self.emit(Op::LoadK, und, ku, 0);
        let cmp = self.alloc_temp();
        self.emit(Op::StrictEq, cmp, src, und);
        let skip = self.emit(Op::JumpIfFalse, cmp, 0, 0);
        self.emit(Op::Move, src, dv, 0);
        self.free_temps_to(dv);
        let after = self.here();
        self.patch_to(skip, after);
        self.bind(inner, src)
    }

    fn assign_to(&mut self, obj: &Expr, src: u32) -> Result<(), String> {
        match obj {
            Expr::Member { obj: base, key, .. } => {
                let ot = self.alloc_temp();
                self.expr(base, ot)?;
                match key {
                    MemberKey::Dot(name) => {
                        let k = self.const_str(name);
                        let ip = self.emit(Op::SetProp, ot, k, src);
                        let __k = self.key_of(name);
                        self.set_ic_key(ip, __k);
                    }
                    MemberKey::Computed(e) => {
                        let kt = self.alloc_temp();
                        self.expr(e, kt)?;
                        self.emit(Op::SetIndex, ot, kt, src);
                        self.free_temps_to(kt);
                    }
                }
                self.free_temps_to(ot);
            }
            // A bare identifier can be a local, a captured upvalue, or (when
            // nothing shadows it) a global; `store_local` picks.
            Expr::Ident(name) => self.store_local(name, src),
            Expr::This => {
                self.emit(Op::SetLocal, THIS_SLOT, src, 0);
            }
            _ => {
                let t = self.alloc_temp();
                self.expr(obj, t)?;
                self.free_temps_to(t);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// functions & classes
// ---------------------------------------------------------------------------
// classes
// ---------------------------------------------------------------------------

fn super_call(args: Vec<Expr>) -> Stmt {
    Stmt::Expr(Expr::Call {
        callee: Box::new(Expr::Super),
        args,
        optional: false,
    })
}

fn class_key_name(key: &ClassKey) -> String {
    match key {
        ClassKey::Name(n) => n.clone(),
        ClassKey::Private(n) => format!("#{}", n),
        ClassKey::Computed(_) => String::new(),
    }
}

fn method_key(name: &str, is_get: bool, is_set: bool) -> String {
    if is_get {
        format!("#get:{}", name)
    } else if is_set {
        format!("#set:{}", name)
    } else {
        name.to_string()
    }
}

impl Compiler {
    fn class_def(&mut self, c: &ClassDef, dst: u32) -> Result<(), String> {
        self.class_def_named(c, dst, None)
    }

    /// `display`: the name an anonymous class expression takes from its binding.
    fn class_def_named(
        &mut self,
        c: &ClassDef,
        dst: u32,
        display: Option<String>,
    ) -> Result<(), String> {
        // Hidden *global* naming this class, so `super.x` in any member can reach
        // its home class from any closure (see `home_stack`). A global, not a
        // local: `class_def` is entered with `dst` already allocated, and a local
        // declared here would be handed that live temp's slot. Unique per class,
        // so nesting and repetition cannot collide. Assigned once the class
        // object exists.
        self.class_serial += 1;
        let home_alias = format!("\u{0}home{}", self.class_serial);
        self.home_stack.push((home_alias.clone(), false));
        // `sup_proto` ends up holding the *superclass's prototype object*, which
        // becomes the class prototype's `__proto__`. Reading `.prototype` off
        // `null` for a base class would be a TypeError, so it is taken from
        // `Object.prototype` instead.
        let sup_proto = self.alloc_temp();
        let derived = c.superclass.is_some();
        let mut sup_reg: Option<u32> = None;
        let proto_key = self.key_of("prototype");
        let pkc = self.const_str("prototype");
        match &c.superclass {
            Some(e) => {
                let sup = self.alloc_temp();
                // A class name is bound in the global table too, and looking the
                // superclass up there keeps the value alive across the whole
                // `class` statement instead of depending on a local slot.
                if let Expr::Ident(name) = e.as_ref() {
                    self.load_global(name, sup)?;
                } else {
                    self.expr(e, sup)?;
                }
                let ip = self.emit(Op::GetProp, sup_proto, sup, pkc);
                self.set_ic_key(ip, proto_key);
                // Keep `sup` live: the constructor's own [[Prototype]] is bound
                // to it below. Everything from `sup_proto` is freed at the end.
                sup_reg = Some(sup);
            }
            None => {
                let g = self.alloc_temp();
                let gk = self.const_str("Object");
                self.emit(Op::GetGlobal, g, gk, 0);
                let ip = self.emit(Op::GetProp, sup_proto, g, pkc);
                self.set_ic_key(ip, proto_key);
                self.free_temps_to(g);
            }
        }

        let mut user_ctor: Option<Func> = None;
        for m in &c.members {
            if let ClassMemberKind::Method(f, false) = &m.kind {
                if let ClassKey::Name(n) = &m.key {
                    if n == "constructor" {
                        user_ctor = Some((**f).clone());
                    }
                }
            }
        }

        // Field initialisers become `this.#name = <init>` statements at the top
        // of the constructor. That is exactly what the spec desugars them to,
        // and it keeps accessor/field ordering right.
        let mut field_inits: Vec<Stmt> = Vec::new();
        for m in &c.members {
            // Static fields run once on the class below; adding them here
            // would repeat their side effects and install them on instances.
            let ClassMemberKind::Field(init) = &m.kind else {
                continue;
            };
            // Public fields keep their plain name; `#name` stays private.
            let key = match &m.key {
                ClassKey::Name(n) => MemberKey::Dot(n.clone()),
                ClassKey::Private(n) => MemberKey::Dot(format!("#{}", n)),
                ClassKey::Computed(e) => MemberKey::Computed(Box::new(e.as_ref().clone())),
            };
            let lhs = Expr::Member {
                obj: Box::new(Expr::This),
                key,
                optional: false,
            };
            field_inits.push(Stmt::Expr(Expr::Assign(
                AssignOp::Assign,
                Box::new(Target::Member(Box::new(lhs))),
                Box::new(init.as_ref().clone()),
            )));
        }

        let ctor: Func = match user_ctor {
            Some(mut u) => {
                let mut nb = std::mem::take(&mut field_inits);
                nb.append(&mut u.body);
                u.body = nb;
                u
            }
            None => {
                let params = if derived {
                    vec![Param {
                        target: Target::Simple("args".into()),
                        default: None,
                        rest: true,
                    }]
                } else {
                    Vec::new()
                };
                let mut body = field_inits;
                if derived {
                    body.insert(
                        0,
                        super_call(vec![Expr::Spread(Box::new(Expr::Ident("args".into())))]),
                    );
                }
                Func {
                    name: Some("constructor".into()),
                    params,
                    body,
                    is_arrow: false,
                    is_async: false,
                    is_generator: false,
                    expr_body: None,
                }
            }
        };

        self.cellify_pending();
        let ctx = if derived {
            FnCtx::DerivedCtor
        } else {
            FnCtx::Ctor
        };
        // The class's `.name` is the class name (or empty for an anonymous
        // expression), not the literal "constructor".
        let pidx = self.compile_fn(
            &ctor,
            ctx,
            Some(c.name.clone().or(display).unwrap_or_default()),
        )?;
        let k = self.const_fn(pidx);
        self.emit(Op::NewClosure, dst, k, 0);
        if let Some(sup) = sup_reg {
            self.emit(Op::SetProto, dst, sup, 0);
        }
        {
            let gk = self.const_str(&home_alias);
            self.emit(Op::DefGlobal, gk, dst, 0);
        }

        let proto = self.alloc_temp();
        self.emit(Op::NewObject, proto, 0, 0);
        self.emit(Op::SetProto, proto, sup_proto, 0);
        let cp = self.const_str("prototype");
        let ip = self.emit(Op::SetProp, dst, cp, proto);
        self.set_ic_key(ip, proto_key);
        let ctor_key = self.key_of("constructor");
        let ck = self.const_str("constructor");
        let ip = self.emit(Op::SetProp, proto, ck, dst);
        self.set_ic_key(ip, ctor_key);
        if let Some(n) = &c.name {
            // A class declaration binds a local in the enclosing scope, so
            // methods (and later code) can refer to it by name.
            if self.find_binding(n).is_none() {
                self.add_local(n);
            }
            self.store_local(n, dst);
        }

        for m in &c.members {
            // The parser marks getters and setters by prefixing the method's
            // name; recover the flag here so they install as accessors.
            let (f, is_get, is_set, is_static) = match &m.kind {
                ClassMemberKind::Method(f, s) => {
                    let name = f.name.clone().unwrap_or_default();
                    if name.starts_with("\u{1}g:") {
                        (f, true, false, *s)
                    } else if name.starts_with("\u{1}s:") {
                        (f, false, true, *s)
                    } else {
                        (f, false, false, *s)
                    }
                }
                ClassMemberKind::Field(_)
                | ClassMemberKind::StaticField(_)
                | ClassMemberKind::StaticBlock(_) => continue,
            };
            let target = if is_static { dst } else { proto };
            // `[expr]() {}`: the key expression is evaluated once while the
            // class is being defined, before the method closure is created.
            let computed_key = match &m.key {
                ClassKey::Computed(e) => {
                    let k = self.alloc_temp();
                    self.expr(e, k)?;
                    Some(k)
                }
                _ => None,
            };
            let t = self.alloc_temp();
            self.cellify_pending();
            if let Some(h) = self.home_stack.last_mut() {
                h.1 = is_static;
            }
            let pidx = self.compile_fn(f, FnCtx::ClassMethod, Some(class_key_name(&m.key)))?;
            let kk = self.const_fn(pidx);
            self.emit(Op::NewClosure, t, kk, 0);
            match computed_key {
                Some(k) => {
                    let k = if is_get || is_set {
                        let pref = self.alloc_temp();
                        let pc = self.const_str(if is_get { "#get:" } else { "#set:" });
                        self.emit(Op::LoadK, pref, pc, 0);
                        let joined = self.alloc_temp();
                        self.emit(Op::Add, joined, pref, k);
                        joined
                    } else {
                        k
                    };
                    self.emit(Op::SetIndex, target, k, t);
                    self.free_temps_to(t);
                }
                None => {
                    let name = method_key(&class_key_name(&m.key), is_get, is_set);
                    let name_key = self.key_of(&name);
                    let kc = self.const_str(&name);
                    let ip = self.emit(Op::DefProp, target, kc, t);
                    self.set_ic_key(ip, name_key);
                    self.free_temps_to(t);
                }
            }
        }

        // Static field initialisers and static blocks run against the
        // constructor itself, in source order — a later static block can read
        // what an earlier one wrote.
        for m in &c.members {
            match &m.kind {
                ClassMemberKind::StaticField(init) => {
                    let computed_key = match &m.key {
                        ClassKey::Computed(e) => {
                            let k = self.alloc_temp();
                            self.expr(e, k)?;
                            Some(k)
                        }
                        _ => None,
                    };
                    // A static field's `this` is the class, so its initialiser
                    // runs inside a thunk called with `dst` as receiver.
                    let this_slot = self.alloc_temp();
                    self.emit(Op::Move, this_slot, dst, 0);
                    let callee = self.alloc_temp();
                    self.cellify_pending();
                    if let Some(h) = self.home_stack.last_mut() {
                        h.1 = true;
                    }
                    let thunk = Func {
                        name: None,
                        params: Vec::new(),
                        body: Vec::new(),
                        is_arrow: false,
                        is_async: false,
                        is_generator: false,
                        expr_body: Some(init.clone()),
                    };
                    let pidx = self.compile_fn(&thunk, FnCtx::ClassMethod, None)?;
                    let k = self.const_fn(pidx);
                    self.emit(Op::NewClosure, callee, k, 0);
                    let val = self.alloc_temp();
                    self.emit(Op::CallThis, val, this_slot, 0);
                    match computed_key {
                        Some(k) => {
                            self.emit(Op::SetIndex, dst, k, val);
                        }
                        None => {
                            let name = class_key_name(&m.key);
                            let name_key = self.key_of(&name);
                            let kc = self.const_str(&name);
                            let ip = self.emit(Op::DefProp, dst, kc, val);
                            self.set_ic_key(ip, name_key);
                        }
                    }
                    self.free_temps_to(this_slot);
                }
                ClassMemberKind::StaticBlock(f) => {
                    // `CallThis` layout: `[this, callee, args...]`.
                    let this_slot = self.alloc_temp();
                    self.emit(Op::Move, this_slot, dst, 0);
                    let callee = self.alloc_temp();
                    self.cellify_pending();
                    if let Some(h) = self.home_stack.last_mut() {
                        h.1 = true;
                    }
                    let pidx = self.compile_fn(f, FnCtx::ClassMethod, Some("static".into()))?;
                    let k = self.const_fn(pidx);
                    self.emit(Op::NewClosure, callee, k, 0);
                    let r = self.alloc_temp();
                    self.emit(Op::CallThis, r, this_slot, 0);
                    self.free_temps_to(this_slot);
                }
                _ => {}
            }
        }

        self.home_stack.pop();
        self.free_temps_to(sup_proto);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// expressions
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Seg {
    kind: SegKind,
    optional: bool,
}

#[derive(Clone)]
enum SegKind {
    Dot(String),
    Computed(Expr),
    Call(Vec<Expr>),
}

fn flatten<'a>(e: &'a Expr, segs: &mut Vec<Seg>) -> &'a Expr {
    match e {
        Expr::Member { obj, key, optional } => {
            let base = flatten(obj, segs);
            let kind = match key {
                MemberKey::Dot(n) => SegKind::Dot(n.clone()),
                MemberKey::Computed(e) => SegKind::Computed((**e).clone()),
            };
            segs.push(Seg {
                kind,
                optional: *optional,
            });
            base
        }
        Expr::Call {
            callee,
            args,
            optional,
        } => {
            let base = flatten(callee, segs);
            segs.push(Seg {
                kind: SegKind::Call(args.clone()),
                optional: *optional,
            });
            base
        }
        Expr::OptionalChain(inner) => flatten(inner, segs),
        _ => e,
    }
}

impl Compiler {
    #[allow(dead_code)]
    fn peek_temp(&self) -> u32 {
        let fs = self.states.last().unwrap();
        fs.temps_top.max(fs.locals_top) as u32
    }

    /// Global-table read that skips the local/upvalue chain entirely.
    fn load_global(&mut self, name: &str, dst: u32) -> Result<(), String> {
        let k = self.const_str(name);
        let ip = self.emit(Op::GetGlobal, dst, k, 0);
        let gk = self.key_of(name);
        self.set_ic_key(ip, gk);
        Ok(())
    }

    /// Is `slot` currently in string-accumulator mode?
    fn is_accum(&self, slot: u32) -> bool {
        let n = self.states.len() - 1;
        self.states[n].accums.contains(&slot)
    }

    fn set_accum(&mut self, slot: u32, on: bool) {
        let n = self.states.len() - 1;
        if on {
            self.states[n].accums.insert(slot);
        } else {
            self.states[n].accums.remove(&slot);
        }
    }

    fn is_accum_ok(&self, slot: u32) -> bool {
        let n = self.states.len() - 1;
        self.states[n].accum_ok.contains(&slot)
    }

    fn set_accum_ok(&mut self, slot: u32, on: bool) {
        let n = self.states.len() - 1;
        if on {
            self.states[n].accum_ok.insert(slot);
        } else {
            self.states[n].accum_ok.remove(&slot);
        }
    }

    /// Is `name` a `let`/`const` of an enclosing block that has not been declared
    /// yet, in this same function? Then reading it must throw rather than look at
    /// the global object (which is what it did before, silently).
    fn is_pending_lexical(&self, name: &str) -> bool {
        let fs = self.states.len() - 1;
        self.pending_lexical
            .iter()
            .rev()
            .find(|(owner, set)| *owner == fs && set.contains(name))
            .is_some()
    }

    fn load_name(&mut self, name: &str, dst: u32) -> Result<(), String> {
        // Checked before the binding lookup: inside `let x = <init>` the slot for
        // `x` is already allocated, so a binding exists before a value does. That
        // ordering is exactly the dead zone.
        if self.is_pending_lexical(name) {
            let k = self.const_str(name);
            let _unused = dst;
            self.emit(Op::ThrowTdz, k, 0, 0);
            return Ok(());
        }
        if let Some(b) = self.find_binding(name) {
            // Reading an accumulated local materialises it first; afterwards the
            // slot is a plain string again.
            if !b.captured && self.is_accum(b.slot) {
                self.emit(Op::FlushStr, b.slot, 0, 0);
                self.set_accum_ok(b.slot, false);
            }
            if b.captured {
                self.emit(Op::CellGet, dst, b.slot, 0);
            } else {
                self.emit(Op::GetLocal, dst, b.slot, 0);
            }
            return Ok(());
        }
        if let Some(u) = self.resolve_upval(self.states.len() - 1, name) {
            self.emit(Op::GetUpval, dst, u, 0);
            return Ok(());
        }
        match name {
            "undefined" => {
                self.emit(Op::LoadUndefined, dst, 0, 0);
                return Ok(());
            }
            "NaN" => {
                let k = self.const_num(f64::NAN);
                self.emit(Op::LoadK, dst, k, 0);
                return Ok(());
            }
            "Infinity" => {
                let k = self.const_num(f64::INFINITY);
                self.emit(Op::LoadK, dst, k, 0);
                return Ok(());
            }
            _ => {}
        }
        let k = self.const_str(name);
        let ip = self.emit(Op::GetGlobal, dst, k, 0);
        let __k = self.key_of(name);
        self.set_ic_key(ip, __k);
        Ok(())
    }

    fn emit_native(&mut self, name: &str, srcs: &[u32]) -> u32 {
        let g = self.alloc_temp();
        let k = self.const_str(name);
        self.emit(Op::GetGlobal, g, k, 0);
        for s in srcs {
            let t = self.alloc_temp();
            self.emit(Op::Move, t, *s, 0);
        }
        let r = self.alloc_temp();
        self.emit(Op::Call, r, g, srcs.len() as u32);
        self.free_temps_to(g);
        r
    }

    fn new_array_lit(&mut self, items: &[Option<Expr>]) -> Result<u32, String> {
        let n = items.len();
        let base = self.alloc_temp();
        // `Op::NewArray` reads the element window `base+1..base+n` as a
        // contiguous block, so reserve it up front instead of letting each
        // element's `expr` drag the temp cursor around.
        let n32 = n as u32;
        let elem = self.alloc_temps(n32);
        for (slot, item) in items.iter().enumerate() {
            let t = elem + slot as u32;
            match item {
                Some(e) => self.expr(e, t)?,
                None => {
                    // Elisions are holes, not undefined-valued slots: a
                    // shared `Empty` constant keeps `[1, , 3]` a 3-length
                    // array with a missing middle for `in` and for-in.
                    let ec = self.const_empty();
                    self.emit(Op::LoadK, t, ec, 0);
                }
            }
            self.free_temps_to(elem + n32);
        }
        self.emit(Op::NewArray, base, n as u32, 0);
        // Hold the element window open so a later allocation cannot reuse the
        // destination register the result now lives in.
        self.free_temps_to(base + 1);
        Ok(base)
    }

    fn expr(&mut self, e: &Expr, dst: u32) -> Result<(), String> {
        match e {
            Expr::Member { .. } | Expr::Call { .. } | Expr::OptionalChain(_) => {
                self.expr_chain(e, dst)?;
            }
            Expr::Num(n) => {
                let k = self.const_num(*n);
                self.emit(Op::LoadK, dst, k, 0);
            }
            Expr::Str16(s) => {
                let value = arena::active().intern_atom_utf16(s);
                let k = self.const_atom(value);
                self.emit(Op::LoadK, dst, k, 0);
            }
            Expr::Str(s) => {
                let k = self.const_str(s);
                self.emit(Op::LoadK, dst, k, 0);
            }
            Expr::Bool(b) => {
                self.emit(if *b { Op::LoadTrue } else { Op::LoadFalse }, dst, 0, 0);
            }
            Expr::Null => {
                self.emit(Op::LoadNull, dst, 0, 0);
            }
            Expr::Undefined => {
                self.emit(Op::LoadUndefined, dst, 0, 0);
            }
            Expr::BigInt(i) => {
                let k = self.const_num(*i as f64);
                self.emit(Op::LoadK, dst, k, 0);
            }
            Expr::Ident(n) => {
                // `arguments` is not a real binding: it is an array of the
                // current frame's arguments. `BuildRest` with start 0 builds
                // exactly that from the frame's arg window, so no new opcode and
                // no per-call allocation in functions that never mention it.
                if n == "arguments" && self.find_binding("arguments").is_none() {
                    self.emit(Op::BuildRest, dst, 0, 0);
                } else {
                    self.load_name(n, dst)?
                }
            }
            Expr::This | Expr::Super | Expr::NewTarget => {
                if matches!(e, Expr::Super) {
                    let t = self.alloc_temp();
                    let home = self.home_stack.last().cloned();
                    let pk = self.key_of("__proto__");
                    let kc = self.const_str("__proto__");
                    if let Some((alias, is_static)) = home {
                        // Home.prototype.__proto__ (instance side) or
                        // Home.__proto__ (static side).
                        self.load_global(&alias, t)?;
                        if !is_static {
                            let prk = self.key_of("prototype");
                            let prc = self.const_str("prototype");
                            let ip = self.emit(Op::GetProp, t, t, prc);
                            self.set_ic_key(ip, prk);
                        }
                        let p = self.alloc_temp();
                        let ip = self.emit(Op::GetProp, p, t, kc);
                        self.set_ic_key(ip, pk);
                        self.emit(Op::Move, dst, p, 0);
                    } else {
                        // Outside a class (object-literal method): the receiver's
                        // prototype, as before.
                        self.load_name("\u{0}this", t)?;
                        let p = self.alloc_temp();
                        let ip = self.emit(Op::GetProp, p, t, kc);
                        self.set_ic_key(ip, pk);
                        self.emit(Op::Move, dst, p, 0);
                    }
                    self.free_temps_to(t);
                } else if matches!(e, Expr::NewTarget) {
                    self.load_name("\u{0}newtarget", dst)?;
                } else {
                    self.emit(Op::LoadThis, dst, 0, 0);
                }
            }
            Expr::Template(parts) => self.template(parts, dst)?,
            Expr::TaggedTemplate { tag, parts } => {
                // A tagged template calls `tag(strings, ...values)`: the values
                // are spread, so each one needs its own adjacent argument
                // register. Layout: `[callee, strings, value0, value1, ...]`.
                let callee = self.alloc_temp();
                let strings = self.alloc_temp();
                let val_parts: Vec<&Expr> = parts
                    .iter()
                    .filter_map(|p| match p {
                        TplPart::Expr(e) => Some(e.as_ref()),
                        _ => None,
                    })
                    .collect();
                let nvals = val_parts.len() as u32;
                let arg_base = if nvals > 0 {
                    self.alloc_temps(nvals)
                } else {
                    0
                };
                let str_parts: Vec<Option<Expr>> = parts
                    .iter()
                    .filter_map(|p| match p {
                        TplPart::Str { cooked, .. } => Some(Some(Expr::Str(cooked.clone()))),
                        _ => None,
                    })
                    .collect();
                let raw_parts: Vec<Option<Expr>> = parts
                    .iter()
                    .filter_map(|p| match p {
                        TplPart::Str { raw, .. } => Some(Some(Expr::Str(raw.clone()))),
                        _ => None,
                    })
                    .collect();
                let sr = self.new_array_lit(&str_parts)?;
                let rr = self.new_array_lit(&raw_parts)?;
                let tstr = self.emit_native("__jhol_tagged_template", &[sr, rr]);
                self.emit(Op::Move, strings, tstr, 0);
                for (i, e) in val_parts.iter().enumerate() {
                    self.expr(e, arg_base + i as u32)?;
                    self.free_temps_to(arg_base + nvals);
                }
                self.expr(tag, callee)?;
                self.emit(Op::Call, dst, callee, nvals + 1);
                self.free_temps_to(callee);
            }
            Expr::Regex(body, flags) => {
                let b = self.alloc_temp();
                let k = self.const_str(body);
                self.emit(Op::LoadK, b, k, 0);
                let f2 = self.alloc_temp();
                let k2 = self.const_str(flags);
                self.emit(Op::LoadK, f2, k2, 0);
                let r = self.emit_native("__jhol_new_regexp", &[b, f2]);
                self.emit(Op::Move, dst, r, 0);
            }
            Expr::Array(items) => {
                let r = if items.iter().any(|i| matches!(i, Some(Expr::Spread(_)))) {
                    self.spread_array_lit(items)?
                } else {
                    self.new_array_lit(items)?
                };
                self.emit(Op::Move, dst, r, 0);
            }
            Expr::Object(props) => {
                if self.object_lit_fused(props, dst)? {
                    return Ok(());
                }
                // Count what will be stored directly so the bag is sized once.
                // Spread can add more, but that only means the Vec grows later
                // — reserving too little is safe, reserving nothing is slow.
                let hint = props
                    .iter()
                    .filter(|p| !matches!(p, ObjProp::Spread(_)))
                    .count() as u32;
                self.emit(Op::NewObject, dst, hint, 0);
                for p in props {
                    match p {
                        ObjProp::Init(MemberKey::Dot(n), val) if n == "__proto__" => {
                            // Object-literal `__proto__: value` (non-computed,
                            // non-shorthand) sets [[Prototype]] rather than
                            // defining a property. Non-object values are ignored.
                            let t = self.alloc_temp();
                            self.expr(val, t)?;
                            self.emit(Op::SetProto, dst, t, 0);
                            self.free_temps_to(t);
                        }
                        ObjProp::Init(key, val) => {
                            let t = self.alloc_temp();
                            self.expr(val, t)?;
                            self.object_put(dst, key, None, t, false)?;
                            self.free_temps_to(t);
                        }
                        ObjProp::Shorthand(name) => {
                            let t = self.alloc_temp();
                            self.load_name(name, t)?;
                            self.object_put_named(dst, name, t);
                            self.free_temps_to(t);
                        }
                        ObjProp::Spread(e) => {
                            let t = self.alloc_temp();
                            self.expr(e, t)?;
                            let r = self.emit_native("__jhol_assign_all", &[dst, t]);
                            self.emit(Op::Move, dst, r, 0);
                        }
                        ObjProp::Method { key, func, kind } => {
                            let is_get = *kind == MethodKind::Get;
                            let is_set = *kind == MethodKind::Set;
                            let t = self.alloc_temp();
                            self.cellify_pending();
                            let pidx = self.compile_fn(func, FnCtx::Method, Some(String::new()))?;
                            let kk = self.const_fn(pidx);
                            self.emit(Op::NewClosure, t, kk, 0);
                            let name = match key {
                                MemberKey::Dot(n) => n.clone(),
                                MemberKey::Computed(_) => String::new(),
                            };
                            self.object_put_named2(dst, &name, t, is_get, is_set);
                            self.free_temps_to(t);
                        }
                    }
                }
            }
            Expr::Func(f) => {
                let n = f.name.clone().or_else(|| self.name_hint.take());
                self.new_closure(f, FnCtx::Func, n, dst)?;
            }
            Expr::Arrow(f) => {
                let n = f.name.clone().or_else(|| self.name_hint.take());
                self.new_closure(f, FnCtx::Arrow, n, dst)?;
            }
            Expr::Class(c) => {
                let hint = self.name_hint.take();
                self.class_def_named(c, dst, hint)?
            }
            Expr::Unary(op, inner) => self.unary(*op, inner, dst)?,
            // `is_prefix` picks the yielded value; both forms write the new
            // value back to the target, and the recorded operator decides
            // whether that value is `+1` or `-1`.
            Expr::Update(is_prefix, uop, inner) => {
                // `i++` on a plain local: load the constant once, then one
                // read-modify-write (postfix still needs the old value, so it
                // reads the local before the update).
                if let Expr::Ident(name) = inner.as_ref() {
                    if let Some(b) = self.find_binding(name) {
                        if !b.captured && *uop == crate::ast::UpdOp::Inc && !self.is_accum(b.slot) {
                            let one = self.alloc_temp();
                            let k = self.const_num(1.0);
                            self.emit(Op::LoadK, one, k, 0);
                            if !*is_prefix {
                                self.emit(Op::GetLocal, dst, b.slot, 0);
                                self.emit(Op::LocalAdd, b.slot, one, 0);
                            } else {
                                self.emit(Op::LocalAdd, b.slot, one, 0);
                                self.emit(Op::GetLocal, dst, b.slot, 0);
                            }
                            self.free_temps_to(one);
                            return Ok(());
                        }
                    }
                }
                let t = self.alloc_temp();
                self.expr(inner, t)?;
                let one = self.alloc_temp();
                let ko = self.const_num(1.0);
                self.emit(Op::LoadK, one, ko, 0);
                let nv = self.alloc_temp();
                match uop {
                    UpdOp::Inc => self.emit(Op::Add, nv, t, one),
                    UpdOp::Dec => self.emit(Op::Sub, nv, t, one),
                };
                self.assign_to(inner, nv)?;
                self.emit(Op::Move, dst, if *is_prefix { nv } else { t }, 0);
                self.free_temps_to(t);
            }
            Expr::Binary(op, l, r) => self.binary(*op, l, r, dst)?,
            Expr::Logical(op, l, r) => self.logical(*op, l, r, dst)?,
            Expr::Assign(op, target, value) => self.assign(*op, target, value, dst)?,
            Expr::Cond(c, a, b) => {
                let t = self.alloc_temp();
                self.expr(c, t)?;
                let else_ip = self.emit(Op::JumpIfFalse, t, 0, 0);
                self.free_temps_to(t);
                let ta = self.alloc_temp();
                self.expr(a, ta)?;
                self.emit(Op::Move, dst, ta, 0);
                self.free_temps_to(ta);
                let end_ip = self.emit(Op::Jump, 0, 0, 0);
                let mid = self.here();
                self.patch_to(else_ip, mid);
                let tb = self.alloc_temp();
                self.expr(b, tb)?;
                self.emit(Op::Move, dst, tb, 0);
                self.free_temps_to(tb);
                let end = self.here();
                self.patch_to(end_ip, end);
            }
            Expr::New(callee, args) => {
                if args.iter().any(|a| matches!(a, Expr::Spread(_))) {
                    let ct = self.alloc_temp();
                    self.expr(callee, ct)?;
                    let arr = self.alloc_temp();
                    self.emit(Op::NewArray, arr, 0, 0);
                    for a in args {
                        if let Expr::Spread(inner) = a {
                            let it = self.alloc_temp();
                            self.expr(inner, it)?;
                            let r = self.emit_native("__jhol_push_all", &[arr, it]);
                            self.emit(Op::Move, arr, r, 0);
                        } else {
                            let v = self.alloc_temp();
                            self.expr(a, v)?;
                            let r = self.emit_native("__jhol_push", &[arr, v]);
                            self.emit(Op::Move, arr, r, 0);
                        }
                    }
                    let r = self.emit_native("__jhol_apply_new", &[ct, arr]);
                    self.emit(Op::Move, dst, r, 0);
                } else {
                    let b = self.alloc_temp();
                    self.expr(callee, b)?;
                    // `Op::Construct` reads `b+1..b+n` as a contiguous window;
                    // interleaved `alloc_temp` + `expr` breaks that, exactly as
                    // it did for `Op::Call`.
                    let n = args.len() as u32;
                    let arg_base = self.alloc_temps(n);
                    for (i, a) in args.iter().enumerate() {
                        self.expr(a, arg_base + i as u32)?;
                        self.free_temps_to(arg_base + n);
                    }
                    let r = self.alloc_temp();
                    self.emit(Op::Construct, r, b, n);
                    self.free_temps_to(b);
                    self.emit(Op::Move, dst, r, 0);
                }
            }
            Expr::Seq(items) => {
                for (i, it) in items.iter().enumerate() {
                    if i + 1 == items.len() {
                        self.expr(it, dst)?;
                    } else {
                        let t = self.alloc_temp();
                        self.expr(it, t)?;
                        self.free_temps_to(t);
                    }
                }
            }
            // `yield e` suspends the frame; the value sent into `next()` is
            // threaded back through the same register on resume.
            Expr::Yield(inner, delegate) => {
                if *delegate {
                    let inner = inner
                        .as_ref()
                        .ok_or("SyntaxError: yield* needs an operand")?;
                    self.compile_yield_star(inner, dst)?;
                } else {
                    let t = self.alloc_temp();
                    match inner {
                        Some(e) => self.expr(e, t)?,
                        None => {
                            self.emit(Op::LoadUndefined, t, 0, 0);
                        }
                    }
                    self.emit(Op::Yield, t, 0, 0);
                    // On resume the VM writes the sent value back into `t`; move
                    // it to the expression destination so `const x = yield 1` works.
                    self.emit(Op::Move, dst, t, 0);
                }
            }
            Expr::Spread(_) => {
                return Err("SyntaxError: unsupported expression in this position".into())
            }
        }
        Ok(())
    }

    /// `[a, ...rest, b]` — built at runtime with the push helpers, because the
    /// element count is not known statically.
    fn spread_array_lit(&mut self, items: &[Option<Expr>]) -> Result<u32, String> {
        let arr = self.alloc_temp();
        self.emit(Op::NewArray, arr, 0, 0);
        for item in items {
            match item {
                None => {
                    let v = self.alloc_temp();
                    self.emit(Op::LoadUndefined, v, 0, 0);
                    let _ = self.emit_native("__jhol_push", &[arr, v]);
                }
                Some(Expr::Spread(inner)) => {
                    let v = self.alloc_temp();
                    self.expr(inner, v)?;
                    let _ = self.emit_native("__jhol_push_all", &[arr, v]);
                }
                Some(other) => {
                    let v = self.alloc_temp();
                    self.expr(other, v)?;
                    let _ = self.emit_native("__jhol_push", &[arr, v]);
                }
            }
        }
        Ok(arr)
    }

    fn object_put(
        &mut self,
        obj: u32,
        key: &MemberKey,
        _computed: Option<u32>,
        src: u32,
        _set: bool,
    ) -> Result<(), String> {
        match key {
            MemberKey::Dot(n) => {
                let k = self.const_str(n);
                let ip = self.emit(Op::DefProp, obj, k, src);
                let __k = self.key_of(n);
                self.set_ic_key(ip, __k);
            }
            MemberKey::Computed(e) => {
                let kt = self.alloc_temp();
                self.expr(e, kt)?;
                self.emit(Op::SetIndex, obj, kt, src);
                self.free_temps_to(kt);
            }
        }
        Ok(())
    }

    fn object_put_named(&mut self, obj: u32, name: &str, src: u32) {
        let k = self.const_str(name);
        let ip = self.emit(Op::DefProp, obj, k, src);
        let __k = self.key_of(name);
        self.set_ic_key(ip, __k);
    }

    fn object_put_named2(&mut self, obj: u32, name: &str, src: u32, is_get: bool, is_set: bool) {
        let real = method_key(name, is_get, is_set);
        let k = self.const_str(&real);
        let ip = self.emit(Op::DefProp, obj, k, src);
        let __k = self.key_of(&real);
        self.set_ic_key(ip, __k);
    }

    /// Emit `{…}` as a single `Op::NewObjectLit` when the literal is a plain
    /// bag of static-key data properties. Spread, computed keys, accessors,
    /// methods and duplicate keys all fall back to the general
    /// `NewObject` + one-`DefProp`-per-key sequence, which reproduces their
    /// exact semantics. Returns `false` when the literal was not fused.
    ///
    /// Values are evaluated into a contiguous temp block first, then handed to
    /// the opcode, so evaluation order (and the fact that the new object is not
    /// yet visible to the values) is preserved.
    fn object_lit_fused(&mut self, props: &[ObjProp], dst: u32) -> Result<bool, String> {
        let mut keys: Vec<PropKey> = Vec::with_capacity(props.len());
        for p in props {
            match p {
                // `{ __proto__: p }` sets the prototype; the general path does that.
                ObjProp::Init(MemberKey::Dot(n), _) if n == "__proto__" => return Ok(false),
                ObjProp::Init(MemberKey::Dot(n), _) => keys.push(self.key_of(n)),
                ObjProp::Shorthand(n) => keys.push(self.key_of(n)),
                _ => return Ok(false),
            }
        }
        let n = keys.len();
        // A repeated key would make the fusion emit a duplicate slot instead of
        // overwriting, so anything non-trivial uses the general path.
        for i in 0..n {
            if keys[i + 1..].contains(&keys[i]) {
                return Ok(false);
            }
        }
        let base = if n == 0 {
            0
        } else {
            self.alloc_temps(n as u32)
        };
        for (i, p) in props.iter().enumerate() {
            match p {
                ObjProp::Init(_, val) => self.expr(val, base + i as u32)?,
                ObjProp::Shorthand(name) => self.load_name(name, base + i as u32)?,
                _ => unreachable!("object_lit_fused rejected this property"),
            }
        }
        let kc = self.const_keys(keys);
        self.emit(Op::NewObjectLit, dst, kc, base);
        if n > 0 {
            self.free_temps_to(base);
        }
        Ok(true)
    }

    fn template(&mut self, parts: &[TplPart], dst: u32) -> Result<(), String> {
        let mut first = true;
        let mut cur = dst;
        for p in parts {
            match p {
                TplPart::Raw(src) => {
                    let mut sub = crate::parser::Parser::new(src)
                        .map_err(|m| format!("SyntaxError: bad template hole: {}", m))?;
                    let e = sub.parse_expression_only()?;
                    let t = self.alloc_temp();
                    self.expr(&e, t)?;
                    if first {
                        self.emit(Op::Move, cur, t, 0);
                        first = false;
                    } else {
                        let r = self.alloc_temp();
                        self.emit(Op::Add, r, cur, t);
                        // `cur` is live, so it must stay reserved.
                        cur = r;
                        // Reserve the new accumulator before anything else can
                        // allocate below it and clobber the running string.
                        self.free_temps_to(cur + 1);
                    }
                }
                TplPart::Str { cooked: s, .. } => {
                    if first {
                        let k = self.const_str(s);
                        self.emit(Op::LoadK, cur, k, 0);
                        first = false;
                    } else if !s.is_empty() {
                        let t = self.alloc_temp();
                        let k = self.const_str(s);
                        self.emit(Op::LoadK, t, k, 0);
                        let r = self.alloc_temp();
                        self.emit(Op::Add, r, cur, t);
                        cur = r;
                        // Reserve the new accumulator before anything else can
                        // allocate below it and clobber the running string.
                        self.free_temps_to(cur + 1);
                    }
                }
                TplPart::Expr(e) => {
                    let t = self.alloc_temp();
                    self.expr(e, t)?;
                    if first {
                        self.emit(Op::Move, cur, t, 0);
                        first = false;
                    } else {
                        let r = self.alloc_temp();
                        self.emit(Op::Add, r, cur, t);
                        cur = r;
                        // Reserve the new accumulator before anything else can
                        // allocate below it and clobber the running string.
                        self.free_temps_to(cur + 1);
                    }
                }
            }
        }
        if first {
            self.emit(Op::LoadUndefined, cur, 0, 0);
        }
        if cur != dst {
            self.emit(Op::Move, dst, cur, 0);
        }
        Ok(())
    }

    fn unary(&mut self, op: UnOp, inner: &Expr, dst: u32) -> Result<(), String> {
        let t = self.alloc_temp();
        self.expr(inner, t)?;
        match op {
            UnOp::Neg => {
                self.emit(Op::Neg, dst, t, 0);
            }
            UnOp::Not => {
                self.emit(Op::Not, dst, t, 0);
            }
            UnOp::BitNot => {
                self.emit(Op::BitNot, dst, t, 0);
            }
            UnOp::TypeOf => {
                self.emit(Op::TypeOf, dst, t, 0);
            }
            // `+x` is ToNumber, which is not what `x + 0` does for strings,
            // so it goes through the helper.
            UnOp::Plus => {
                let n = self.emit_native("__jhol_tonum", &[t]);
                self.emit(Op::Move, dst, n, 0);
            }
            UnOp::Void => {
                self.emit(Op::LoadUndefined, dst, 0, 0);
            }
            UnOp::Await => {
                if !self.states.last().map(|f| f.is_async).unwrap_or(false) {
                    return Err("SyntaxError: await outside an async function".into());
                }
                // `unary()` already evaluated the operand into `t`. Suspending on
                // it and then moving the resumed value into `dst` is one
                // evaluation; evaluating `inner` again here made the body observe
                // its own awaited expression twice (two rejected promises for one
                // `Promise.reject`).
                self.emit(Op::Yield, t, 0, 0);
                self.emit(Op::Move, dst, t, 0);
            }
            UnOp::Delete => {
                if let Expr::Member { obj, key, .. } = inner {
                    let ot = self.alloc_temp();
                    self.expr(obj, ot)?;
                    match key {
                        MemberKey::Dot(n) => {
                            let k = self.const_str(n);
                            let ip = self.emit(Op::DeleteProp, dst, ot, k);
                            let __k = self.key_of(n);
                            self.set_ic_key(ip, __k);
                        }
                        MemberKey::Computed(e) => {
                            let kt = self.alloc_temp();
                            self.expr(e, kt)?;
                            self.emit(Op::DeleteIndex, dst, ot, kt);
                            self.free_temps_to(kt);
                        }
                    }
                    self.free_temps_to(ot);
                } else {
                    self.emit(Op::LoadTrue, dst, 0, 0);
                }
            }
        }
        self.free_temps_to(t);
        Ok(())
    }

    /// Operator number for `Op::BinOpK`, or `None` when the operator has no
    /// constant form (equality, bitwise, `in`, `instanceof`, …).
    fn binop_k(op: BinOp) -> Option<u8> {
        Some(match op {
            BinOp::Add => 0,
            BinOp::Sub => 1,
            BinOp::Mul => 2,
            BinOp::Div => 3,
            BinOp::Mod => 4,
            BinOp::Lt => 5,
            BinOp::Gt => 6,
            BinOp::Le => 7,
            BinOp::Ge => 8,
            _ => return None,
        })
    }

    /// Slot of a plain local — not captured into a cell, not in string
    /// accumulator mode — that an operand can address directly. A local's slot
    /// *is* a register index, so `i + 1` / `o.x` need no `GetLocal` to copy it
    /// into a temporary first.
    fn plain_local_slot(&mut self, e: &Expr) -> Option<u32> {
        let name = match e {
            Expr::Ident(n) => n,
            _ => return None,
        };
        match self.find_binding(name) {
            Some(b) if !b.captured && !self.is_accum(b.slot) => Some(b.slot),
            _ => None,
        }
    }

    /// Whether `slot` is a register a chain result may be written into
    /// *directly*. Temps and ordinary locals are fine; a captured local's slot
    /// holds a `JCell`, and an accumulator slot holds pending string bytes, so
    /// storing a value over either would corrupt state the `Move` used to leave
    /// untouched.
    fn chain_dst_ok(&self, slot: u32) -> bool {
        let fs = self.states.last().unwrap();
        if slot as usize >= fs.locals_top {
            return true;
        }
        !*fs.captured.get(slot as usize).unwrap_or(&false) && !self.is_accum(slot)
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, dst: u32) -> Result<(), String> {
        // `x <op> <number>` never needs the constant in a register, and when
        // `x` is a plain local it does not need a temporary either: the whole
        // expression is one `BinOpK` instead of `GetLocal`, `LoadK` and a binop.
        if let (Some(sub), Expr::Num(n)) = (Self::binop_k(op), r) {
            let k = self.const_num(*n);
            match self.plain_local_slot(l) {
                Some(slot) => {
                    self.emit(Op::BinOpK(sub), dst, slot, k);
                }
                None => {
                    let t = self.alloc_temp();
                    self.expr(l, t)?;
                    self.emit(Op::BinOpK(sub), dst, t, k);
                    self.free_temps_to(t);
                }
            }
            return Ok(());
        }
        let a = self.alloc_temp();
        self.expr(l, a)?;
        let b = self.alloc_temp();
        self.expr(r, b)?;
        let emit_op = |c: &mut Compiler, o: Op| {
            c.emit(o, dst, a, b);
        };
        match op {
            BinOp::Add => emit_op(self, Op::Add),
            BinOp::Sub => emit_op(self, Op::Sub),
            BinOp::Mul => emit_op(self, Op::Mul),
            BinOp::Div => emit_op(self, Op::Div),
            BinOp::Mod => emit_op(self, Op::Mod),
            BinOp::Pow => emit_op(self, Op::Pow),
            BinOp::Lt => emit_op(self, Op::Lt),
            BinOp::Gt => emit_op(self, Op::Gt),
            BinOp::Le => emit_op(self, Op::Le),
            BinOp::Ge => emit_op(self, Op::Ge),
            BinOp::EqEq => emit_op(self, Op::Eq),
            BinOp::StrictEq => emit_op(self, Op::StrictEq),
            BinOp::BitAnd => emit_op(self, Op::BitAnd),
            BinOp::BitOr => emit_op(self, Op::BitOr),
            BinOp::BitXor => emit_op(self, Op::BitXor),
            BinOp::Shl => emit_op(self, Op::Shl),
            BinOp::Shr => emit_op(self, Op::Shr),
            BinOp::UShr => emit_op(self, Op::UShr),
            BinOp::In => emit_op(self, Op::In),
            BinOp::InstanceOf => emit_op(self, Op::InstanceOf),
            BinOp::NotEq => {
                emit_op(self, Op::Eq);
                self.emit(Op::Not, dst, dst, 0);
            }
            BinOp::StrictNotEq => {
                emit_op(self, Op::StrictEq);
                self.emit(Op::Not, dst, dst, 0);
            }
        }
        self.free_temps_to(a);
        Ok(())
    }

    fn logical(&mut self, op: LogicalOp, l: &Expr, r: &Expr, dst: u32) -> Result<(), String> {
        let a = self.alloc_temp();
        self.expr(l, a)?;
        let ip = match op {
            LogicalOp::And => self.emit(Op::JumpIfFalse, a, 0, 0),
            LogicalOp::Or => self.emit(Op::JumpIfTrue, a, 0, 0),
            // `??` is the odd one out: it evaluates the right operand only when
            // the left one is absent.
            LogicalOp::Nullish => self.emit(Op::JumpIfNotNilish, a, 0, 0),
        };
        let b = self.alloc_temp();
        self.expr(r, b)?;
        self.emit(Op::Move, a, b, 0);
        self.free_temps_to(a + 1);
        let end = self.here();
        self.patch_to(ip, end);
        self.emit(Op::Move, dst, a, 0);
        Ok(())
    }

    fn assign(
        &mut self,
        op: AssignOp,
        target: &Target,
        value: &Expr,
        dst: u32,
    ) -> Result<(), String> {
        // Destructuring assignment: evaluate the RHS once, then bind.
        if matches!(target, Target::Array(_) | Target::Object(_)) {
            let t = self.alloc_temp();
            self.expr(value, t)?;
            self.bind(target, t)?;
            self.emit(Op::Move, dst, t, 0);
            self.free_temps_to(t);
            return Ok(());
        }
        let expr_target = target_to_expr(target);
        let simple = match target {
            Target::Simple(n) => Some(n.clone()),
            _ => None,
        };
        let is_member = expr_target
            .as_ref()
            .map(|e| matches!(e, Expr::Member { .. }));

        if !matches!(op, AssignOp::AddAssign) {
            if let Some(name) = &simple {
                if let Some(b) = self.find_binding(name) {
                    if !b.captured {
                        self.set_accum(b.slot, false);
                        self.set_accum_ok(b.slot, false);
                    }
                }
            }
        }

        let binop = match op {
            AssignOp::AddAssign => Some(BinOp::Add),
            AssignOp::SubAssign => Some(BinOp::Sub),
            AssignOp::MulAssign => Some(BinOp::Mul),
            AssignOp::DivAssign => Some(BinOp::Div),
            AssignOp::ModAssign => Some(BinOp::Mod),
            AssignOp::PowAssign => Some(BinOp::Pow),
            AssignOp::ShlAssign => Some(BinOp::Shl),
            AssignOp::ShrAssign => Some(BinOp::Shr),
            AssignOp::UShrAssign => Some(BinOp::UShr),
            AssignOp::BitAndAssign => Some(BinOp::BitAnd),
            AssignOp::BitOrAssign => Some(BinOp::BitOr),
            AssignOp::BitXorAssign => Some(BinOp::BitXor),
            _ => None,
        };
        let logical = match op {
            AssignOp::AndAssign => Some(LogicalOp::And),
            AssignOp::OrAssign => Some(LogicalOp::Or),
            AssignOp::NullishAssign => Some(LogicalOp::Nullish),
            _ => None,
        };

        // `s += x` on an uncaptured local: accumulate in place. The buffer is
        // never a `Value`, so `var t = s` cannot alias it, and `load_name`
        // flushes before any other use of the variable.
        if matches!(op, AssignOp::AddAssign) {
            if let Some(name) = &simple {
                let binding = self.find_binding(name);
                let eligible = match binding {
                    Some(b) => !b.captured && self.is_accum_ok(b.slot),
                    None => false,
                };
                if let Some(b) = binding {
                    if eligible {
                        let v = self.alloc_temp();
                        self.expr(value, v)?;
                        self.emit(Op::AppendStr, b.slot, v, 0);
                        self.set_accum(b.slot, true);
                        self.free_temps_to(v);
                        self.emit(Op::GetLocal, dst, b.slot, 0);
                        return Ok(());
                    }
                }
            }
        }
        // `x += y` on an uncaptured local becomes one read-modify-write.
        // Placed after the string-accumulator path so `s += ch` keeps its
        // buffer, and after the accum-exit bookkeeping above.
        if matches!(op, AssignOp::AddAssign) {
            if let (Some(name), Some(BinOp::Add)) = (&simple, binop) {
                if let Some(b) = self.find_binding(name) {
                    if !b.captured && !self.is_accum(b.slot) {
                        let v = self.alloc_temp();
                        self.expr(value, v)?;
                        self.emit(Op::LocalAdd, b.slot, v, 0);
                        self.emit(Op::GetLocal, dst, b.slot, 0);
                        self.free_temps_to(v);
                        return Ok(());
                    }
                }
            }
        }
        match binop {
            Some(bop) => {
                let cur = self.alloc_temp();
                if let Some(e) = &expr_target {
                    self.expr(e, cur)?;
                } else {
                    self.emit(Op::LoadUndefined, cur, 0, 0);
                }
                let v = self.alloc_temp();
                self.expr(value, v)?;
                self.emit_arith(bop, cur, cur, v);
                if let Some(e) = &expr_target {
                    self.assign_to(e, cur)?;
                } else if let Some(n) = &simple {
                    self.store_local(n, cur);
                }
                self.emit(Op::Move, dst, cur, 0);
                self.free_temps_to(cur);
            }
            None => match logical {
                Some(lop) => {
                    let cur = self.alloc_temp();
                    if let Some(e) = &expr_target {
                        self.expr(e, cur)?;
                    } else {
                        self.emit(Op::LoadUndefined, cur, 0, 0);
                    }
                    let skip = match lop {
                        LogicalOp::And => self.emit(Op::JumpIfFalse, cur, 0, 0),
                        LogicalOp::Or => self.emit(Op::JumpIfTrue, cur, 0, 0),
                        LogicalOp::Nullish => self.emit(Op::JumpIfNilish, cur, 0, 0),
                    };
                    let v = self.alloc_temp();
                    self.expr(value, v)?;
                    if let Some(e) = &expr_target {
                        self.assign_to(e, v)?;
                    } else if let Some(n) = &simple {
                        self.store_local(n, v);
                    }
                    self.emit(Op::Move, cur, v, 0);
                    self.free_temps_to(cur);
                    let end = self.here();
                    self.patch_to(skip, end);
                    self.emit(Op::Move, dst, cur, 0);
                }
                None => {
                    if let Some(e) = &expr_target {
                        self.expr(value, dst)?;
                        self.assign_to(e, dst)?;
                    } else {
                        self.expr(value, dst)?;
                        if let Some(n) = &simple {
                            self.store_local(n, dst);
                        }
                    }
                    let _ = is_member;
                }
            },
        }
        Ok(())
    }

    fn emit_arith(&mut self, op: BinOp, dst: u32, a: u32, b: u32) {
        let o = match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Mod => Op::Mod,
            BinOp::Pow => Op::Pow,
            BinOp::Shl => Op::Shl,
            BinOp::Shr => Op::Shr,
            BinOp::UShr => Op::UShr,
            BinOp::BitAnd => Op::BitAnd,
            BinOp::BitOr => Op::BitOr,
            BinOp::BitXor => Op::BitXor,
            _ => Op::Add,
        };
        self.emit(o, dst, a, b);
    }
}

fn target_to_expr(t: &Target) -> Option<Expr> {
    match t {
        Target::Simple(n) => Some(Expr::Ident(n.clone())),
        Target::Member(e) => Some((**e).clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// member / call chains
// ---------------------------------------------------------------------------

impl Compiler {
    /// Compile a `a.b.c(d).e` style chain.
    ///
    /// The chain is flattened first and then emitted left to right, keeping two
    /// live registers: `val` (the value just produced) and `recv` (the receiver
    /// a following method call should pass as `this`). Optional links emit a
    /// single `JumpIfNilish` that jumps to the chain end, so the whole rest of
    /// the chain is skipped with no per-step bookkeeping.
    fn expr_chain(&mut self, e: &Expr, dst: u32) -> Result<(), String> {
        let mut segs: Vec<Seg> = Vec::new();
        let base = flatten(e, &mut segs);
        let base_is_super = matches!(base, Expr::Super);

        // When the receiver is a plain local, keep its slot: a local's slot is
        // a register index, so the first read can address it directly and the
        // `GetLocal` that used to materialise it disappears.
        let mut val = match self.plain_local_slot(base) {
            Some(slot) => slot,
            None => {
                let t = self.alloc_temp();
                self.expr(base, t)?;
                t
            }
        };
        let mut recv = val;
        let mut after_prop = false;
        let mut nil_needed = false;
        let mut nil_patches: Vec<usize> = Vec::new();

        for (si, seg) in segs.iter().enumerate() {
            // The last link owns the chain's destination register, so a call
            // there writes its result straight into `dst` and the closing
            // `Move dst, val` never gets emitted.
            let is_last = si + 1 == segs.len();
            // A link is guarded when it is itself optional, and every link after
            // an optional one is guarded too, because the value flowing in may
            // already be `undefined`.
            if seg.optional || nil_needed {
                let p = self.emit(Op::JumpIfNilish, val, 0, 0);
                nil_patches.push(p);
            }
            match &seg.kind {
                SegKind::Dot(name) => {
                    // A property read that ends the chain (`x = o.a`, or an
                    // operand of `+`) owns the chain's destination register:
                    // storing the field straight there is what the closing
                    // `Move dst, val` would have done, so it is not emitted.
                    let r = if is_last && self.chain_dst_ok(dst) {
                        dst
                    } else {
                        self.alloc_temp()
                    };
                    let k = self.const_str(name);
                    let ip = self.emit(Op::GetProp, r, val, k);
                    let __k = self.key_of(name);
                    self.set_ic_key(ip, __k);
                    // The value being read becomes the receiver for a following
                    // method call: `a.b.c()` calls `c` with `this = a.b`.
                    //
                    // Exception: `super.m()` looks `m` up on the parent prototype
                    // but must run it with the *current* `this`. Using the parent
                    // prototype as the receiver made the callee's own `this`
                    // (and so its `super.x`) resolve from the wrong object.
                    if base_is_super && si == 0 {
                        let th = self.alloc_temp();
                        self.emit(Op::LoadThis, th, 0, 0);
                        recv = th;
                    } else {
                        recv = val;
                    }
                    val = r;
                    after_prop = true;
                    if seg.optional {
                        nil_needed = true;
                    }
                }
                SegKind::Computed(key) => {
                    let kt = self.alloc_temp();
                    self.expr(key, kt)?;
                    let r = self.alloc_temp();
                    self.emit(Op::GetIndex, r, val, kt);
                    val = r;
                    after_prop = true;
                    if seg.optional {
                        nil_needed = true;
                    }
                }
                SegKind::Call(args) => {
                    // `super(...)` inside a derived constructor.
                    if base_is_super && !after_prop {
                        self.emit_ctor_call(args, Op::ConstructSuper, val, dst)?;
                        self.emit(Op::Move, dst, val, 0);
                        after_prop = false;
                        continue;
                    }
                    if args.iter().any(|a| matches!(a, Expr::Spread(_))) {
                        let r = self.emit_spread_call(args, val, recv, after_prop)?;
                        val = r;
                        recv = r;
                        after_prop = false;
                        continue;
                    }
                    let mut b = self.alloc_temp(); // callee (or `this` for methods)
                    let is_method = after_prop;
                    // Arguments must land in ADJACENT registers: the VM reads
                    // them as `arg_base..arg_base + n`. Allocating them one
                    // `alloc_temp()` at a time was wrong, because evaluating an
                    // argument calls `free_temps_to` internally and drags the
                    // temp cursor back — so the next argument landed in a slot
                    // the call never read, and the call saw whatever the
                    // expression had parked there (typically the receiver).
                    // Reserving the block up front and restoring the cursor
                    // after each argument keeps the layout stable.
                    let n = args.len() as u32;
                    if is_method {
                        // Layout: `[this, callee, args...]` — the callee needs
                        // its own slot, otherwise the first argument reuses it.
                        self.alloc_temp();
                        self.emit(Op::Move, b, recv, 0);
                        self.emit(Op::Move, b + 1, val, 0);
                        let arg_base = self.alloc_temps(n);
                        for (i, a) in args.iter().enumerate() {
                            self.expr(a, arg_base + i as u32)?;
                            self.free_temps_to(arg_base + n);
                        }
                        let r = if is_last { dst } else { self.alloc_temp() };
                        self.emit(Op::CallThis, r, b, n);
                        self.free_temps_to(b);
                        val = r;
                        recv = r;
                    } else {
                        // The callee almost always arrives in the temp allocated
                        // immediately before this block (a `GetUpval`, a global
                        // read, a property load). When it does, that slot *is*
                        // the callee register, and the staging `Move` vanishes.
                        // A temp can never be written by argument evaluation —
                        // those registers are claimed above this one — so the
                        // callee is still read before the arguments run.
                        let locals_top = self.states.last().unwrap().locals_top;
                        if val + 1 == b && val as usize >= locals_top {
                            self.free_temps_to(val);
                            b = self.alloc_temp();
                        } else {
                            self.emit(Op::Move, b, val, 0);
                        }
                        let arg_base = self.alloc_temps(n);
                        for (i, a) in args.iter().enumerate() {
                            self.expr(a, arg_base + i as u32)?;
                            self.free_temps_to(arg_base + n);
                        }
                        let r = if is_last { dst } else { self.alloc_temp() };
                        self.emit(Op::Call, r, b, n);
                        self.free_temps_to(b);
                        val = r;
                        recv = r;
                    }
                    after_prop = false;
                }
            }
        }
        if !nil_patches.is_empty() {
            // Normal path: store the value, then jump over the `undefined` fill
            // so it cannot clobber it. Guarded links jump straight to the fill.
            if val != dst {
                self.emit(Op::Move, dst, val, 0);
            }
            let skip = self.emit(Op::Jump, 0, 0, 0);
            let fill = self.here();
            self.emit(Op::LoadUndefined, dst, 0, 0);
            let end = self.here();
            self.patch_to(skip, end);
            for p in &nil_patches {
                self.patch_to(*p, fill);
            }
            return Ok(());
        }
        if val != dst {
            self.emit(Op::Move, dst, val, 0);
        }
        Ok(())
    }

    /// `new`-style call where the callee register already holds the function.
    fn emit_ctor_call(
        &mut self,
        args: &[Expr],
        op: Op,
        callee_reg: u32,
        _dst: u32,
    ) -> Result<u32, String> {
        if args.iter().any(|a| matches!(a, Expr::Spread(_))) {
            let arr = self.alloc_temp();
            self.emit(Op::NewArray, arr, 0, 0);
            for a in args {
                if let Expr::Spread(inner) = a {
                    let it = self.alloc_temp();
                    self.expr(inner, it)?;
                    let r = self.emit_native("__jhol_push_all", &[arr, it]);
                    self.emit(Op::Move, arr, r, 0);
                } else {
                    let v = self.alloc_temp();
                    self.expr(a, v)?;
                    let r = self.emit_native("__jhol_push", &[arr, v]);
                    self.emit(Op::Move, arr, r, 0);
                }
            }
            if op == Op::ConstructSuper {
                // The callee register is only a placeholder for `super`; the
                // parent is found at runtime, so `new`-applying it cannot work.
                let r = self.alloc_temp();
                self.emit(Op::ConstructSuperSpread, r, arr, 0);
                return Ok(r);
            }
            let r = self.emit_native("__jhol_apply_new", &[callee_reg, arr]);
            return Ok(r);
        }
        let b = self.alloc_temp();
        self.emit(Op::Move, b, callee_reg, 0);
        // Same contiguous-window requirement as `Op::Call`/`Op::Construct`.
        let n = args.len() as u32;
        let arg_base = self.alloc_temps(n);
        for (i, a) in args.iter().enumerate() {
            self.expr(a, arg_base + i as u32)?;
            self.free_temps_to(arg_base + n);
        }
        let r = self.alloc_temp();
        self.emit(op, r, b, args.len() as u32);
        self.free_temps_to(b);
        Ok(r)
    }

    /// A call whose arguments include a spread. The argument list is built at
    /// runtime with `__jhol_push`/`__jhol_push_all` and then applied, which is
    /// the only way to keep mixed spreads correct without a varargs ABI.
    fn emit_spread_call(
        &mut self,
        args: &[Expr],
        callee: u32,
        recv: u32,
        is_method: bool,
    ) -> Result<u32, String> {
        // Build the argument list at runtime, then hand it to `__jhol_apply`:
        // the only way to keep mixed spreads correct without a varargs ABI.
        let arr = self.alloc_temp();
        self.emit(Op::NewArray, arr, 0, 0);
        for a in args {
            match a {
                Expr::Spread(inner) => {
                    let it = self.alloc_temp();
                    self.expr(inner, it)?;
                    let r = self.emit_native("__jhol_push_all", &[arr, it]);
                    let _ = r;
                }
                other => {
                    let v = self.alloc_temp();
                    self.expr(other, v)?;
                    let r = self.emit_native("__jhol_push", &[arr, v]);
                    let _ = r;
                }
            }
        }
        if is_method {
            Ok(self.emit_native("__jhol_apply", &[callee, recv, arr]))
        } else {
            let undef = self.alloc_temp();
            self.emit(Op::LoadUndefined, undef, 0, 0);
            Ok(self.emit_native("__jhol_apply", &[callee, undef, arr]))
        }
    }
}

/// True when a *nested function* in `s` references `name`.
///
/// A direct reference (`sum += i`) does not count: only a closure that outlives
/// the iteration needs its own binding. Walking every mention instead made a
/// plain counting loop re-box its variable on all 50M iterations.
fn stmt_captures(name: &str, s: &Stmt) -> bool {
    match s {
        Stmt::Expr(e) => expr_captures(name, e),
        Stmt::Func(f) => func_mentions(name, f),
        Stmt::Class(c) => class_mentions(name, c),
        Stmt::Var(_, decls) => decls
            .iter()
            .any(|(_, i)| i.as_ref().is_some_and(|e| expr_captures(name, e))),
        Stmt::Block(b) => b.iter().any(|s| stmt_captures(name, s)),
        Stmt::If(c, a, b) => {
            expr_captures(name, c)
                || stmt_captures(name, a)
                || b.as_ref().is_some_and(|b| stmt_captures(name, b))
        }
        Stmt::While(c, b) | Stmt::With(c, b) => expr_captures(name, c) || stmt_captures(name, b),
        Stmt::DoWhile(b, c) => stmt_captures(name, b) || expr_captures(name, c),
        Stmt::For(i, t, u, b) => {
            i.as_ref().is_some_and(|i| stmt_captures(name, i))
                || t.as_ref().is_some_and(|t| expr_captures(name, t))
                || u.as_ref().is_some_and(|u| expr_captures(name, u))
                || stmt_captures(name, b)
        }
        Stmt::ForInOf { expr, body, .. } => expr_captures(name, expr) || stmt_captures(name, body),
        Stmt::Return(Some(e)) | Stmt::Throw(e) => expr_captures(name, e),
        Stmt::Try(b, c, f) => {
            stmt_captures(name, b)
                || c.as_ref().is_some_and(|(_, cb)| stmt_captures(name, cb))
                || f.as_ref().is_some_and(|f| stmt_captures(name, f))
        }
        Stmt::Switch(d, cases) => {
            expr_captures(name, d)
                || cases.iter().any(|(t, b)| {
                    t.as_ref().is_some_and(|t| expr_captures(name, t))
                        || b.iter().any(|s| stmt_captures(name, s))
                })
        }
        Stmt::Labeled(_, b) => stmt_captures(name, b),
        _ => false,
    }
}

fn expr_captures(name: &str, e: &Expr) -> bool {
    match e {
        Expr::Ident(_) => false,
        Expr::Func(f) | Expr::Arrow(f) => func_mentions(name, f),
        Expr::Class(c) => class_mentions(name, c),
        Expr::Template(parts) | Expr::TaggedTemplate { parts, .. } => parts
            .iter()
            .any(|p| matches!(p, TplPart::Expr(e) if expr_captures(name, e))),
        Expr::Array(items) => items.iter().flatten().any(|e| expr_captures(name, e)),
        Expr::Object(props) => props.iter().any(|p| match p {
            ObjProp::Init(k, v) => {
                matches!(k, MemberKey::Computed(e) if expr_captures(name, e))
                    || expr_captures(name, v)
            }
            ObjProp::Shorthand(n) => n == name,
            ObjProp::Spread(e) => expr_captures(name, e),
            ObjProp::Method { key, func, .. } => {
                matches!(key, MemberKey::Computed(e) if expr_captures(name, e))
                    || func_mentions(name, func)
            }
        }),
        Expr::Unary(_, e)
        | Expr::Update(.., e)
        | Expr::Spread(e)
        | Expr::Yield(Some(e), _)
        | Expr::OptionalChain(e) => expr_captures(name, e),
        Expr::Yield(None, _) => false,
        Expr::Binary(_, l, r) | Expr::Logical(_, l, r) => {
            expr_captures(name, l) || expr_captures(name, r)
        }
        Expr::Assign(_, _t, r) => expr_captures(name, r),
        Expr::Cond(c, a, b) => {
            expr_captures(name, c) || expr_captures(name, a) || expr_captures(name, b)
        }
        Expr::Call { callee, args, .. } => {
            expr_captures(name, callee) || args.iter().any(|a| expr_captures(name, a))
        }
        Expr::New(callee, args) => {
            expr_captures(name, callee) || args.iter().any(|a| expr_captures(name, a))
        }
        Expr::Member { obj, .. } => expr_captures(name, obj),
        Expr::Seq(items) => items.iter().any(|e| expr_captures(name, e)),
        _ => false,
    }
}

/// True when some nested `function`/arrow in `s` references `name`. Used to
/// decide whether a `let` loop variable needs a per-iteration cell.
fn stmt_mentions(name: &str, s: &Stmt) -> bool {
    match s {
        Stmt::Expr(e) => expr_mentions(name, e),
        Stmt::Var(_, decls) => decls
            .iter()
            .any(|(_, i)| i.as_ref().map_or(false, |e| expr_mentions(name, e))),
        Stmt::Block(b) => b.iter().any(|s| stmt_mentions(name, s)),
        Stmt::If(c, a, b) => {
            expr_mentions(name, c)
                || stmt_mentions(name, a)
                || b.as_ref().is_some_and(|b| stmt_mentions(name, b))
        }
        Stmt::While(c, b) | Stmt::With(c, b) => expr_mentions(name, c) || stmt_mentions(name, b),
        Stmt::DoWhile(b, c) => stmt_mentions(name, b) || expr_mentions(name, c),
        Stmt::For(i, t, u, b) => {
            i.as_ref().is_some_and(|i| stmt_mentions(name, i))
                || t.as_ref().is_some_and(|t| expr_mentions(name, t))
                || u.as_ref().is_some_and(|u| expr_mentions(name, u))
                || stmt_mentions(name, b)
        }
        Stmt::ForInOf { expr, body, .. } => expr_mentions(name, expr) || stmt_mentions(name, body),
        Stmt::Return(Some(e)) | Stmt::Throw(e) => expr_mentions(name, e),
        Stmt::Try(b, c, f) => {
            stmt_mentions(name, b)
                || c.as_ref().is_some_and(|(_, cb)| stmt_mentions(name, cb))
                || f.as_ref().is_some_and(|f| stmt_mentions(name, f))
        }
        Stmt::Switch(d, cases) => {
            expr_mentions(name, d)
                || cases.iter().any(|(t, b)| {
                    t.as_ref().is_some_and(|t| expr_mentions(name, t))
                        || b.iter().any(|s| stmt_mentions(name, s))
                })
        }
        Stmt::Labeled(_, b) => stmt_mentions(name, b),
        Stmt::Func(f) => func_mentions(name, f),
        Stmt::Class(c) => class_mentions(name, c),
        _ => false,
    }
}

fn func_mentions(name: &str, f: &Func) -> bool {
    let params = f.params.iter().any(|p| {
        target_mentions(name, &p.target)
            || p.default.as_ref().is_some_and(|d| expr_mentions(name, d))
    });
    if let Some(e) = &f.expr_body {
        return params || expr_mentions(name, e);
    }
    params || f.body.iter().any(|s| stmt_mentions(name, s))
}

fn target_mentions(name: &str, t: &Target) -> bool {
    match t {
        Target::Simple(n) => n == name,
        Target::Member(e) => expr_mentions(name, e),
        Target::Array(items) => items.iter().flatten().any(|t| target_mentions(name, t)),
        Target::Object(fs) => fs.iter().any(|f| {
            matches!(f.key, MemberKey::Computed(ref e) if expr_mentions(name, e))
                || f.default.as_ref().is_some_and(|d| expr_mentions(name, d))
                || f.target.as_ref().is_some_and(|t| target_mentions(name, t))
        }),
        Target::WithDefault(t, d) => target_mentions(name, t) || expr_mentions(name, d),
        Target::Rest { target, .. } => target_mentions(name, target),
    }
}

fn class_mentions(name: &str, c: &ClassDef) -> bool {
    c.superclass
        .as_ref()
        .is_some_and(|s| expr_mentions(name, s))
        || c.members.iter().any(|m| match &m.kind {
            ClassMemberKind::Method(f, _) => {
                func_mentions(name, f)
                    || match &m.key {
                        ClassKey::Computed(e) => expr_mentions(name, e),
                        _ => false,
                    }
            }
            ClassMemberKind::Field(e) | ClassMemberKind::StaticField(e) => expr_mentions(name, e),
            ClassMemberKind::StaticBlock(f) => func_mentions(name, f),
        })
}

fn expr_mentions(name: &str, e: &Expr) -> bool {
    match e {
        Expr::Ident(n) => n == name,
        Expr::This
        | Expr::Super
        | Expr::NewTarget
        | Expr::Num(_)
        | Expr::Str(_)
        | Expr::Str16(_)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Undefined
        | Expr::BigInt(_)
        | Expr::Regex(_, _) => false,
        Expr::Template(parts) | Expr::TaggedTemplate { parts, .. } => {
            parts.iter().any(|p| match p {
                TplPart::Str { .. } => false,
                TplPart::Raw(_) => false,
                TplPart::Expr(e) => expr_mentions(name, e),
            })
        }
        Expr::Array(items) => items.iter().flatten().any(|e| expr_mentions(name, e)),
        Expr::Object(props) => props.iter().any(|p| match p {
            ObjProp::Init(k, v) => {
                matches!(k, MemberKey::Computed(e) if expr_mentions(name, e))
                    || expr_mentions(name, v)
            }
            ObjProp::Shorthand(n) => n == name,
            ObjProp::Spread(e) => expr_mentions(name, e),
            ObjProp::Method { key, func, .. } => {
                matches!(key, MemberKey::Computed(e) if expr_mentions(name, e))
                    || func_mentions(name, func)
            }
        }),
        Expr::Unary(_, e)
        | Expr::Update(.., e)
        | Expr::Spread(e)
        | Expr::Yield(Some(e), _)
        | Expr::OptionalChain(e) => expr_mentions(name, e),
        Expr::Yield(None, _) => false,
        Expr::Binary(_, l, r) | Expr::Logical(_, l, r) => {
            expr_mentions(name, l) || expr_mentions(name, r)
        }
        Expr::Assign(_, _t, r) => expr_mentions(name, r),
        Expr::Cond(c, a, b) => {
            expr_mentions(name, c) || expr_mentions(name, a) || expr_mentions(name, b)
        }
        Expr::Call { callee, args, .. } => {
            expr_mentions(name, callee) || args.iter().any(|a| expr_mentions(name, a))
        }
        Expr::New(callee, args) => {
            expr_mentions(name, callee) || args.iter().any(|a| expr_mentions(name, a))
        }
        Expr::Member { obj, .. } => expr_mentions(name, obj),
        Expr::Seq(items) => items.iter().any(|e| expr_mentions(name, e)),
        Expr::Func(f) | Expr::Arrow(f) => func_mentions(name, f),
        Expr::Class(c) => class_mentions(name, c),
    }
}
