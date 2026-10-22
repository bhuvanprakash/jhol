//! Optional JavaScriptCore backend for jhol-runtime.
//! macOS/Linux only. Uses JSC's built-in JIT/tiered compiler.
//!
//! Optimizations applied:
//!   * Thread-local pool of pre-bootstrapped JSContext objects so that repeated
//!     `run_file` / `eval_code` calls do not pay the cost of context creation
//!     and bootstrap registration on every invocation.
//!   * Global object reset between checkouts to preserve isolation while still
//!     keeping the compiled bootstrap, callbacks and JIT warm code in memory.
//!   * Exit-code is reset per checkout and read at the end of each run.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime};

use dashmap::DashMap;

use javascriptcore::sys::{JSContextRef, JSObjectRef, JSValueRef, OpaqueJSValue};
use javascriptcore::{evaluate_script, sys, JSContext, JSValue};

use crate::errors::{RuntimeError, RuntimeResult};
use crate::timer::{self, TimerState};
use crate::transpile;

fn file_cache() -> &'static DashMap<PathBuf, (SystemTime, Arc<str>)> {
    static CACHE: OnceLock<DashMap<PathBuf, (SystemTime, Arc<str>)>> = OnceLock::new();
    CACHE.get_or_init(DashMap::new)
}

/// Exit code of the most recently checked-out JSC runtime.
/// Reset to 0 on every checkout so that pooled contexts do not leak the
/// previous invocation's exit status.
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

const BOOTSTRAP: &str = r#"
globalThis.global = globalThis;
if (!globalThis.console) globalThis.console = {};
globalThis.console.log = function() { __jhol_stdout_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
globalThis.console.warn = function() { __jhol_stderr_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
globalThis.console.error = function() { __jhol_stderr_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
globalThis.console.info = function() { __jhol_stdout_write(Array.prototype.join.call(arguments, ' ') + '\n'); };
if (!globalThis.process) globalThis.process = {};
globalThis.process.exit = function(c) { __jhol_set_exit_code(c || 0); };
globalThis.process.hrtime = function() { return __jhol_hrtime(); };
globalThis.process.hrtime.bigint = function() { return BigInt(__jhol_hrtime_ns()); };
if (!globalThis.require) globalThis.require = function(n){ throw new Error('require not implemented: ' + n); };
if (!globalThis.__jhol_jsc_module_cache) globalThis.__jhol_jsc_module_cache = {};
globalThis._jhol_timer_id = 0;
globalThis._jhol_timers = {};
globalThis._jhol_intervals = {};
globalThis.setTimeout = function(fn, ms) {
    var id = ++globalThis._jhol_timer_id;
    globalThis._jhol_timers[id] = fn;
    __jhol_set_timeout('var __f = globalThis._jhol_timers[' + id + ']; if(__f){delete globalThis._jhol_timers[' + id + ']; __f.call(globalThis);}', ms || 0);
    return id;
};
globalThis.clearTimeout = function(id) { delete globalThis._jhol_timers[id]; __jhol_cancel_timer(id); };
globalThis.setInterval = function(fn, ms) {
    var id = ++globalThis._jhol_timer_id;
    globalThis._jhol_intervals[id] = fn;
    __jhol_set_interval('var __f = globalThis._jhol_intervals[' + id + ']; if(__f){__f.call(globalThis);}', ms || 0);
    return id;
};
globalThis.clearInterval = function(id) { delete globalThis._jhol_intervals[id]; __jhol_cancel_timer(id); };
globalThis.setImmediate = function(fn) {
    var id = ++globalThis._jhol_timer_id;
    globalThis._jhol_timers[id] = function() { fn.apply(null, Array.prototype.slice.call(arguments, 1)); };
    __jhol_set_timeout('var __f = globalThis._jhol_timers[' + id + ']; if(__f){delete globalThis._jhol_timers[' + id + ']; __f.call(globalThis);}', 0);
    return id;
};
globalThis.clearImmediate = function(id) { delete globalThis._jhol_timers[id]; __jhol_cancel_timer(id); };
// Native direct bindings for hot primitives.
// Buffer polyfill uses the same utf8 encoder as the QuickJS backend.
(function(){
function utf8_encode(s){var o=[];for(var i=0;i<s.length;i++){var c=s.charCodeAt(i);if(c<128)o.push(c);else if(c<2048){o.push(192|c>>6);o.push(128|c&63)}else{o.push(224|c>>12);o.push(128|c>>6&63);o.push(128|c&63)}}return new Uint8Array(o)}
function utf8_decode(b,s,e){s=s||0;e=e||b.length;var o='';for(var i=s;i<e;){var c=b[i];if(c<128){o+=String.fromCharCode(c);i++}else if(c<224){o+=String.fromCharCode((c&31)<<6|b[i+1]&63);i+=2}else{o+=String.fromCharCode((c&15)<<12|(b[i+1]&63)<<6|b[i+2]&63);i+=3}}return o}
function Buf(input,enc){if(typeof input==='string'){input=utf8_encode(input)}this._bf=input||new Uint8Array(0)}
Buf.prototype=Object.create(Uint8Array.prototype);Buf.prototype.constructor=Buf;
Buf.prototype.toString=function(enc,s,e){return utf8_decode(this._bf||this,s,e)};
Buf.from=function(i,e){return new Buf(i,e||'utf-8')};Buf.alloc=function(s){return new Buf(new Uint8Array(s))};
Buf.isBuffer=function(o){return o instanceof Buf||o instanceof Uint8Array};
globalThis.Buffer=Buf;})();
globalThis._jhol_fs = {
    readFileSync: function(p, enc) { return __jhol_fs_read(p || '', enc || 'utf8'); },
    existsSync: function(p) { return __jhol_fs_exists(p || ''); },
    writeFileSync: function(p, data) { return __jhol_fs_write(p || '', data || ''); },
    mkdirSync: function(p) { return __jhol_fs_mkdir(p || ''); },
    statSync: function(p) { var s = __jhol_fs_stat(p || ''); if(!s) return null; var a = s.split(';'); return { isDirectory: function(){return a[0]==='dir';}, size: parseInt(a[1]||'0',10) }; },
    readdirSync: function(p) { return JSON.parse(__jhol_fs_readdir(p || '') || '[]'); }
};
globalThis._jhol_crypto = {
    sha256: function(data) { return __jhol_sha256(data || ''); }
};
// rt-parity globals (ported from the QuickJS backend). Grown-ups'
// caution: JSC's globalThis may expose native non-configurable `crypto`/`fetch`
// accessors — AUGMENT existing objects, never assign over them (assignment
// throws in the bootstrap and kills every run).
(function(){
    if (typeof globalThis.fetch !== 'function') {
        globalThis.fetch = function(url, opts) { return Promise.resolve(JSON.parse(__jhol_fetch(url || '', JSON.stringify(opts || {})))); };
    }
    if (!globalThis.child_process) globalThis.child_process = {};
    globalThis.child_process.spawnSync = function(cmd, args) { return JSON.parse(__jhol_spawn(cmd || '', JSON.stringify(args || [])) || '{}'); };
    // rt-parity: builtin module names resolve to the engine's shims; anything
    // else keeps the original behavior.
    (function(){
        var __orig = globalThis.require;
        globalThis.require = function(n) {
            if (n === 'child_process') return globalThis.child_process;
            if (n === 'crypto') return globalThis.crypto;
            if (n === 'os') return { platform: globalThis.process.platform, arch: globalThis.process.arch };
            if (n === 'fs') return globalThis._jhol_fs;
            if (typeof __orig === 'function') return __orig.apply(null, arguments);
            throw new Error('require not implemented: ' + n);
        };
    })();
    globalThis.process.chdir = function(d) { return __jhol_chdir(d || '.'); };
    globalThis.process.env = (function() {
        var ks = JSON.parse(__jhol_env_keys('') || '[]'); var e = {};
        for (var i = 0; i < ks.length; i++) { (function(k){ Object.defineProperty(e, k, { configurable: true, enumerable: true, get: function(){ return __jhol_env_get(k); } }); })(ks[i]); }
        e.KEYS = ks;
        return e;
    })();
    globalThis.process.platform = (function(){ var o = JSON.parse(__jhol_os_info('') || '{}'); return o.platform || ''; })();
    globalThis.process.arch = (function(){ var o = JSON.parse(__jhol_os_info('') || '{}'); return o.arch || ''; })();
    var c = globalThis.crypto;
    if (!c) { try { c = {}; globalThis.crypto = c; } catch (e) { c = globalThis.crypto; } }
    if (c) {
        c.randomBytes = function(n) { return __jhol_random_bytes(n || 0); };
        c.randomUUID = function() { return __jhol_random_uuid(''); };
        c.createHash = function(alg) { return { update: function(d){ this._d = (this._d||'') + (d||''); return this; }, digest: function(enc){ return __jhol_hash(this._d || ''); } }; };
    }
})();
"#;

/// Maximum number of bootstrapped contexts kept in the thread-local pool.
const POOL_CAPACITY: usize = 4;

thread_local! {
    /// Pool of pre-bootstrapped JSC contexts.  JavaScriptCore contexts are
    /// single-threaded, so the pool is scoped to the current thread.
    static CONTEXT_POOL: RefCell<Vec<JSContext>> = RefCell::new(Vec::with_capacity(POOL_CAPACITY));

    /// Names present on the global object immediately after bootstrap.  Used
    /// to reset a pooled context back to a clean state before the next run.
    static ALLOWED_GLOBALS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

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

unsafe fn arg_number(
    ctx: JSContextRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    idx: usize,
) -> Option<f64> {
    if idx >= argument_count || arguments.is_null() {
        return None;
    }
    let val = JSValue::from_raw(ctx, *arguments.add(idx));
    val.as_number().ok()
}

unsafe fn js_string(ctx: JSContextRef, s: &str) -> *const OpaqueJSValue {
    let cstring = std::ffi::CString::new(s).unwrap_or_default();
    let js_s = sys::JSStringCreateWithUTF8CString(cstring.as_ptr());
    let val = sys::JSValueMakeString(ctx, js_s);
    sys::JSStringRelease(js_s);
    val
}

unsafe fn js_number(ctx: JSContextRef, n: f64) -> *const OpaqueJSValue {
    sys::JSValueMakeNumber(ctx, n)
}

unsafe fn js_boolean(ctx: JSContextRef, b: bool) -> *const OpaqueJSValue {
    sys::JSValueMakeBoolean(ctx, b)
}

unsafe fn js_from_json(ctx: JSContextRef, s: &str) -> *const OpaqueJSValue {
    let cstring = std::ffi::CString::new(s).unwrap_or_default();
    let js_s = sys::JSStringCreateWithUTF8CString(cstring.as_ptr());
    let val = sys::JSValueMakeFromJSONString(ctx, js_s);
    sys::JSStringRelease(js_s);
    val
}

unsafe extern "C" fn os_info_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let arch = std::env::consts::ARCH;
    let platform = std::env::consts::OS;
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).to_string();
    let homedir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    js_string(ctx, &format!(
        "{{\"arch\":\"{}\",\"platform\":\"{}\",\"cpus\":{},\"homedir\":\"{}\",\"tmpdir\":\"{}\"}}",
        arch, platform, cpus, homedir, std::env::temp_dir().display()))
}

unsafe extern "C" fn env_get_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let key = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_string(ctx, &crate::engine::env_get_impl(&key).unwrap_or_default())
}

unsafe extern "C" fn http_listen_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, &crate::engine::http_listen_impl())
}

unsafe extern "C" fn ws_connect_cb2(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let url = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_string(ctx, &crate::engine::ws_connect_impl(&url))
}

unsafe extern "C" fn db_open_cb2(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_string(ctx, &crate::engine::db_open_impl(&path))
}

unsafe extern "C" fn db_exec_cb2(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let sql = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_string(ctx, &crate::engine::db_exec_impl(0, &sql))
}

unsafe extern "C" fn db_close_cb2(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, &crate::engine::db_close_impl(0))
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

unsafe extern "C" fn set_timeout_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let callback = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let ms = arg_number(ctx, argument_count, arguments, 1)
        .unwrap_or(0.0)
        .max(0.0) as u32;
    let id = timer::set_timeout(callback, ms);
    js_number(ctx, id as f64)
}

unsafe extern "C" fn set_interval_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let callback = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let ms = arg_number(ctx, argument_count, arguments, 1)
        .unwrap_or(0.0)
        .max(0.0) as u32;
    let id = timer::set_interval(callback, ms);
    js_number(ctx, id as f64)
}

unsafe extern "C" fn cancel_timer_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    if let Some(n) = arg_number(ctx, argument_count, arguments, 0) {
        timer::clear_timer(n as u32);
    }
    sys::JSValueMakeUndefined(ctx)
}

unsafe extern "C" fn fs_read_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    js_string(ctx, &text)
}

unsafe extern "C" fn fs_exists_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_boolean(ctx, std::path::Path::new(&path).exists())
}

unsafe extern "C" fn fs_write_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let data = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_boolean(ctx, std::fs::write(&path, &data).is_ok())
}

unsafe extern "C" fn fs_mkdir_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    js_boolean(ctx, std::fs::create_dir_all(&path).is_ok())
}

unsafe extern "C" fn fs_stat_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let s = match std::fs::metadata(&path) {
        Ok(m) => format!("{};{}", if m.is_dir() { "dir" } else { "file" }, m.len()),
        Err(_) => String::new(),
    };
    js_string(ctx, &s)
}

unsafe extern "C" fn fs_readdir_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let path = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let list: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(_) => vec![],
    };
    let json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string());
    js_string(ctx, &json)
}

unsafe extern "C" fn sha256_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    use sha2::Digest;
    let data = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let hash = format!("{:x}", sha2::Sha256::digest(data.as_bytes()));
    js_string(ctx, &hash)
}

unsafe extern "C" fn hrtime_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    js_number(ctx, now.as_secs_f64())
}

unsafe extern "C" fn hrtime_ns_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    js_string(ctx, &now.as_nanos().to_string())
}

unsafe extern "C" fn jit_fib_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let n = arg_number(ctx, argument_count, arguments, 0).unwrap_or(0.0) as i64;
    js_number(ctx, crate::jit::jit_fib(n) as f64)
}

unsafe extern "C" fn jit_loop_sum_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let n = arg_number(ctx, argument_count, arguments, 0).unwrap_or(0.0) as i64;
    js_number(ctx, crate::jit::jit_loop_sum(n) as f64)
}

unsafe extern "C" fn jit_fact_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let n = arg_number(ctx, argument_count, arguments, 0).unwrap_or(0.0) as i64;
    js_number(ctx, crate::jit::jit_fact(n) as f64)
}

unsafe extern "C" fn jit_loop_power_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let n = arg_number(ctx, argument_count, arguments, 0).unwrap_or(0.0) as i64;
    js_number(ctx, crate::jit::jit_loop_power(n) as f64)
}

// ── rt-parity callbacks (ported from engine.rs) ─────────────────────────────

unsafe extern "C" fn hash_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    use sha2::Digest;
    let data = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let mut h = sha2::Sha256::new();
    h.update(data.as_bytes());
    js_string(ctx, &format!("{:x}", h.finalize()))
}

unsafe extern "C" fn chdir_cb(
    _ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let dir = arg_string(_ctx, argument_count, arguments).unwrap_or_default();
    js_boolean(_ctx, std::env::set_current_dir(&dir).is_ok())
}

unsafe extern "C" fn env_keys_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let mut keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
    keys.sort();
    js_string(ctx, &serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string()))
}

fn fill_random(buf: &mut [u8]) {
    use rand::Rng;
    rand::thread_rng().fill(buf);
}

unsafe extern "C" fn random_bytes_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let len = arg_number(ctx, argument_count, arguments, 0).unwrap_or(0.0) as usize;
    let count = len.clamp(0, 65536);
    let mut buf = vec![0u8; count];
    fill_random(&mut buf);
    // Real JS number array (parity with the QuickJS callback's Vec<u8>):
    // JSON is the cheap constructor available on this backend.
    let json = format!(
        "[{}]",
        buf.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")
    );
    drop(buf);
    js_from_json(ctx, &json)
}

unsafe extern "C" fn random_uuid_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let mut buf = [0u8; 16];
    fill_random(&mut buf);
    buf[6] = (buf[6] & 0x0f) | 0x40; // version 4
    buf[8] = (buf[8] & 0x3f) | 0x80; // variant
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        buf[0], buf[1], buf[2], buf[3],
        buf[4], buf[5],
        buf[6], buf[7],
        buf[8], buf[9],
        buf[10], buf[11], buf[12], buf[13], buf[14], buf[15]
    );
    js_string(ctx, &uuid)
}

unsafe extern "C" fn fetch_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let url = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let result = crate::engine::fetch_url(&url);
    js_string(ctx, &result)
}

unsafe extern "C" fn spawn_cb(
    ctx: JSContextRef,
    _function: JSObjectRef,
    _this: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> *const OpaqueJSValue {
    let cmd = arg_string(ctx, argument_count, arguments).unwrap_or_default();
    let args_json = if argument_count > 1 && !arguments.is_null() {
        let v = unsafe { JSValue::from_raw(ctx, *arguments.add(1)) };
        v.to_json_string(0).ok().map(|s| s.to_string()).unwrap_or_default()
    } else { String::new() };
    let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
    let output = std::process::Command::new(&cmd).args(&args).output();
    let result = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            format!(
                "{{\"ok\":true,\"code\":{},\"stdout\":{},\"stderr\":{}}}",
                out.status.code().unwrap_or(-1),
                serde_json::to_string(&stdout).unwrap_or_else(|_| "null".to_string()),
                serde_json::to_string(&stderr).unwrap_or_else(|_| "null".to_string())
            )
        }
        Err(e) => format!("{{\"ok\":false,\"error\":\"{}\"}}", e),
    };
    js_string(ctx, &result)
}

fn register_callback(
    ctx: &JSContext,
    name: &str,
    cb: javascriptcore::sys::JSObjectCallAsFunctionCallback,
) {
    let global = ctx.global_object().expect("global object");
    let func = JSValue::new_function(ctx, name, cb);
    let _ = global.set_property(name, func);
}

/// Snapshot the enumerable property names on `globalThis` immediately after
/// bootstrap so we can strip user-added globals when the context is recycled.
fn snapshot_globals(ctx: &JSContext) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Ok(global) = ctx.global_object() {
        for name in global.property_names() {
            names.insert(name.to_string());
        }
    }
    names
}

/// Build a single JS expression that deletes every own/enumerable global
/// property except those present in `allowed`.
fn reset_globals_script(allowed: &HashSet<String>) -> String {
    let mut list = String::with_capacity(allowed.len() * 12);
    for (idx, name) in allowed.iter().enumerate() {
        if idx > 0 {
            list.push(',');
        }
        // Escape quotes and backslashes so the generated literal is safe.
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        list.push('"');
        list.push_str(&escaped);
        list.push('"');
    }
    format!(
        r#"(function(a){{var g=globalThis;for(var k in g){{if(a.indexOf(k)<0){{try{{delete g[k];}}catch(e){{}}}}}}}})([{}]);"#,
        list
    )
}

/// Reset a pooled context back to its bootstrapped state.
fn reset_context(ctx: &JSContext) {
    let allowed = ALLOWED_GLOBALS.with(|a| a.borrow().clone());
    if allowed.is_empty() {
        return;
    }
    let script = reset_globals_script(&allowed);
    let _ = evaluate_script(ctx, script, None, "<reset>", 1);
}

pub struct JscRuntime {
    context: Option<JSContext>,
    working_dir: PathBuf,
    pooled: bool,
    timers: TimerState,
}

impl JscRuntime {
    pub fn new() -> RuntimeResult<Self> {
        EXIT_CODE.store(0, Ordering::Relaxed);
        timer::init();

        let mut pooled = false;
        let mut context = CONTEXT_POOL.with(|pool| pool.borrow_mut().pop());

        if let Some(ref ctx) = context {
            reset_context(ctx);
            pooled = true;
        } else {
            context = Some(JSContext::new());
        }

        let context = context.expect("context created or popped from pool");
        let cwd = std::env::current_dir().map_err(RuntimeError::IoError)?;
        let mut rt = Self {
            context: Some(context),
            working_dir: cwd,
            pooled,
            timers: TimerState::new(),
        };

        if !pooled {
            rt.bootstrap()?;
            let allowed = snapshot_globals(rt.context.as_ref().expect("context"));
            ALLOWED_GLOBALS.with(|a| {
                *a.borrow_mut() = allowed;
            });
        }

        Ok(rt)
    }

    fn bootstrap(&mut self) -> RuntimeResult<()> {
        let ctx = self.context.as_ref().expect("context");
        register_callback(ctx, "__jhol_stdout_write", Some(stdout_cb));
        register_callback(ctx, "__jhol_stderr_write", Some(stderr_cb));
        register_callback(ctx, "__jhol_set_exit_code", Some(exit_code_cb));
        register_callback(ctx, "__jhol_set_timeout", Some(set_timeout_cb));
        register_callback(ctx, "__jhol_set_interval", Some(set_interval_cb));
        register_callback(ctx, "__jhol_cancel_timer", Some(cancel_timer_cb));
        register_callback(ctx, "__jhol_fs_read", Some(fs_read_cb));
        register_callback(ctx, "__jhol_fs_exists", Some(fs_exists_cb));
        register_callback(ctx, "__jhol_fs_write", Some(fs_write_cb));
        register_callback(ctx, "__jhol_fs_mkdir", Some(fs_mkdir_cb));
        register_callback(ctx, "__jhol_fs_stat", Some(fs_stat_cb));
        register_callback(ctx, "__jhol_fs_readdir", Some(fs_readdir_cb));
        register_callback(ctx, "__jhol_sha256", Some(sha256_cb));
        register_callback(ctx, "__jhol_hrtime", Some(hrtime_cb));
        register_callback(ctx, "__jhol_hrtime_ns", Some(hrtime_ns_cb));
        register_callback(ctx, "__jhol_jit_fib", Some(jit_fib_cb));
        register_callback(ctx, "__jhol_jit_loop_sum", Some(jit_loop_sum_cb));
    register_callback(ctx, "__jhol_jit_fact", Some(jit_fact_cb));
    register_callback(ctx, "__jhol_jit_loop_power", Some(jit_loop_power_cb));
        // rt-parity: callbacks the QuickJS backend exposes and the JSC backend
        // was missing (audited by diffing the two bootstrap surfaces).
        register_callback(ctx, "__jhol_hash", Some(hash_cb));
        register_callback(ctx, "__jhol_env_get", Some(env_get_cb));
        // Shared optional-feature impls — identical behavior to the QuickJS
        // backend (shared fns in engine.rs, so the backends cannot drift).
        register_callback(ctx, "__jhol_http_listen", Some(http_listen_cb));
        register_callback(ctx, "__jhol_ws_connect", Some(ws_connect_cb2));
        register_callback(ctx, "__jhol_db_open", Some(db_open_cb2));
        register_callback(ctx, "__jhol_db_exec", Some(db_exec_cb2));
        register_callback(ctx, "__jhol_db_close", Some(db_close_cb2));
        register_callback(ctx, "__jhol_chdir", Some(chdir_cb));
        register_callback(ctx, "__jhol_os_info", Some(os_info_cb));
        register_callback(ctx, "__jhol_env_keys", Some(env_keys_cb));
        register_callback(ctx, "__jhol_random_bytes", Some(random_bytes_cb));
        register_callback(ctx, "__jhol_random_uuid", Some(random_uuid_cb));
        register_callback(ctx, "__jhol_fetch", Some(fetch_cb));
        register_callback(ctx, "__jhol_spawn", Some(spawn_cb));
        // Feature impls are registered above from the shared engine.rs fns
        // (identical behavior on both backends).
        evaluate_script(ctx, BOOTSTRAP, None, "<bootstrap>", 1)
            .map_err(|e| RuntimeError::Internal(format!("JSC bootstrap failed: {}", e)))?;
        Ok(())
    }

    pub fn execute_file(&mut self, path: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|_| RuntimeError::FileNotFound(path.to_path_buf()))?;
        let filename = canonical.to_str().unwrap_or("unknown").to_string();
        self.working_dir = canonical
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        // File-content cache: avoid re-reading unchanged files between runs.
        let source = if let Some(entry) = file_cache().get(&canonical) {
            let (cached_mtime, cached_content) = entry.value();
            let mtime = std::fs::metadata(&canonical)
                .ok()
                .and_then(|m| m.modified().ok());
            if mtime == Some(*cached_mtime) {
                cached_content.as_ref().to_string()
            } else {
                drop(entry);
                self.read_and_cache_file(&canonical)?
            }
        } else {
            self.read_and_cache_file(&canonical)?
        };

        // Transpile cache: avoid re-transpiling unchanged TypeScript/JSX.
        let js_source = if transpile::needs_transpile(&canonical) {
            let key = crate::transpile_cache::key(&source);
            if let Some(cached) = crate::transpile_cache::load(key) {
                crate::jit::maybe_optimize(&cached)
            } else {
                let js = transpile::transpile(&source, &filename)?;
                let _ = crate::transpile_cache::save(key, &js);
                crate::jit::maybe_optimize(&js)
            }
        } else {
            crate::jit::maybe_optimize(&source)
        };

        self.evaluate(&js_source, &filename)
    }

    fn read_and_cache_file(&self, path: &Path) -> RuntimeResult<String> {
        let source = std::fs::read_to_string(path).map_err(RuntimeError::IoError)?;
        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        if let Some(mt) = mtime {
            file_cache().insert(path.to_path_buf(), (mt, Arc::from(source.clone())));
        }
        Ok(source)
    }

    pub fn execute_code(&mut self, code: &str, filename: &str) -> RuntimeResult<()> {
        let js_source = if filename.ends_with(".ts")
            || filename.ends_with(".tsx")
            || filename.ends_with(".jsx")
        {
            crate::jit::maybe_optimize(&transpile::transpile(code, filename)?)
        } else {
            crate::jit::maybe_optimize(code)
        };
        self.evaluate(&js_source, filename)
    }

    fn evaluate(&mut self, code: &str, filename: &str) -> RuntimeResult<()> {
        let _dirname = self.working_dir.to_string_lossy().to_string();

        // Cache key is based on the fully wrapped module source so that changes
        // to filename or dirname invalidate the in-memory compiled function.
        let hash_input = format!("{}\0{}\0{}", filename, _dirname, code);
        let hash = crate::transpile_cache::key(&hash_input);

        let escaped_filename = serde_json::to_string(filename)
            .map_err(|e| RuntimeError::Internal(format!("json: {}", e)))?;
        let escaped_dirname = serde_json::to_string(&_dirname)
            .map_err(|e| RuntimeError::Internal(format!("json: {}", e)))?;

        // Compile the user code as a Function object with Node-like
        // (module,exports,require,__filename,__dirname) parameters, cache it in
        // the persistent context, and call it.  The first run pays the parse /
        // compile cost; subsequent runs reuse the warm function and its
        // JIT-compiled body.
        // Prepend "use strict" so the module body runs in strict mode.  This
        // eliminates sloppy-mode overhead (arguments object aliasing, with
        // lookups, etc.) and usually lets the JIT produce tighter code.
        let wrapped_code = format!("\"use strict\";\n{}", code);
        let escaped_code = serde_json::to_string(&wrapped_code)
            .map_err(|e| RuntimeError::Internal(format!("json: {}", e)))?;

        let script = format!(
            r#"(function(hash, require, __filename, __dirname, code) {{
  var module = {{ exports: {{}} }};
  var exports = module.exports;
  var cache = globalThis.__jhol_jsc_module_cache;
  var fn = cache[hash];
  if (!fn) {{
    fn = new Function('module','exports','require','__filename','__dirname', code);
    cache[hash] = fn;
  }}
  fn.call(module.exports, module, exports, require, __filename, __dirname);
}})("{:016x}", globalThis.require, {}, {}, {});"#,
            hash, escaped_filename, escaped_dirname, escaped_code
        );

        let ctx = self.context.as_ref().expect("context");
        evaluate_script(ctx, script, None, filename, 1)
            .map_err(|e| RuntimeError::ExecutionError(format!("{}:{:?}", filename, e)))?;
        Ok(())
    }

    pub fn run_event_loop(&mut self) -> RuntimeResult<i32> {
        let ctx = self.context.as_ref().expect("context");
        loop {
            timer::ingest_into(&mut self.timers);

            let now = Instant::now();
            let ready = self.timers.drain_ready(now);
            for entry in ready {
                let wrapped = format!(
                    "(function() {{ var __filename = '{}'; var __dirname = '{}'; ({}).call(globalThis); }})();",
                    self.working_dir.join("<timer>").to_string_lossy().replace('\\', "/"),
                    self.working_dir.to_string_lossy().replace('\\', "/"),
                    entry.callback
                );
                let _ = evaluate_script(ctx, wrapped, None, "<timer>", 1);
                timer::ingest_into(&mut self.timers);
            }

            timer::ingest_into(&mut self.timers);

            match self.timers.next_deadline() {
                Some(deadline) => {
                    let now = Instant::now();
                    if deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                }
                None => break,
            }
        }
        Ok(EXIT_CODE.load(Ordering::Relaxed))
    }

    pub fn exit_code(&self) -> i32 {
        EXIT_CODE.load(Ordering::Relaxed)
    }
}

impl Drop for JscRuntime {
    fn drop(&mut self) {
        if self.pooled {
            if let Some(context) = self.context.take() {
                CONTEXT_POOL.with(|pool| {
                    let mut pool = pool.borrow_mut();
                    if pool.len() < POOL_CAPACITY {
                        pool.push(context);
                    }
                });
            }
        }
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


// ── rt-parity stubs (engine.rs has the same shape; features off by default) ──

unsafe extern "C" fn http_listen_stub_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":false,\"error\":\"http-server feature not enabled\"}")
}

unsafe extern "C" fn ws_connect_stub_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":false,\"error\":\"websocket feature not enabled\"}")
}

unsafe extern "C" fn db_open_stub_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":false,\"error\":\"sqlite feature not enabled\"}")
}

unsafe extern "C" fn db_exec_stub_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":false,\"error\":\"sqlite feature not enabled\"}")
}

unsafe extern "C" fn db_close_stub_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":true}")
}

#[cfg(feature = "http-server")]
unsafe extern "C" fn http_listen_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, &crate::engine::http_listen_impl())
}

#[cfg(feature = "websocket")]
unsafe extern "C" fn ws_connect_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":true}")
}

#[cfg(feature = "sqlite")]
unsafe extern "C" fn db_open_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":true,\"id\":1}")
}

#[cfg(feature = "sqlite")]
unsafe extern "C" fn db_exec_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":true}")
}

#[cfg(feature = "sqlite")]
unsafe extern "C" fn db_close_cb(
    ctx: JSContextRef, _f: JSObjectRef, _t: JSObjectRef, _c: usize, _a: *const JSValueRef, _e: *mut JSValueRef,
) -> *const OpaqueJSValue {
    js_string(ctx, "{\"ok\":true}")
}
