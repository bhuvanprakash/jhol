//! CommonJS module loading for JholVM: `require`.
//!
//! Node's CJS resolution subset, implemented directly:
//!
//! * `./x`, `../x` — resolved against the *requiring module's* directory
//!   (tracked on a stack, so nested requires resolve relative to their own
//!   file, not the entry script).
//! * bare `x` — walk up from that directory through `node_modules/`.
//! * file targets try the exact path, then `.js`, then `.json`; directories
//!   try `package.json#main`, then `index.js` / `index.json`.
//! * Modules execute inside a `(module, exports, require, __filename,
//!   __dirname)` wrapper — the same shape node wraps CJS in — so top-level
//!   `return`, `this === exports`, and `module.exports = ...` all behave.
//! * The cache stores the **module object** before execution (cycles see the
//!   partial exports, as in node) and `require` reads `.exports` afterwards,
//!   so `module.exports = X` rebinding is observed.
//!
//! The cache lives on the globals object under a `#`-prefixed key: globals
//! are a GC root, so cached modules are traced without touching the
//! collector, and `#`-keys are filtered from every user-visible key list.

use crate::errors::{type_error, VmResult};
use crate::objects::PropKey;
use crate::value::Value;
use crate::vm::Vm;
use std::path::{Path, PathBuf};

const MODULE_CACHE_KEY: &str = "#jhol_module_cache";

// ---------------------------------------------------------------------------
// resolution
// ---------------------------------------------------------------------------

/// A concrete file for `p`: the path itself, `p.js`, or `p.json`.
fn existing_file(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    for ext in [".js", ".json"] {
        let mut os = p.as_os_str().to_os_string();
        os.push(ext);
        let q = PathBuf::from(os);
        if q.is_file() {
            return Some(q);
        }
    }
    None
}

/// Read `package.json#main` through the engine's own JSON parser (no
/// second JSON implementation in the engine), then normalise it against the
/// directory.
fn package_main(vm: &mut Vm, d: &Path) -> Option<String> {
    let pj = d.join("package.json");
    if !pj.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&pj).ok()?;
    let sv = vm.new_string(&text);
    let parsed = crate::builtins::n_json_parse(vm, Value::UNDEFINED, &[sv]).ok()?;
    let main_key = PropKey::Sym(vm.arena().intern_sym("main"));
    let main = vm.get_prop(parsed, main_key).ok()?;
    if main.is_string() {
        Some(unsafe { main.as_string().as_str() }.to_string())
    } else {
        None
    }
}

/// A directory target: `package.json#main` (file or directory), then
/// `index.js`, then `index.json`.
fn dir_target(vm: &mut Vm, d: &Path) -> Option<PathBuf> {
    if let Some(main) = package_main(vm, d) {
        let m = d.join(main);
        if let Some(f) = existing_file(&m) {
            return Some(f);
        }
        if m.is_dir() {
            if let Some(f) = existing_file(&m.join("index")) {
                return Some(f);
            }
        }
    }
    existing_file(&d.join("index"))
}

fn target(vm: &mut Vm, spec_path: &Path) -> Option<PathBuf> {
    if let Some(f) = existing_file(spec_path) {
        return Some(f);
    }
    if spec_path.is_dir() {
        return dir_target(vm, spec_path);
    }
    None
}

/// Resolve `spec` from `dir`. The error string matches node's shape so
/// `catch (e)` output looks familiar: `Cannot find module 'x'`.
pub fn resolve(vm: &mut Vm, spec: &str, dir: &Path) -> Result<PathBuf, String> {
    let not_found = || format!("Cannot find module '{}' from '{}'", spec, dir.display());

    if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
        return target(vm, &dir.join(spec)).ok_or_else(not_found);
    }
    if spec.starts_with('/') {
        return target(vm, Path::new(spec)).ok_or_else(not_found);
    }

    // Bare specifier: walk up through node_modules, node-style.
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let cand = d.join("node_modules").join(spec);
        if cand.exists() {
            if let Some(f) = target(vm, &cand) {
                return Ok(f);
            }
        }
        cur = d.parent();
    }
    Err(format!("Cannot find module '{}'", spec))
}

// ---------------------------------------------------------------------------
// the require native
// ---------------------------------------------------------------------------

fn intern(vm: &mut Vm, s: &str) -> PropKey {
    PropKey::Sym(vm.arena().intern_sym(s))
}

/// The module cache: a plain object hanging off globals (a GC root).
fn module_cache(vm: &mut Vm) -> Value {
    let g = Value::from_ptr(vm.globals);
    let k = intern(vm, MODULE_CACHE_KEY);
    if let Some(v) = unsafe { g.as_object() }.get(k) {
        return v;
    }
    let obj = vm.new_object();
    vm.define_prop(g, k, obj);
    obj
}

/// The directory the currently-executing module lives in (entry script or
/// the module nearest the top of the require stack).
pub fn current_dir(vm: &Vm) -> PathBuf {
    vm.module_dirs
        .last()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Build the wrapper function for a module body without running it.
/// (`run_module` would clear the live stack — require must nest.)
fn make_module_fn(vm: &mut Vm, src: &str, name: &str) -> VmResult<Value> {
    // The wrapper is our own source-level module system: node wraps CJS the
    // same way, and it is what makes top-level `return` and
    // `this === exports` behave.
    let wrapped = format!(
        "(function(module, exports, require, __filename, __dirname) {{\n{}\n}})",
        src
    );
    let prog = crate::parser::parse_program(&wrapped)
        .map_err(|m| crate::errors::VmError::Syntax(format!("{} ({})", m, name)))?;

    // CRITICAL: the wrapped source is a *program* whose single statement is a
    // function expression. Compiling the program and calling its entry would
    // evaluate the expression and throw the closure away — the module body
    // would never run (observed: `exports.hello` stayed undefined). Extract
    // the function-expression node and compile THAT as the module function,
    // so the entry proto's params are (module, exports, require, ...) and the
    // body is the module source itself.
    let func_node = match prog.body.last() {
        Some(crate::ast::Stmt::Expr(crate::ast::Expr::Func(f))) => (**f).clone(),
        _ => {
            return Err(crate::errors::VmError::Syntax(format!(
                "{}: module wrapper did not parse to a function",
                name
            )))
        }
    };
    let module_func = crate::ast::Func {
        name: Some(name.to_string()),
        ..func_node
    };
    let compiled = crate::compiler::compile(&module_func, name)
        .map_err(|m| crate::errors::VmError::Syntax(format!("{} ({})", m, name)))?;

    // Append the protos to the live table (frames hold `Arc::as_ptr` into
    // stable Arc allocations, so growth is safe) and materialise the entry as
    // an ordinary callable — no stack clearing, unlike `run_module`.
    let module_base = vm.protos.len();
    if std::env::var_os("JHOL_LIFE_DEBUG").is_some() {
        let proto = compiled.protos.get(compiled.entry as usize);
        if let Some(pr) = proto {
            eprintln!("modfn {}: nslots={} nparams={} code:", name, pr.nslots, pr.nparams);
            for (i, ins) in pr.code.iter().enumerate() {
                eprintln!("  {:3} {:?} a={} b={} c={}", i, ins.op, ins.a, ins.b, ins.c);
            }
        }
    }
    vm.protos.extend(compiled.protos);
    let proto = vm.protos[module_base + compiled.entry as usize].clone();
    let f = crate::objects::JFunction::bytecode(Value::UNDEFINED, proto.clone(), Value::UNDEFINED);
    let fp = vm.arena().alloc_function(f);
    let v = Value::from_ptr(fp);
    unsafe {
        (*fp).proto = Value::from_ptr(vm.proto_function_ptr() as *mut crate::objects::JFunction);
        (*fp).hdr.flags |= crate::objects::FLAG_CTOR;
        (*fp).module_base = module_base;
        (*fp).upvalues = Vec::new();
    }
    Ok(v)
}

/// `require(spec)` — the CJS entry point. See the module docs.
pub fn n_require(vm: &mut Vm, _this: Value, args: &[Value]) -> VmResult<Value> {
    let spec_v = args.first().copied().unwrap_or(Value::UNDEFINED);
    if !spec_v.is_string() {
        return Err(type_error("require expects a string specifier"));
    }
    let spec = unsafe { spec_v.as_string().as_str() }.to_string();

    let from = current_dir(vm);
    let resolved = resolve(vm, &spec, &from).map_err(|m| type_error(&m))?;
    let key_str = resolved.display().to_string();

    // Cache: the module OBJECT, inserted before execution so cycles observe
    // partial exports exactly as node does.
    let cache = module_cache(vm);
    let ck = intern(vm, &key_str);
    if let Some(cached) = unsafe { cache.as_object() }.get(ck) {
        return Ok(unsafe { cached.as_object() }
            .get(intern(vm, "exports"))
            .unwrap_or(Value::UNDEFINED));
    }

    let module_obj = vm.new_object();
    let exports_obj = vm.new_object();
    let exports_key = intern(vm, "exports");
    vm.define_prop(module_obj, exports_key, exports_obj);
    vm.define_prop(cache, ck, module_obj);

    let filename = key_str.clone();
    let dirname = resolved
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/".into());

    // JSON modules: the whole file is the exports; no wrapper, no execution.
    let is_json = resolved.extension().and_then(|e| e.to_str()) == Some("json");
    if is_json {
        let text = std::fs::read_to_string(&resolved)
            .map_err(|e| type_error(&format!("Cannot read {}: {}", filename, e)))?;
        let sv = vm.new_string(&text);
        let parsed = crate::builtins::n_json_parse(vm, Value::UNDEFINED, &[sv])?;
        let exports_key = intern(vm, "exports");
        vm.define_prop(module_obj, exports_key, parsed);
        return Ok(parsed);
    }

    let src = std::fs::read_to_string(&resolved)
        .map_err(|e| type_error(&format!("Cannot read {}: {}", filename, e)))?;
    let fnv = make_module_fn(vm, &src, &filename)?;

    let filename_v = vm.new_string(&filename);
    let dirname_v = vm.new_string(&dirname);
    // The require global is the same native for every module; the *directory
    // stack* gives it per-module resolution.
    let require_key = intern(vm, "require");
    let globals_v = Value::from_ptr(vm.globals);
    let require_v = unsafe { globals_v.as_object() }
        .get(require_key)
        .unwrap_or(Value::UNDEFINED);

    // node calls the wrapper with `this === module.exports`.
    let call_args = [module_obj, exports_obj, require_v, filename_v, dirname_v];

    let parent_dir = resolved
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/".into());
    vm.module_dirs.push(parent_dir);
    let result = vm.call_direct_args(fnv, exports_obj, &call_args);
    vm.module_dirs.pop();
    result?;

    let exports_key = intern(vm, "exports");
    Ok(unsafe { module_obj.as_object() }
        .get(exports_key)
        .unwrap_or(Value::UNDEFINED))
}
