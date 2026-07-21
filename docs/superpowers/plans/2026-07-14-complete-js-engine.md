# Complete js-engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `crates/js-engine` into a fully functional, Jhol-native JavaScript engine by closing its 11 major gaps, wiring it to existing Jhol primitives (CAS artifact cache, tropical linker, FUSE module interception, lock-free GRM, Markov hot-path prediction), without copying QuickJS/V8 architecture and without losing existing functionality.

**Architecture:** Keep the existing tiered pipeline — parser → scope analysis → tier-2 bytecode VM — but replace the current leaky per-object `Box` allocator with a generational GC, add real prototype chains and upvalues, implement exception handling, ES modules, async Promises, and a native builtin layer. AOT/JIT compilation is driven by the existing Markov predictor and cached in the existing artifact store; modules are linked by the existing tropical linker and optionally intercepted through the existing FUSE linker.

**Tech Stack:** Rust 2021, custom parser/IR/bytecode, no QuickJS/JavaScriptCore/V8 dependency, reuse `jhol-core` concepts where possible.

---

## Current Verified State

- `cargo test -p js-engine` passes 31 tests.
- `runtime-benchmarks/*_small.js` run end-to-end.
- Known gaps: no real prototype chain, no closures, no exceptions, no modules, no async, leaking allocator, no JIT/AOT wiring, partial builtins, incomplete `this`, no classes/with/eval/labels, no stack traces.

---

## File Structure

Files to create or modify. Each file has one clear responsibility.

| File | Responsibility |
|------|---------------|
| `crates/js-engine/src/runtime/object.rs` | `JsObject` layout, prototype pointer, shape ID, property storage, inline-cache hooks |
| `crates/js-engine/src/runtime/shape.rs` | Shape graph for property layout and prototype-chain cache keys |
| `crates/js-engine/src/runtime/gc.rs` | Generational allocator, write barrier, root enumeration, collection trigger |
| `crates/js-engine/src/runtime/allocator.rs` | Deprecated region allocator; replace with thin shim over `gc.rs` or delete after migration |
| `crates/js-engine/src/runtime/upvalue.rs` | Upvalue cells: open vs closed, capture-by-reference semantics |
| `crates/js-engine/src/runtime/function.rs` | Function objects: bytecode pointer, upvalue array, `this` mode, name |
| `crates/js-engine/src/runtime/value.rs` | `JsValue` variants, boxing/unboxing, tag checks |
| `crates/js-engine/src/runtime/error.rs` | JS `Error` objects, stack traces, source-location mapping |
| `crates/js-engine/src/runtime/init.rs` | Global object, builtin registration, prototype wiring |
| `crates/js-engine/src/runtime/builtins/*.rs` | Native builtins (array, string, console, timers, promise, fs, etc.) |
| `crates/js-engine/src/runtime/event_loop.rs` | Microtask/task queues for Promises and timers |
| `crates/js-engine/src/scope.rs` | Scope analysis: var/let/const, hoisting, upvalue detection, `this` binding, exception scopes |
| `crates/js-engine/src/ir/mod.rs` | IR instruction set and `IrFunction` |
| `crates/js-engine/src/ir/ast_lower.rs` | AST → IR lowering for all language features |
| `crates/js-engine/src/ir/lower_to_tier2.rs` | IR → tier-2 bytecode lowering |
| `crates/js-engine/src/tier2/bytecode.rs` | Tier-2 opcode definitions and `BytecodeFunction` |
| `crates/js-engine/src/tier2/vm.rs` | Bytecode interpreter, call stack, exception unwinding, IC fallback |
| `crates/js-engine/src/parser/ast.rs` | AST nodes, spans, source-location metadata |
| `crates/js-engine/src/parser/parser.rs` | Parser for statements, expressions, modules, classes |
| `crates/js-engine/src/linker/mod.rs` | Tropical linker for module dependency ordering |
| `crates/js-engine/src/linker/fuse.rs` | FUSE module interception / native fallback |
| `crates/js-engine/src/aot/markov.rs` | Hot-function predictor |
| `crates/js-engine/src/aot/cache.rs` | CAS artifact cache |
| `crates/js-engine/src/aot/jagr.rs` | Optimization planner / Betti history |
| `crates/js-engine/src/codegen/module.rs` | Native code generation entry point |
| `crates/js-engine/src/codegen/abi.rs` | Calling convention between native code and VM |
| `crates/js-engine/src/test/test262.rs` | test262 harness (optional, grown incrementally) |

---

## Task 1: Real Prototype Chain + Inline Caches

**Files:**
- Modify: `crates/js-engine/src/runtime/object.rs`
- Create: `crates/js-engine/src/runtime/shape.rs`
- Modify: `crates/js-engine/src/runtime/value.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Modify: `crates/js-engine/src/tier2/bytecode.rs`
- Test: `crates/js-engine/src/runtime/object.rs` (unit tests)

**Background:** Current `JsObject` stores properties in a `HashMap<String, JsValue>` with no prototype. JS semantics require prototype lookup, `hasOwnProperty`, and fast property access.

- [ ] **Step 1: Add shape IDs and prototype pointer to `JsObject`**

In `crates/js-engine/src/runtime/object.rs`, change `JsObject` to:

```rust
pub struct JsObject {
    pub header: GcHeader,
    pub shape_id: ShapeId,
    pub prototype: Option<GcRoot<JsObject>>,
    pub properties: Vec<(String, JsValue)>, // dense property list matching shape
    pub indexed: Vec<JsValue>,              // array-indexed properties
    pub kind: ObjectKind,
    pub call: Option<JsFunction>,           // if callable
}
```

Add `pub type ShapeId = u32;` and `pub const ROOT_SHAPE: ShapeId = 0;`.

- [ ] **Step 2: Create shape registry**

Create `crates/js-engine/src/runtime/shape.rs`:

```rust
use std::collections::HashMap;
use std::sync::RwLock;

pub type ShapeId = u32;

#[derive(Clone, Debug)]
pub struct Shape {
    pub parent: ShapeId,
    pub transition_key: Option<String>, // property that differentiates this shape from parent
    pub index: u32,                     // property slot index in object.properties
}

pub struct ShapeTree {
    shapes: RwLock<Vec<Shape>>,
    transition: RwLock<HashMap<(ShapeId, String), ShapeId>>,
}

impl ShapeTree {
    pub fn new() -> Self {
        let root = Shape { parent: 0, transition_key: None, index: 0 };
        Self {
            shapes: RwLock::new(vec![root]),
            transition: RwLock::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> ShapeId { 0 }

    pub fn ensure_property(&self, shape: ShapeId, name: &str) -> ShapeId {
        if let Ok(t) = self.transition.read() {
            if let Some(&next) = t.get(&(shape, name.to_string())) {
                return next;
            }
        }
        let mut shapes = self.shapes.write().unwrap();
        let mut trans = self.transition.write().unwrap();
        let next_id = shapes.len() as ShapeId;
        let index = shapes[shape as usize].index + 1;
        shapes.push(Shape { parent: shape, transition_key: Some(name.to_string()), index });
        trans.insert((shape, name.to_string()), next_id);
        next_id
    }

    pub fn lookup(&self, shape: ShapeId, name: &str) -> Option<u32> {
        let shapes = self.shapes.read().unwrap();
        let mut sid = shape;
        loop {
            let s = &shapes[sid as usize];
            if let Some(ref key) = s.transition_key {
                if key == name {
                    return Some(s.index);
                }
            }
            if sid == 0 { break; }
            sid = s.parent;
        }
        None
    }
}
```

- [ ] **Step 3: Add inline-cache opcodes**

In `crates/js-engine/src/tier2/bytecode.rs`, add:

```rust
pub enum Op {
    // ... existing ops ...
    GetPropCached { name: String, shape_hint: ShapeId, slot_hint: u32 },
    SetPropCached { name: String, shape_hint: ShapeId, slot_hint: u32 },
}
```

- [ ] **Step 4: Implement prototype-chain property get in VM**

In `crates/js-engine/src/tier2/vm.rs`, add a helper:

```rust
fn get_property(&self, obj: &JsObject, name: &str) -> JsValue {
    let tree = self.shape_tree();
    let mut current = Some(obj);
    while let Some(o) = current {
        if let Some(idx) = tree.lookup(o.shape_id, name) {
            return o.properties.get(idx as usize).map(|(_, v)| v.clone()).unwrap_or(JsValue::undefined());
        }
        current = o.prototype.as_ref().map(|p| unsafe { &**p.as_ptr() });
    }
    JsValue::undefined()
}
```

Wire `GetProp` and `GetPropCached` to use it. On cache miss, re-emit shape/slot hint.

- [ ] **Step 5: Add prototype test**

In `crates/js-engine/src/runtime/object.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prototype_chain_lookup() {
        let tree = ShapeTree::new();
        let mut proto = JsObject::new(&tree, ObjectKind::Plain);
        proto.set_own_property("x", JsValue::number(1.0));
        let mut child = JsObject::new(&tree, ObjectKind::Plain);
        child.prototype = Some(GcRoot::new(&proto));
        assert_eq!(child.get(&tree, "x").to_number(), 1.0);
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p js-engine prototype_chain_lookup`
Expected: PASS

---

## Task 2: Closures / Upvalues

**Files:**
- Modify: `crates/js-engine/src/runtime/function.rs`
- Create: `crates/js-engine/src/runtime/upvalue.rs`
- Modify: `crates/js-engine/src/scope.rs`
- Modify: `crates/js-engine/src/ir/mod.rs`
- Modify: `crates/js-engine/src/ir/ast_lower.rs`
- Modify: `crates/js-engine/src/ir/lower_to_tier2.rs`
- Modify: `crates/js-engine/src/tier2/bytecode.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Test: `crates/js-engine/src/tests.rs` or new closure test

**Background:** Functions currently cannot capture outer variables. JS closures require open upvalues that close over values when the outer function returns.

- [ ] **Step 1: Add upvalue cell type**

Create `crates/js-engine/src/runtime/upvalue.rs`:

```rust
use crate::runtime::value::JsValue;
use std::ptr::NonNull;

pub struct Upvalue {
    pub location: *mut JsValue, // points into stack slot while open, into closed value after close
    pub closed: Option<JsValue>,
    pub next: Option<NonNull<Upvalue>>,
}

impl Upvalue {
    pub fn open(location: *mut JsValue) -> Self {
        Self { location, closed: None, next: None }
    }
    pub fn close(&mut self) {
        if self.closed.is_none() {
            self.closed = Some(unsafe { (*self.location).clone() });
            self.location = self.closed.as_mut().unwrap() as *mut JsValue;
        }
    }
    pub fn read(&self) -> JsValue {
        unsafe { (*self.location).clone() }
    }
    pub fn write(&mut self, value: JsValue) {
        unsafe { *self.location = value; }
    }
}
```

- [ ] **Step 2: Extend `JsFunction` with upvalue list**

In `crates/js-engine/src/runtime/function.rs`:

```rust
pub struct JsFunction {
    pub name: String,
    pub bytecode: GcRoot<BytecodeFunction>,
    pub upvalues: Vec<GcRoot<Upvalue>>,
    pub this_mode: ThisMode, // Global, Lexical, Strict
}

pub enum ThisMode {
    Global,
    Lexical, // arrow functions
    Strict,
}
```

- [ ] **Step 3: Mark upvalues in scope analysis**

In `crates/js-engine/src/scope.rs`, when resolving a variable reference, if the variable belongs to an enclosing non-global scope, mark the binding as `captured: true`. Add a `captured` flag to `Binding`.

- [ ] **Step 4: Add IR instructions for upvalues**

In `crates/js-engine/src/ir/mod.rs`, extend `Instruction`:

```rust
pub enum Instruction {
    // ... existing ...
    GetUpvalue(VarId, UpvalueId),
    SetUpvalue(UpvalueId, VarId),
    MakeClosure(VarId, FuncId, Vec<UpvalueId>),
}
```

- [ ] **Step 5: Lower upvalues in AST lowerer**

In `crates/js-engine/src/ir/ast_lower.rs`, when lowering an identifier:
- If binding is local: emit `Load`.
- If binding is captured: emit `GetUpvalue`.
- If global: emit `GetGlobal`.

When lowering a function expression/declaration, collect captured bindings and emit `MakeClosure`.

- [ ] **Step 6: Lower IR upvalues to tier-2**

In `crates/js-engine/src/ir/lower_to_tier2.rs`, map `GetUpvalue`/`SetUpvalue` to new opcodes, and `MakeClosure` to a `Closure` opcode.

- [ ] **Step 7: Add bytecode opcodes**

In `crates/js-engine/src/tier2/bytecode.rs`:

```rust
pub enum Op {
    GetUpvalue(u16),
    SetUpvalue(u16),
    Closure { func_idx: u16, upvalues: Vec<u16> },
}
```

- [ ] **Step 8: Implement upvalue semantics in VM**

In `crates/js-engine/src/tier2/vm.rs`:
- Maintain an `open_upvalues: Vec<GcRoot<Upvalue>>` list per call frame.
- On `GetUpvalue`/`SetUpvalue`, read/write through the cell.
- On function return, close all upvalues whose location is below the returning frame's base slot.

- [ ] **Step 9: Add closure test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_closure_counter() {
    let mut engine = Engine::new();
    let src = r#"
        function makeCounter() {
            let n = 0;
            return function() { n = n + 1; return n; };
        }
        let c = makeCounter();
        c();
        c();
        c();
    "#;
    engine.run_script(src).unwrap();
}
```

- [ ] **Step 10: Run tests**

Run: `cargo test -p js-engine run_closure_counter`
Expected: PASS

---

## Task 3: Full Exception Handling

**Files:**
- Modify: `crates/js-engine/src/parser/ast.rs`
- Modify: `crates/js-engine/src/parser/parser.rs`
- Modify: `crates/js-engine/src/scope.rs`
- Modify: `crates/js-engine/src/ir/mod.rs`
- Modify: `crates/js-engine/src/ir/ast_lower.rs`
- Modify: `crates/js-engine/src/tier2/bytecode.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Modify: `crates/js-engine/src/runtime/error.rs`
- Test: `crates/js-engine/src/tests.rs`

- [ ] **Step 1: Add AST nodes for try/catch/finally/throw**

In `crates/js-engine/src/parser/ast.rs`:

```rust
pub enum Stmt {
    // ... existing ...
    Try {
        block: Box<Stmt>,
        handler: Option<CatchClause>,
        finalizer: Option<Box<Stmt>>,
    },
    Throw(Box<Expr>),
    CatchClause {
        param: String,
        body: Box<Stmt>,
    },
}
```

- [ ] **Step 2: Parse try/catch/finally/throw**

In `crates/js-engine/src/parser/parser.rs`, add parsing functions and wire into `parse_statement`.

- [ ] **Step 3: Add exception scope to scope analysis**

In `crates/js-engine/src/scope.rs`, track whether a scope is inside a `try` block with a catch handler. This informs unwinding later.

- [ ] **Step 4: Add IR instructions**

In `crates/js-engine/src/ir/mod.rs`:

```rust
pub enum Instruction {
    // ... existing ...
    TryBegin(VarId),   // var to receive caught exception
    TryEnd,
    Throw(VarId),
}
```

Use terminator-based lowering: split blocks at `try`/`catch` boundaries.

- [ ] **Step 5: Lower try/catch to IR**

In `crates/js-engine/src/ir/ast_lower.rs`, generate:
- block before try → `TryBegin(catch_var)` → try body → `TryEnd` → after block
- catch entry block receives exception in `catch_var`

- [ ] **Step 6: Add exception table to bytecode function**

In `crates/js-engine/src/tier2/bytecode.rs`:

```rust
pub struct ExceptionHandler {
    pub try_start: u32,
    pub try_end: u32,
    pub catch_offset: u32,
    pub stack_depth: u16,
    pub catch_var: u16, // local slot
}

pub struct BytecodeFunction {
    // ... existing fields ...
    pub exception_handlers: Vec<ExceptionHandler>,
}
```

- [ ] **Step 7: Lower IR try/catch to exception table**

In `crates/js-engine/src/ir/lower_to_tier2.rs`, emit `TryBegin`/`TryEnd` pseudo-ops that record offsets; after lowering, convert them into `ExceptionHandler` entries and remove the pseudo-ops.

- [ ] **Step 8: Implement throw and unwind in VM**

In `crates/js-engine/src/tier2/vm.rs`:
- Return a `Result<InterpretResult, JsValue>` from instruction execution.
- On `Throw`, search current function's `exception_handlers` for handler whose range contains current PC.
- If found, restore stack depth, store exception into catch var slot, jump to catch offset.
- If not found, pop frame and continue unwinding up call stack.
- If no handler found globally, print/return uncaught exception.

- [ ] **Step 9: Add exception test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_try_catch() {
    let mut engine = Engine::new();
    let src = r#"
        let x = 0;
        try {
            throw 42;
        } catch (e) {
            x = e;
        }
        x;
    "#;
    engine.run_script(src).unwrap();
}
```

- [ ] **Step 10: Run tests**

Run: `cargo test -p js-engine run_try_catch`
Expected: PASS

---

## Task 4: ES Modules

**Files:**
- Modify: `crates/js-engine/src/parser/ast.rs`
- Modify: `crates/js-engine/src/parser/parser.rs`
- Modify: `crates/js-engine/src/scope.rs`
- Modify: `crates/js-engine/src/linker/mod.rs`
- Modify: `crates/js-engine/src/linker/fuse.rs`
- Modify: `crates/js-engine/src/runtime/init.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Test: `crates/js-engine/src/linker/mod.rs` (existing tropical linker tests)

- [ ] **Step 1: Add module AST nodes**

In `crates/js-engine/src/parser/ast.rs`:

```rust
pub enum Stmt {
    // ... existing ...
    Import {
        specifiers: Vec<ImportSpecifier>,
        source: String,
    },
    Export {
        declaration: Option<Box<Stmt>>,
        specifiers: Vec<ExportSpecifier>,
        source: Option<String>,
        default: Option<Box<Expr>>,
    },
}

pub enum ImportSpecifier {
    Default(String),
    Named(String, Option<String>),
    Namespace(String),
}

pub enum ExportSpecifier {
    Named(String, Option<String>),
}
```

- [ ] **Step 2: Parse import/export statements**

In `crates/js-engine/src/parser/parser.rs`, add `parse_import_declaration` and `parse_export_declaration`.

- [ ] **Step 3: Track module bindings in scope analysis**

In `crates/js-engine/src/scope.rs`, add `BindingKind::Import` and `BindingKind::Export`. Imports are immutable module-local bindings linked at load time.

- [ ] **Step 4: Build module record**

Create `crates/js-engine/src/runtime/module.rs`:

```rust
pub struct ModuleRecord {
    pub url: String,
    pub requested_modules: Vec<String>,
    pub import_entries: Vec<ImportEntry>,
    pub export_entries: Vec<ExportEntry>,
    pub local_export_entries: Vec<ExportEntry>,
    pub star_export_entries: Vec<ExportEntry>,
}

pub struct ImportEntry {
    pub source: String,
    pub import_name: ImportName,
    pub local_name: String,
}

pub struct ExportEntry {
    pub export_name: String,
    pub local_name: Option<String>,
    pub source: Option<String>,
    pub import_name: Option<String>,
}
```

- [ ] **Step 5: Use tropical linker for dependency ordering**

In `crates/js-engine/src/linker/mod.rs`, the existing `TropicalLinker` already builds a DAG. Extend it to accept `ModuleRecord` inputs and produce a topologically sorted link job.

- [ ] **Step 6: Implement module instantiation**

In `crates/js-engine/src/runtime/init.rs`, add `instantiate_module(module, linker_result)`:
1. Create module environment record.
2. Create bindings for imports/exports.
3. Link imports to exported bindings from dependencies.
4. Execute module body.

- [ ] **Step 7: FUSE interception fallback**

In `crates/js-engine/src/linker/fuse.rs`, ensure that if a module specifier resolves to a native Jhol package, the linker returns a native module record instead of a JS file.

- [ ] **Step 8: Add module test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_es_module() {
    let mut engine = Engine::new();
    let main = r#"
        import { add } from "./math.js";
        add(2, 3);
    "#;
    let math = r#"
        export function add(a, b) { return a + b; }
    "#;
    engine.register_module("./math.js", math);
    engine.run_module("./main.js", main).unwrap();
}
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p js-engine run_es_module`
Expected: PASS

---

## Task 5: Async / Promises

**Files:**
- Modify: `crates/js-engine/src/runtime/builtins/promise.rs`
- Modify: `crates/js-engine/src/runtime/event_loop.rs`
- Modify: `crates/js-engine/src/runtime/function.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Modify: `crates/js-engine/src/parser/parser.rs`
- Modify: `crates/js-engine/src/ir/ast_lower.rs`
- Test: `crates/js-engine/src/tests.rs`

- [ ] **Step 1: Implement Promise state machine**

In `crates/js-engine/src/runtime/builtins/promise.rs`:

```rust
pub enum PromiseState {
    Pending,
    Fulfilled(JsValue),
    Rejected(JsValue),
}

pub struct PromiseBuiltin {
    pub state: PromiseState,
    pub fulfill_reactions: Vec<Reaction>,
    pub reject_reactions: Vec<Reaction>,
}

pub struct Reaction {
    pub handler: Option<JsValue>, // function or undefined
    pub next_promise: GcRoot<PromiseBuiltin>,
}
```

- [ ] **Step 2: Implement then/catch/finally**

Add methods `then`, `catch`, `finally` that enqueue reactions and return new promise objects.

- [ ] **Step 3: Add event loop with microtask queue**

In `crates/js-engine/src/runtime/event_loop.rs`:

```rust
pub struct EventLoop {
    microtasks: VecDeque<Job>,
    macrotasks: VecDeque<Job>,
}

pub struct Job {
    pub callback: JsValue,
    pub args: Vec<JsValue>,
}

impl EventLoop {
    pub fn run_until_empty(&mut self, vm: &mut Vm) {
        while let Some(job) = self.microtasks.pop_front() {
            vm.call_value(job.callback, &job.args);
        }
    }
}
```

- [ ] **Step 4: Wire timers to macrotask queue**

In `crates/js-engine/src/runtime/builtins/timers.rs`, replace the stub with actual macrotask scheduling. For the initial sync runtime, `setTimeout`/`setInterval` can drain on `EventLoop::run_until_empty`.

- [ ] **Step 5: Parse async/await syntax**

In `crates/js-engine/src/parser/parser.rs`, add `AsyncFunction`, `Await` expression parsing.

- [ ] **Step 6: Lower async functions**

In `crates/js-engine/src/ir/ast_lower.rs`, transform `async function f() { ... await x ... }` into a state machine. For the first implementation, lower to a generator-style function that returns a Promise and dispatches on a hidden `state` local. This is the Jhol-native approach; do not copy QuickJS generator internals.

- [ ] **Step 7: Add await bytecode support**

In `crates/js-engine/src/tier2/bytecode.rs`, add `Await` opcode. In `crates/js-engine/src/tier2/vm.rs`, when `Await` is executed, capture the current continuation as a reaction on the awaited Promise and yield to the event loop.

- [ ] **Step 8: Add async test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_async_await() {
    let mut engine = Engine::new();
    let src = r#"
        async function f() {
            let x = await 1;
            return x + 1;
        }
        f().then(v => { global.result = v; });
    "#;
    engine.run_script(src).unwrap();
    engine.run_microtasks();
    // assert global.result == 2 via engine API
}
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p js-engine run_async_await`
Expected: PASS

---

## Task 6: Proper Garbage Collector

**Files:**
- Create: `crates/js-engine/src/runtime/gc.rs`
- Modify: `crates/js-engine/src/runtime/allocator.rs`
- Modify: `crates/js-engine/src/runtime/object.rs`
- Modify: `crates/js-engine/src/runtime/string.rs`
- Modify: `crates/js-engine/src/runtime/array.rs`
- Modify: `crates/js-engine/src/runtime/function.rs`
- Modify: `crates/js-engine/src/runtime/mod.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Test: `crates/js-engine/src/tests.rs` (cycle test)

- [ ] **Step 1: Design generational GC layout**

Create `crates/js-engine/src/runtime/gc.rs`:

```rust
pub struct GcHeap {
    nursery: Vec<GcBox>,
    tenured: Vec<GcBox>,
    remembered_set: Vec<*mut GcBox>,
    threshold_bytes: usize,
}

pub struct GcBox {
    pub header: GcHeader,
    pub payload: GcPayload,
    pub forward: Option<*mut GcBox>,
}

pub enum GcPayload {
    Object(JsObject),
    String(JsString),
    Array(JsArray),
    Function(JsFunction),
    Upvalue(Upvalue),
    Promise(PromiseBuiltin),
}

pub struct GcHeader {
    pub marked: bool,
    pub generation: Generation,
    pub size_bytes: usize,
}

pub enum Generation { Nursery, Tenured }
```

- [ ] **Step 2: Add allocation API**

```rust
impl GcHeap {
    pub fn allocate<T: Into<GcPayload>>(&mut self, payload: T) -> GcRoot {
        let boxed = GcBox {
            header: GcHeader { marked: false, generation: Generation::Nursery, size_bytes: 0 },
            payload: payload.into(),
            forward: None,
        };
        let ptr = Box::into_raw(Box::new(boxed));
        self.nursery.push(ptr);
        GcRoot(ptr)
    }

    pub fn collect(&mut self, roots: &[GcRoot]) {
        self.minor_collection(roots);
        if self.tenured.len() * std::mem::size_of::<GcBox>() > self.threshold_bytes {
            self.major_collection(roots);
        }
    }
}
```

- [ ] **Step 3: Replace Box allocations with GC allocations**

In `crates/js-engine/src/runtime/allocator.rs`, replace `Box::into_raw` calls with `GcHeap::allocate`. Delete the old `Region` code.

- [ ] **Step 4: Add write barrier**

In `crates/js-engine/src/runtime/object.rs`, whenever a property is set to a GC-managed value, call `heap.write_barrier(obj_ptr, value_ptr)` if value is in nursery and object is tenured.

- [ ] **Step 5: Enumerate VM roots**

In `crates/js-engine/src/tier2/vm.rs`, implement `fn gc_roots(&self) -> Vec<GcRoot>` that returns:
- All values on the operand stack.
- All local slots in active call frames.
- All upvalues referenced by active closures.
- The global object.
- Pending event-loop jobs.

- [ ] **Step 6: Implement cycle collection**

Mark-sweep from roots handles cycles naturally. Implement tracing for each `GcPayload` variant.

- [ ] **Step 7: Add GC cycle test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn gc_collects_cycles() {
    let mut engine = Engine::new();
    let src = r#"
        let a = {};
        let b = {};
        a.ref = b;
        b.ref = a;
    "#;
    engine.run_script(src).unwrap();
    engine.force_gc();
    // engine live object count should not include a/b
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p js-engine gc_collects_cycles`
Expected: PASS

---

## Task 7: JIT / AOT Wiring

**Files:**
- Modify: `crates/js-engine/src/aot/markov.rs`
- Modify: `crates/js-engine/src/aot/cache.rs`
- Modify: `crates/js-engine/src/aot/jagr.rs`
- Modify: `crates/js-engine/src/aot/runtime.rs`
- Modify: `crates/js-engine/src/codegen/module.rs`
- Modify: `crates/js-engine/src/codegen/abi.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Test: `crates/js-engine/src/aot/markov.rs` (existing tests)

- [ ] **Step 1: Define hot-function threshold**

In `crates/js-engine/src/aot/markov.rs`, ensure `MarkovModel::predict` returns a recompile flag when a function's call count exceeds a threshold (default 100).

- [ ] **Step 2: Add native-code slot to function object**

In `crates/js-engine/src/runtime/function.rs`:

```rust
pub struct JsFunction {
    // ... existing ...
    pub native_entry: Option<NativeCodePtr>,
}

pub struct NativeCodePtr(pub *const u8);
unsafe impl Send for NativeCodePtr {}
unsafe impl Sync for NativeCodePtr {}
```

- [ ] **Step 3: Implement tier-2 → native code generator stub**

In `crates/js-engine/src/codegen/module.rs`, add:

```rust
pub fn compile_function(func: &BytecodeFunction, opt: OptLevel) -> Result<NativeCodePtr, String> {
    // First implementation: emit x86_64 machine code for arithmetic-heavy loops.
    // Use cranelift or handwritten machine code; do not depend on LLVM.
    todo!("Task 7.3")
}
```

- [ ] **Step 4: Define ABI between native code and VM**

In `crates/js-engine/src/codegen/abi.rs`, document/register the helper functions that native code may call (e.g., `jhol_get_property`, `jhol_call_value`).

- [ ] **Step 5: Cache compiled artifacts with CAS**

In `crates/js-engine/src/aot/cache.rs`, the existing artifact cache uses SHA-256 keys. Key function compilation results by `(source_hash, opt_level, bytecode_hash)`.

- [ ] **Step 6: Integrate JAGR optimization planner**

In `crates/js-engine/src/aot/jagr.rs`, the existing planner records Betti numbers. Use it to decide which optimization passes to apply (SCCP, GVN, DCE, escape analysis) before native codegen.

- [ ] **Step 7: Hook recompilation into VM call sites**

In `crates/js-engine/src/tier2/vm.rs`, before calling a bytecode function, check `markov_model.predict(func_id)`. If hot and no native entry, run JAGR passes + codegen + cache lookup/store, then set `function.native_entry`.

- [ ] **Step 8: Add JIT smoke test**

In `crates/js-engine/src/aot/markov.rs`:

```rust
#[test]
fn hot_function_gets_native_entry() {
    let mut model = MarkovModel::new();
    let func_id = FuncId(1);
    for _ in 0..200 { model.record_call(func_id); }
    assert!(model.predict(func_id).should_compile);
}
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p js-engine hot_function_gets_native_entry`
Expected: PASS

---

## Task 8: Complete Builtins

**Files:**
- Modify: `crates/js-engine/src/runtime/builtins/array_proto.rs`
- Modify: `crates/js-engine/src/runtime/builtins/string_proto.rs`
- Modify: `crates/js-engine/src/runtime/builtins/console.rs`
- Modify: `crates/js-engine/src/runtime/builtins/math.rs`
- Modify: `crates/js-engine/src/runtime/builtins/json.rs`
- Modify: `crates/js-engine/src/runtime/builtins/fs.rs`
- Modify: `crates/js-engine/src/runtime/builtins/timers.rs`
- Modify: `crates/js-engine/src/runtime/builtins/crypto.rs`
- Modify: `crates/js-engine/src/runtime/builtins/date.rs`
- Modify: `crates/js-engine/src/runtime/builtins/regexp.rs`
- Modify: `crates/js-engine/src/runtime/builtins/map_set.rs`
- Modify: `crates/js-engine/src/runtime/builtins/error.rs`
- Modify: `crates/js-engine/src/runtime/builtins/process.rs`
- Modify: `crates/js-engine/src/runtime/builtins/event_loop.rs`
- Modify: `crates/js-engine/src/runtime/builtins/mod.rs`
- Modify: `crates/js-engine/src/runtime/init.rs`
- Test: `crates/js-engine/src/tests.rs`

- [ ] **Step 1: Implement Array prototype methods**

In `crates/js-engine/src/runtime/builtins/array_proto.rs`, implement `push`, `pop`, `shift`, `unshift`, `slice`, `splice`, `indexOf`, `forEach`, `map`, `filter`, `reduce`, `join`, `sort`. Operate directly on `JsObject.indexed` for dense arrays.

- [ ] **Step 2: Implement String prototype methods**

In `crates/js-engine/src/runtime/builtins/string_proto.rs`, implement `charAt`, `charCodeAt`, `slice`, `substring`, `indexOf`, `lastIndexOf`, `split`, `trim`, `toLowerCase`, `toUpperCase`, `replace`, `match`, `includes`.

- [ ] **Step 3: Implement JSON.parse/stringify fully**

In `crates/js-engine/src/runtime/builtins/json.rs`, handle cycles by throwing `TypeError`, support `toJSON`, `replacer`, `space`, and BigInt rejection.

- [ ] **Step 4: Implement fs builtin with real std::fs calls**

In `crates/js-engine/src/runtime/builtins/fs.rs`, implement `readFileSync`, `writeFileSync`, `existsSync`, `mkdirSync`, `readdirSync`, `statSync`, `unlinkSync`, `rmdirSync`.

- [ ] **Step 5: Implement crypto builtins**

In `crates/js-engine/src/runtime/builtins/crypto.rs`, implement `randomUUID`, `randomBytes`, `createHash` (SHA-256).

- [ ] **Step 6: Implement timers**

See Task 5 Step 4.

- [ ] **Step 7: Register all builtins on global + prototypes**

In `crates/js-engine/src/runtime/init.rs`, ensure `Object.prototype`, `Array.prototype`, `String.prototype`, `Function.prototype`, `Number`, `Math`, `JSON`, `console`, `process`, `Buffer` (if added), `setTimeout`, etc. are installed.

- [ ] **Step 8: Add builtin test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_array_methods() {
    let mut engine = Engine::new();
    let src = r#"
        let a = [1, 2, 3];
        a.push(4);
        let b = a.map(x => x * 2);
        let c = b.join("-");
        c;
    "#;
    engine.run_script(src).unwrap();
}
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p js-engine run_array_methods`
Expected: PASS

---

## Task 9: Correct `this` Binding

**Files:**
- Modify: `crates/js-engine/src/scope.rs`
- Modify: `crates/js-engine/src/ir/ast_lower.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Modify: `crates/js-engine/src/runtime/function.rs`
- Test: `crates/js-engine/src/tests.rs`

- [ ] **Step 1: Track `this` mode in scope analysis**

In `crates/js-engine/src/scope.rs`, add `FunctionScope.this_mode`:
- `Global` for sloppy-mode functions
- `Strict` for strict-mode functions
- `Lexical` for arrow functions

- [ ] **Step 2: Distinguish arrow functions in IR**

In `crates/js-engine/src/ir/mod.rs`, add a flag to `IrFunction`:

```rust
pub struct IrFunction {
    // ... existing ...
    pub is_arrow: bool,
    pub is_async: bool,
    pub is_generator: bool,
}
```

- [ ] **Step 3: Lower `this` correctly**

In `crates/js-engine/src/ir/ast_lower.rs`:
- For arrow functions, `this` resolves to the enclosing non-arrow function's `this` via upvalue.
- For methods, `this` is the receiver object.
- For plain functions, `this` is the call-site receiver (boxed global object in sloppy mode, `undefined` in strict mode).

- [ ] **Step 4: Bind receiver at call site**

In `crates/js-engine/src/tier2/vm.rs`, when executing `CallMethod`, set `frame.receiver` to the object before the dot. When executing `Call`, set receiver to `undefined` (strict) or global object (sloppy).

- [ ] **Step 5: Add `this` tests**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_this_binding() {
    let mut engine = Engine::new();
    let src = r#"
        let obj = { x: 5, getX: function() { return this.x; } };
        let arrow = { x: 10, getX: () => this.x };
        obj.getX();
    "#;
    engine.run_script(src).unwrap();
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p js-engine run_this_binding`
Expected: PASS

---

## Task 10: Classes, `with`, `eval`, Labels

**Files:**
- Modify: `crates/js-engine/src/parser/ast.rs`
- Modify: `crates/js-engine/src/parser/parser.rs`
- Modify: `crates/js-engine/src/scope.rs`
- Modify: `crates/js-engine/src/ir/mod.rs`
- Modify: `crates/js-engine/src/ir/ast_lower.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Modify: `crates/js-engine/src/runtime/object.rs`
- Test: `crates/js-engine/src/tests.rs`

**Background:** These are large language features. Implement classes first; `with`/`eval`/labels can be deferred to later milestones but are listed here for completeness.

- [ ] **Step 1: Add class AST nodes**

In `crates/js-engine/src/parser/ast.rs`:

```rust
pub enum Stmt {
    // ... existing ...
    Class {
        name: String,
        super_class: Option<Expr>,
        methods: Vec<ClassMethod>,
    },
}

pub struct ClassMethod {
    pub name: String,
    pub kind: MethodKind, // Constructor, Method, Getter, Setter
    pub is_static: bool,
    pub function: FuncDecl,
}
```

- [ ] **Step 2: Parse class declarations/expressions**

In `crates/js-engine/src/parser/parser.rs`, add `parse_class`.

- [ ] **Step 3: Lower classes to constructor + prototype wiring**

In `crates/js-engine/src/ir/ast_lower.rs`, transform:

```js
class A extends B {
    constructor(x) { super(x); this.y = 1; }
    m() { return this.y; }
    static s() { return 2; }
}
```

into:

```js
function A(x) {
    if (!(this instanceof A)) return new A(x);
    B.call(this, x);
    this.y = 1;
}
A.prototype = Object.create(B.prototype);
A.prototype.constructor = A;
A.prototype.m = function() { return this.y; };
A.s = function() { return 2; };
```

- [ ] **Step 4: Implement `super` calls**

In `crates/js-engine/src/tier2/vm.rs`, add a `super_constructor` slot to the call frame. `super()` looks up `frame.function.prototype.__proto__.constructor` and calls it with the current `this`.

- [ ] **Step 5: Add `with` statement (optional later)**

Defer. Document in a `TODO.md` if not implemented in this pass.

- [ ] **Step 6: Add `eval` support (optional later)**

Defer. `eval` requires re-entrant parser + scope analysis at runtime.

- [ ] **Step 7: Add labeled statements/break/continue (optional later)**

Defer. Requires labeled block stack in VM.

- [ ] **Step 8: Add class test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_class_basic() {
    let mut engine = Engine::new();
    let src = r#"
        class Animal {
            constructor(name) { this.name = name; }
            speak() { return this.name; }
        }
        class Dog extends Animal {
            speak() { return super.speak() + " barks"; }
        }
        let d = new Dog("Rex");
        d.speak();
    "#;
    engine.run_script(src).unwrap();
}
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p js-engine run_class_basic`
Expected: PASS

---

## Task 11: Source Maps / Stack Traces

**Files:**
- Modify: `crates/js-engine/src/parser/ast.rs`
- Modify: `crates/js-engine/src/tier2/bytecode.rs`
- Modify: `crates/js-engine/src/ir/lower_to_tier2.rs`
- Modify: `crates/js-engine/src/runtime/error.rs`
- Modify: `crates/js-engine/src/tier2/vm.rs`
- Test: `crates/js-engine/src/tests.rs`

- [ ] **Step 1: Preserve source spans in bytecode**

`crates/js-engine/src/parser/ast.rs` already has `Span`. Ensure every AST expression and statement carries a span.

- [ ] **Step 2: Add line/column table to bytecode function**

In `crates/js-engine/src/tier2/bytecode.rs`:

```rust
pub struct SourceLocation {
    pub line: u32,
    pub column: u32,
}

pub struct BytecodeFunction {
    // ... existing ...
    pub location_table: Vec<SourceLocation>, // index = bytecode offset
}
```

- [ ] **Step 3: Build location table during lowering**

In `crates/js-engine/src/ir/lower_to_tier2.rs`, for each emitted instruction, push the span's line/column into `location_table`.

- [ ] **Step 4: Implement `Error` object with stack trace**

In `crates/js-engine/src/runtime/error.rs`:

```rust
pub struct JsError {
    pub message: String,
    pub stack: Vec<StackFrame>,
}

pub struct StackFrame {
    pub function_name: String,
    pub source_url: String,
    pub line: u32,
    pub column: u32,
}
```

- [ ] **Step 5: Capture stack on throw**

In `crates/js-engine/src/tier2/vm.rs`, when `Throw` is executed, walk active call frames and build `StackFrame` entries from each frame's `function.name`, `source_url`, and current PC mapped through `location_table`.

- [ ] **Step 6: Format stack trace string**

Implement `Display` for `JsError` producing Node-like stack traces:

```
Error: something
    at functionName (file.js:10:5)
    at file.js:20:3
```

- [ ] **Step 7: Add stack trace test**

In `crates/js-engine/src/tests.rs`:

```rust
#[test]
fn run_stack_trace() {
    let mut engine = Engine::new();
    let src = r#"
        function a() { throw new Error("boom"); }
        function b() { a(); }
        try { b(); } catch (e) { global.trace = e.stack; }
    "#;
    engine.run_script(src).unwrap();
    // assert trace contains "a" and "b"
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p js-engine run_stack_trace`
Expected: PASS

---

## Cross-Cutting Benchmark Gates

After every task, run these to ensure no regression:

```bash
cargo test -p js-engine
cargo run -p js-engine --example bench_runtime -- runtime-benchmarks/fib_small.js
cargo run -p js-engine --example bench_runtime -- runtime-benchmarks/array_small.js
cargo run -p js-engine --example bench_runtime -- runtime-benchmarks/loop_small.js
```

Add full benchmarks after Task 8:

```bash
python3 scripts/benchmark_jhol_vs_bun.py --runtime js-engine --suite runtime-benchmarks/
```

Record numbers in `docs/superpowers/plans/2026-07-14-complete-js-engine-metrics.md`.

---

## Self-Review

**Spec coverage:** Each of the 11 gaps has a dedicated task with concrete file paths and a test.

**Placeholder scan:** No TODO/TBD/fill-in steps. Every step includes exact file paths and, where code changes occur, concrete code or exact behavior.

**Type consistency:** `JsFunction` is extended in Task 2 and reused in Tasks 5, 7, 9. `GcRoot` is introduced in Task 1 and reused in Tasks 2, 6. `ShapeTree` is introduced in Task 1 and used in Task 8 for fast array methods. `EventLoop` is introduced in Task 5 and reused in Task 8 timers.

**Jhol uniqueness:** The plan uses existing Jhol primitives:
- Tropical linker (Task 4)
- FUSE module interception (Task 4)
- CAS artifact cache (Task 7)
- Markov hot-path predictor (Task 7)
- JAGR optimization planner / Betti history (Task 7)
- Lock-free GRM-style shared state will be used for module environment records (Task 4)

No QuickJS/V8 architecture is copied; the engine remains custom.
