//! Optional JavaScriptCore backend for jhol-runtime.
//! macOS/Linux only. Uses JSC's built-in JIT/tiered compiler.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

use javascriptcore::sys::{
    JSContextRef, JSObjectRef, JSValueRef, OpaqueJSValue,
};
use javascriptcore::{evaluate_script, sys, JSContext, JSValue};

use crate::errors::{RuntimeError, RuntimeResult};
use crate::transpile;

static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

unsafe fn arg_string(
    ctx: JSContextRef,
    argument_count: usize,
    arguments: *const JSValueRef,
) -> Option<String> {
    if argument_count == 0 || arguments.is_null() {
        return None;
    }
    let val = JSValue::from_raw(ctx, *arguments);
    val.as_string().ok().map(|s| s.to_string())
}

unsafe extern "C" fn stdout_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    if let Some(s) = arg_string(ctx, argument_count, arguments) {
        print!("{}", s);
    }
    sys::JSValueMakeUndefined(ctx)
}

unsafe extern "C" fn stderr_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    if let Some(s) = arg_string(ctx, argument_count, arguments) {
        eprint!("{}", s);
    }
    sys::JSValueMakeUndefined(ctx)
}

unsafe extern "C" fn exit_code_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    if argument_count > 0 && !arguments.is_null() {
        let val = JSValue::from_raw(ctx, *arguments);
        if let Ok(n) = val.as_number() {
            EXIT_CODE.store(n as i32, Ordering::Relaxed);
        }
    }
    sys::JSValueMakeUndefined(ctx)
}

fn register_callback(ctx: &JSContext, name: &str, cb: javascriptcore::sys::JSObjectCallAsFunctionCallback) {
    let global = ctx.global_object().expect("global object");
    let func = JSValue::new_function(ctx, name, cb);
    let _ = global.set_property(name, func);
}

pub struct JscRuntime {
    context: JSContext,
    working_dir: PathBuf,
}

impl JscRuntime {
    pub fn new() -> RuntimeResult<Self> {
        let context = JSContext::new();
        let cwd = std::env::current_dir().map_err(RuntimeError::IoError)?;
        let mut rt = Self { context, working_dir: cwd };
        rt.bootstrap()?;
        Ok(rt)
    }

    fn bootstrap(&mut self) -> RuntimeResult<()> {
        register_callback(&self.context, "__jhol_stdout_write", Some(stdout_cb));
        register_callback(&self.context, "__jhol_stderr_write", Some(stderr_cb));
        register_callback(&self.context, "__jhol_set_exit_code", Some(exit_code_cb));

        let bootstrap = r#"
globalThis.global = globalThis;
if (!globalThis.console) globalThis.console = {};
globalThis.console.log = function() { __jhol_stdout_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
globalThis.console.warn = function() { __jhol_stderr_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
globalThis.console.error = function() { __jhol_stderr_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
globalThis.console.info = function() { __jhol_stdout_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
if (!globalThis.process) globalThis.process = {};
globalThis.process.exit = function(c) { __jhol_set_exit_code(c || 0); };
"#;
        evaluate_script(&self.context, bootstrap, None, "<bootstrap>", 1)
            .map_err(|e| RuntimeError::Internal(format!("JSC bootstrap failed: {:?}", e)))?;
        Ok(())
    }

    pub fn execute_file(&mut self, path: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|_| RuntimeError::FileNotFound(path.to_path_buf()))?;
        let source = std::fs::read_to_string(&canonical).map_err(RuntimeError::IoError)?;
        let filename = canonical.to_str().unwrap_or("unknown").to_string();
        self.working_dir = canonical.parent().map(|p| p.to_path_buf()).unwrap_or_default();

        let js_source = if transpile::needs_transpile(&canonical) {
            transpile::transpile(&source, &filename)?
        } else {
            source
        };
        self.evaluate(&js_source, &filename)
    }

    pub fn execute_code(&mut self, code: &str, filename: &str) -> RuntimeResult<()> {
        let js_source = if filename.ends_with(".ts")
            || filename.ends_with(".tsx")
            || filename.ends_with(".jsx")
        {
            transpile::transpile(code, filename)?
        } else {
            code.to_string()
        };
        self.evaluate(&js_source, filename)
    }

    fn evaluate(&mut self, code: &str, filename: &str) -> RuntimeResult<()> {
        let _dirname = self.working_dir.to_string_lossy().to_string();
        let wrapped = format!(
            r#"var module = {{ exports: {{}} }};
var exports = module.exports;
var __filename = "{}";
var __dirname = "{}";
{}
"#,
            filename, _dirname, code
        );
        evaluate_script(&self.context, wrapped, None, filename, 1)
            .map_err(|e| RuntimeError::ExecutionError(format!("{}:{:?}", filename, e)))?;
        Ok(())
    }

    pub fn run_event_loop(&mut self) -> RuntimeResult<i32> {
        Ok(EXIT_CODE.load(Ordering::Relaxed))
    }

    pub fn exit_code(&self) -> i32 {
        EXIT_CODE.load(Ordering::Relaxed)
    }
}

pub fn run_file(path: &Path) -> RuntimeResult<i32> {
    let mut rt = JscRuntime::new()?;
    rt.execute_file(path)?;
    rt.run_event_loop()
}

pub fn eval_code(code: &str, filename: Option<&str>) -> RuntimeResult<i32> {
    let mut rt = JscRuntime::new()?;
    rt.execute_code(code, filename.unwrap_or("<eval>"))?;
    rt.run_event_loop()
}
