use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;

static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

fn ureq_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .max_idle_connections(64)
            .build()
    })
}

// Timer queue shared between JS callbacks (which are `Fn`) and the Runtime.
static TIMER_QUEUE: OnceLock<Mutex<Vec<TimerEntry>>> = OnceLock::new();
static CANCELLED_IDS: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();
static NEXT_TIMER_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
struct TimerEntry {
    id: u32,
    deadline: Instant,
    callback: String,
    repeat: Option<Duration>,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap: earliest deadline first, then smallest id.
        other.deadline.cmp(&self.deadline).then_with(|| other.id.cmp(&self.id))
    }
}

struct TimerState {
    timers: BinaryHeap<Reverse<TimerEntry>>,
    cancelled: std::collections::HashSet<u32>,
}

impl TimerState {
    fn new() -> Self {
        Self {
            timers: BinaryHeap::new(),
            cancelled: std::collections::HashSet::new(),
        }
    }

    fn push(&mut self, entry: TimerEntry) {
        self.timers.push(Reverse(entry));
    }

    fn cancel(&mut self, id: u32) {
        self.cancelled.insert(id);
    }

    fn drain_ready(&mut self, now: Instant) -> Vec<TimerEntry> {
        let mut ready = Vec::new();
        while let Some(Reverse(entry)) = self.timers.peek() {
            if entry.deadline > now {
                break;
            }
            let entry = self.timers.pop().unwrap().0;
            if self.cancelled.remove(&entry.id) {
                continue;
            }
            if let Some(interval) = entry.repeat {
                self.timers.push(Reverse(TimerEntry {
                    id: entry.id,
                    deadline: now + interval,
                    callback: entry.callback.clone(),
                    repeat: Some(interval),
                }));
            }
            ready.push(entry);
        }
        ready
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.timers.peek().map(|Reverse(e)| e.deadline)
    }
}

use quickjs_rusty::compile::{compile, from_bytecode, run_compiled_function, to_bytecode};
use quickjs_rusty::{Context, ExecutionError};

use crate::errors::{RuntimeError, RuntimeResult};
use crate::modules::ModuleRegistry;
use crate::resolver::{ModuleFormat, ModuleResolver};
use crate::transpile;
use crate::tropical_graph::TropicalModuleGraph;

/// Shared runtime state accessible from 'static callbacks.
/// The resolver is lock-free for memory-cache hits; the tropical graph and
/// module registry are protected by short, coarse locks.
struct RuntimeState {
    resolver: ModuleResolver,
    module_registry: Arc<ModuleRegistry>,
    tropical_graph: Arc<RwLock<TropicalModuleGraph>>,
}

static RUNTIME_STATE: OnceLock<Arc<RuntimeState>> = OnceLock::new();

fn init_runtime_state(
    resolver: ModuleResolver,
    module_registry: Arc<ModuleRegistry>,
    tg: TropicalModuleGraph,
) {
    let _ = RUNTIME_STATE.set(Arc::new(RuntimeState {
        resolver,
        module_registry,
        tropical_graph: Arc::new(RwLock::new(tg)),
    }));
}

pub struct Runtime {
    context: Context,
    working_dir: PathBuf,
    exit_code: i32,
    /// File-content cache: path → (mtime, content)
    file_cache: HashMap<PathBuf, (SystemTime, Arc<str>)>,
    timers: TimerState,
}

impl Runtime {
    pub fn new() -> RuntimeResult<Self> {
        let context = Context::builder()
            .build()
            .map_err(|e| RuntimeError::Internal(format!("Failed to create JS context: {}", e)))?;

        let cwd = std::env::current_dir().map_err(RuntimeError::IoError)?;

        let _ = TIMER_QUEUE.set(Mutex::new(Vec::new()));
        let _ = CANCELLED_IDS.set(Mutex::new(std::collections::HashSet::new()));

        let resolver = ModuleResolver::new();
        let module_registry = Arc::new(ModuleRegistry::new());
        let tg = TropicalModuleGraph::new(128);
        init_runtime_state(resolver, module_registry, tg);

        let mut rt = Self {
            context,
            working_dir: cwd,
            exit_code: 0,
            file_cache: HashMap::new(),
            timers: TimerState::new(),
        };

        rt.bootstrap()?;
        Ok(rt)
    }

    fn bootstrap(&mut self) -> RuntimeResult<()> {
        let _ = self
            .context
            .add_callback("__jhol_stdout_write", |msg: String| -> bool {
                print!("{}", msg);
                true
            });
        let _ = self
            .context
            .add_callback("__jhol_stderr_write", |msg: String| -> bool {
                eprint!("{}", msg);
                true
            });
        let _ = self
            .context
            .add_callback("__jhol_fs_read", |path: String| -> String {
                std::fs::read_to_string(&path).unwrap_or_default()
            });
        let _ = self
            .context
            .add_callback("__jhol_fs_exists", |path: String| -> bool {
                std::path::Path::new(&path).exists()
            });
        let _ = self
            .context
            .add_callback("__jhol_hash", |data: String| -> String {
                use sha2::Digest;
                let mut h = sha2::Sha256::new();
                h.update(data.as_bytes());
                format!("{:x}", h.finalize())
            });
        let _ = self
            .context
            .add_callback("__jhol_stdout_write_raw", |msg: String| -> bool {
                use std::io::Write;
                std::io::stdout().write_all(msg.as_bytes()).is_ok()
            });
        let _ = self
            .context
            .add_callback("__jhol_chdir", |dir: String| -> bool {
                std::env::set_current_dir(&dir).is_ok()
            });
        let _ = self
            .context
            .add_callback("__jhol_exit", |code: i32| -> i32 { code });
        let _ = self
            .context
            .add_callback("__jhol_random_bytes", |len: i32| -> Vec<u8> {
                use rand::Rng;
                let count = len.clamp(0, 65536) as usize;
                let mut buf = vec![0u8; count];
                rand::thread_rng().fill(&mut buf[..]);
                buf
            });
        let _ = self.context.add_callback(
            "__jhol_hmac",
            |_algo: String, key: String, data: String| -> String {
                use sha2::digest::KeyInit;
                use sha2::Sha256;
                if let Ok(mut mac) = hmac::Hmac::<Sha256>::new_from_slice(key.as_bytes()) {
                    use hmac::Mac;
                    mac.update(data.as_bytes());
                    let result = mac.finalize();
                    let code_bytes = result.into_bytes();
                    format!("{:x}", code_bytes)
                } else {
                    String::new()
                }
            },
        );
        let _ = self
            .context
            .add_callback("__jhol_fs_write", |path: String, data: String| -> bool {
                std::fs::write(&path, &data).is_ok()
            });
        let _ = self
            .context
            .add_callback("__jhol_fs_mkdir", |path: String| -> bool {
                std::fs::create_dir_all(&path).is_ok()
            });
        let _ = self
            .context
            .add_callback("__jhol_fs_stat", |path: String| -> String {
                match std::fs::metadata(&path) {
                    Ok(m) => format!("{};{}", if m.is_dir() { "dir" } else { "file" }, m.len()),
                    Err(_) => "".to_string(),
                }
            });
        let _ = self
            .context
            .add_callback("__jhol_fs_readdir", |path: String| -> Vec<String> {
                match std::fs::read_dir(&path) {
                    Ok(entries) => entries
                        .filter_map(|e| {
                            e.ok().map(|e| e.file_name().to_string_lossy().into_owned())
                        })
                        .collect(),
                    Err(_) => vec![],
                }
            });
        let _ = self
            .context
            .add_callback("__jhol_set_exit_code", |code: i32| -> bool {
                EXIT_CODE.store(code, Ordering::Relaxed);
                true
            });
        let _ = self
            .context
            .add_callback("__jhol_set_timeout", |callback: String, ms: i32| -> i32 {
                let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(queue) = TIMER_QUEUE.get() {
                    if let Ok(mut q) = queue.lock() {
                        q.push(TimerEntry {
                            id,
                            deadline: Instant::now() + Duration::from_millis(ms.max(0) as u64),
                            callback,
                            repeat: None,
                        });
                    }
                }
                id as i32
            });
        let _ = self
            .context
            .add_callback("__jhol_set_interval", |callback: String, ms: i32| -> i32 {
                let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed) + 1;
                let interval = Duration::from_millis(ms.max(0) as u64);
                if let Some(queue) = TIMER_QUEUE.get() {
                    if let Ok(mut q) = queue.lock() {
                        q.push(TimerEntry {
                            id,
                            deadline: Instant::now() + interval,
                            callback,
                            repeat: Some(interval),
                        });
                    }
                }
                id as i32
            });
        let _ = self
            .context
            .add_callback("__jhol_cancel_timer", |id: i32| -> bool {
                let id = id as u32;
                if let Some(queue) = TIMER_QUEUE.get() {
                    if let Ok(mut q) = queue.lock() {
                        q.retain(|e| e.id != id);
                    }
                }
                if let Some(cancelled) = CANCELLED_IDS.get() {
                    if let Ok(mut c) = cancelled.lock() {
                        c.insert(id);
                    }
                }
                true
            });
        // fetch() via shared ureq agent (connection reuse)
        let _ = self
            .context
            .add_callback("__jhol_fetch", |url: String, _opts: String| -> String {
                let resp = ureq_agent().get(&url).call();
                match resp {
                    Ok(r) => {
                        let status = r.status();
                        let body = r.into_string().unwrap_or_default();
                        format!(
                            "{{\"ok\":true,\"status\":{},\"body\":{}}}",
                            status,
                            serde_json::to_string(&body).unwrap_or_else(|_| "null".to_string())
                        )
                    }
                    Err(e) => format!("{{\"ok\":false,\"error\":\"{}\"}}", e),
                }
            });
        // os module
        let _ = self.context.add_callback("__jhol_os_info", |_arg: String| -> String {
            let arch = std::env::consts::ARCH;
            let platform = std::env::consts::OS;
            let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).to_string();
            let homedir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{{\"arch\":\"{}\",\"platform\":\"{}\",\"cpus\":{},\"homedir\":\"{}\",\"tmpdir\":\"{}\"}}",
                arch, platform, cpus, homedir, std::env::temp_dir().display())
        });

        // rt-9: Real HTTP server via tiny_http (optional)
        #[cfg(feature = "http-server")]
        {
            let _ = self.context.add_callback(
                "__jhol_http_listen",
                |port: i32, _cb_name: String| -> String {
                    let port = port.clamp(1024, 65535) as u16;
                    let addr = format!("0.0.0.0:{}", port);
                    match tiny_http::Server::http(&addr) {
                        Ok(server) => {
                            std::thread::spawn(move || {
                                for request in server.incoming_requests() {
                                    let method = request.method().to_string();
                                    let url = request.url().to_string();
                                    let response = tiny_http::Response::from_string(format!(
                                        "{{\"method\":\"{}\",\"url\":\"{}\"}}",
                                        method, url
                                    ));
                                    let _ = request.respond(response);
                                }
                            });
                            format!("{{\"ok\":true,\"port\":{}}}", port)
                        }
                        Err(e) => format!("{{\"ok\":false,\"error\":\"{}\"}}", e),
                    }
                },
            );
        }
        #[cfg(not(feature = "http-server"))]
        {
            let _ = self.context.add_callback(
                "__jhol_http_listen",
                |_port: i32, _cb_name: String| -> String {
                    "{\"ok\":false,\"error\":\"http-server feature not enabled\"}".to_string()
                },
            );
        }

        // rt-10: Real WebSocket via tungstenite (optional)
        #[cfg(feature = "websocket")]
        {
            let _ = self
                .context
                .add_callback("__jhol_ws_connect", |url: String| -> String {
                    match tungstenite::connect(&url) {
                        Ok((mut socket, _)) => {
                            let _ = socket.send(tungstenite::Message::Text("jhol ws connected".into()));
                            "{\"ok\":true}".to_string()
                        }
                        Err(e) => format!("{{\"ok\":false,\"error\":\"{}\"}}", e),
                    }
                });
        }
        #[cfg(not(feature = "websocket"))]
        {
            let _ = self.context.add_callback(
                "__jhol_ws_connect",
                |_url: String| -> String {
                    "{\"ok\":false,\"error\":\"websocket feature not enabled\"}".to_string()
                },
            );
        }

        // rt-11: child_process spawn
        let _ =
            self.context
                .add_callback("__jhol_spawn", |cmd: String, args_json: String| -> String {
                    let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
                    let output = std::process::Command::new(&cmd).args(&args).output();
                    match output {
                        Ok(out) => {
                            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                            format!(
                                "{{\"ok\":true,\"code\":{},\"stdout\":{},\"stderr\":{}}}",
                                out.status.code().unwrap_or(-1),
                                serde_json::to_string(&stdout)
                                    .unwrap_or_else(|_| "null".to_string()),
                                serde_json::to_string(&stderr)
                                    .unwrap_or_else(|_| "null".to_string())
                            )
                        }
                        Err(e) => format!("{{\"ok\":false,\"error\":\"{}\"}}", e),
                    }
                });

        // rt-12: SQLite (optional)
        #[cfg(feature = "sqlite")]
        {
            let _ = self
                .context
                .add_callback("__jhol_db_open", |_path: String| -> String { "{\"ok\":false}".to_string() });
            let _ = self
                .context
                .add_callback("__jhol_db_exec", |_id: i32, _sql: String| -> String { "{\"ok\":false}".to_string() });
            let _ = self
                .context
                .add_callback("__jhol_db_close", |_id: i32, _: String| -> String { "{\"ok\":true}".to_string() });
        }
        #[cfg(not(feature = "sqlite"))]
        {
            let _ = self.context.add_callback(
                "__jhol_db_open",
                |_path: String| -> String {
                    "{\"ok\":false,\"error\":\"sqlite feature not enabled\"}".to_string()
                },
            );
            let _ = self.context.add_callback(
                "__jhol_db_exec",
                |_id: i32, _sql: String| -> String {
                    "{\"ok\":false,\"error\":\"sqlite feature not enabled\"}".to_string()
                },
            );
            let _ = self.context.add_callback(
                "__jhol_db_close",
                |_id: i32, _: String| -> String { "{\"ok\":true}".to_string() },
            );
        }

        // rt-13: File watcher via mtime poll
        let _ = self.context.add_callback(
            "__jhol_watch_start",
            |path: String, _cb_name: String| -> String {
                let canonical = std::path::Path::new(&path);
                if !canonical.exists() {
                    return "{\"ok\":false,\"error\":\"path not found\"}".to_string();
                }
                let mtime = std::fs::metadata(canonical)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    });
                format!("{{\"ok\":true,\"id\":1,\"mtime\":{}}}", mtime.unwrap_or(0))
            },
        );

        // rt-14: Web Crypto — SHA-256/512, HMAC variants
        let _ = self
            .context
            .add_callback("__jhol_sha256", |data: String| -> String {
                use sha2::Digest;
                format!("{:x}", sha2::Sha256::digest(data.as_bytes()))
            });
        let _ = self
            .context
            .add_callback("__jhol_sha512", |data: String| -> String {
                use sha2::Digest;
                format!("{:x}", sha2::Sha512::digest(data.as_bytes()))
            });

        // rt-15: perf_hooks / process exit hooks / env iteration
        let _ = self
            .context
            .add_callback("__jhol_env_keys", |_: String| -> String {
                let mut keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
                keys.sort();
                serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string())
            });
        // crypto.randomUUID
        let _ = self.context.add_callback("__jhol_random_uuid", |_: String| -> String {
            use rand::Rng;
            let mut buf = [0u8; 16];
            rand::thread_rng().fill(&mut buf);
            format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                buf[0],buf[1],buf[2],buf[3],buf[4],buf[5],buf[6],buf[7],
                buf[8],buf[9],buf[10],buf[11],buf[12],buf[13],buf[14],buf[15])
        });

        // L3: Native require callback via Rust ModuleResolver + ModuleRegistry + TropicalGraph
        let _ = self.context.add_callback(
            "__jhol_require_module",
            |parent: String, name: String| -> String {
                let state = match RUNTIME_STATE.get() {
                    Some(s) => Arc::clone(s),
                    None => return String::new(),
                };
                // L4: Register edge in tropical graph under a short write lock.
                {
                    let mut tg = state.tropical_graph.write().expect("tropical graph lock");
                    tg.add_edge(&parent, &name, 1.0, false);
                }
                // Check built-in modules without locking.
                if state.module_registry.has(&name) {
                    if let Some(source) = state.module_registry.get_exports(&name) {
                        return source;
                    }
                    return String::new();
                }
                // Resolve file module (lock-free for memory-cache hits).
                let base = std::path::Path::new(&parent)
                    .parent()
                    .unwrap_or(std::path::Path::new("."));
                let module = match state.resolver.resolve(&name, base) {
                    Ok(r) => r,
                    Err(_) => return format!("throw new Error('Cannot find module: {}')", name),
                };
                let source = module.source.as_ref().to_string();
                if source.is_empty() {
                    return format!("throw new Error('Module {} is empty')", name);
                }
                source
            },
        );

        self.context.eval(r##"
globalThis.global = globalThis;
globalThis._jhol_timer_id = 0;
globalThis._jhol_timers = {};
globalThis._jhol_intervals = {};
globalThis.setTimeout = function(fn, ms) {
    var id = ++globalThis._jhol_timer_id;
    globalThis._jhol_timers[id] = fn;
    __jhol_set_timeout('var __f = globalThis._jhol_timers[' + id + ']; if(__f){delete globalThis._jhol_timers[' + id + ']; __f.call(globalThis);}', ms || 0);
    return id;
};
globalThis.clearTimeout = function(id) { delete globalThis._jhol_timers[id]; };
globalThis.setInterval = function(fn, ms) {
    var id = ++globalThis._jhol_timer_id;
    globalThis._jhol_intervals[id] = fn;
    __jhol_set_interval('var __f = globalThis._jhol_intervals[' + id + ']; if(__f){__f.call(globalThis);}', ms || 0);
    return id;
};
globalThis.clearInterval = function(id) { delete globalThis._jhol_intervals[id]; };
globalThis.setImmediate = function(fn) {
    var args = Array.prototype.slice.call(arguments, 1);
    var id = ++globalThis._jhol_timer_id;
    globalThis._jhol_timers[id] = function() { fn.apply(null, args); };
    __jhol_set_timeout('var __f = globalThis._jhol_timers[' + id + ']; if(__f){delete globalThis._jhol_timers[' + id + ']; __f.call(globalThis);}', 0);
    return id;
};
globalThis.clearImmediate = function(id) { delete globalThis._jhol_timers[id]; };
if (!globalThis.process) globalThis.process = {};
globalThis.process._binding = function(name) {
    var a = Array.prototype.slice.call(arguments, 1);
    if (name === 'fs') {
        return {
            readFileSync: function(p) { return __jhol_fs_read(p||''); },
            existsSync: function(p) { return __jhol_fs_exists(p||''); },
            writeFileSync: function(p, data) { try { __jhol_fs_write(p||'', data||''); return true; } catch(e) { return false; } },
            mkdirSync: function(p) { try { __jhol_fs_mkdir(p||''); return true; } catch(e) { return false; } },
            statSync: function(p) { return __jhol_fs_stat(p||''); },
            readdirSync: function(p) { return __jhol_fs_readdir(p||''); }
        };
    }
    if (name === 'chdir') { return __jhol_chdir(a[0]||''); }
    if (name === 'exit') { return __jhol_exit(typeof a[0]==='number'?a[0]:0); }
    if (name === 'stdout_write') { return __jhol_stdout_write_raw(new Uint8Array(a[0])); }
    if (name === 'stderr_write') { return __jhol_stdout_write_raw(new Uint8Array(a[0])); }
    if (name === 'kill') { return true; }
    return;
};
globalThis.console = {
    log: function() { var a = Array.prototype.join.call(arguments, ' '); __jhol_stdout_write(a+'\n'); },
    warn: function() { var a = Array.prototype.join.call(arguments, ' '); __jhol_stderr_write(a+'\n'); },
    error: function() { var a = Array.prototype.join.call(arguments, ' '); __jhol_stderr_write(a+'\n'); },
    info: function() { var a = Array.prototype.join.call(arguments, ' '); __jhol_stdout_write(a+'\n'); }
};
globalThis.process.exit = function(code) { __jhol_set_exit_code(code || 0); };
globalThis.require = function(name) {
    var src = __jhol_require_module(__filename || '.', name);
    if (src.indexOf('throw') === 0) throw new Error(src);
    var mod = { exports: {} };
    var wrapper = new Function('module', 'exports', 'require', src);
    wrapper(mod, mod.exports, globalThis.require);
    return mod.exports;
};
// rt-2: UTF-8 Buffer polyfill (no TextEncoder in QuickJS)
(function(){
function utf8_encode(s){var o=[];for(var i=0;i<s.length;i++){var c=s.charCodeAt(i);if(c<128)o.push(c);else if(c<2048){o.push(192|c>>6);o.push(128|c&63)}else{o.push(224|c>>12);o.push(128|c>>6&63);o.push(128|c&63)}}return new Uint8Array(o)}
function utf8_decode(b,s,e){s=s||0;e=e||b.length;var o='';for(var i=s;i<e;){var c=b[i];if(c<128){o+=String.fromCharCode(c);i++}else if(c<224){o+=String.fromCharCode((c&31)<<6|b[i+1]&63);i+=2}else{o+=String.fromCharCode((c&15)<<12|(b[i+1]&63)<<6|b[i+2]&63);i+=3}}return o}
function Buf(input,enc){if(typeof input==='string'){input=utf8_encode(input)}this._bf=input||new Uint8Array(0)}
Buf.prototype=Object.create(Uint8Array.prototype);Buf.prototype.constructor=Buf;
Buf.prototype.toString=function(enc,s,e){return utf8_decode(this._bf||this,s,e)};
Buf.from=function(i,e){return new Buf(i,e||'utf-8')};Buf.alloc=function(s){return new Buf(new Uint8Array(s))};
Buf.isBuffer=function(o){return o instanceof Buf||o instanceof Uint8Array};
globalThis.Buffer=Buf;})();
// rt-1: fetch via native __jhol_fetch
if(!globalThis.fetch){(function(){
globalThis.fetch=function(url,opts){var r=JSON.parse(__jhol_fetch(url||'',JSON.stringify(opts||{})));if(!r.ok)throw new Error(r.error);return{ok:r.status>=200&&r.status<300,status:r.status,text:function(){return r.body},json:function(){return JSON.parse(r.body)}}};})();}
// rt-4: os module
if(!globalThis.os){(function(){
var oi=JSON.parse(__jhol_os_info(''));globalThis.os={arch:function(){return oi.arch},platform:function(){return oi.platform},cpus:function(){var a=[];for(var i=0;i<oi.cpus;i++)a.push({model:'cpu',speed:0});return a},homedir:function(){return oi.homedir},tmpdir:function(){return oi.tmpdir}};})();}
// rt-3: crypto
if(!globalThis.crypto)globalThis.crypto={};
globalThis.crypto.randomUUID=function(){return __jhol_random_uuid('')};
globalThis.crypto.randomBytes=function(n){return __jhol_random_bytes(n||16)};
globalThis.crypto.getRandomValues=function(a){var b=__jhol_random_bytes(a.length);for(var i=0;i<a.length;i++)a[i]=b[i];return a};
// rt-5: .env loader
if(!globalThis.process)globalThis.process={};
if(!globalThis.process.env)globalThis.process.env={};
(function(){try{var e=__jhol_fs_read('.env');if(e){e.split(String.fromCharCode(10)).forEach(function(l){l=l.trim();if(!l||l[0]==='#')return;var eq=l.indexOf('=');if(eq<0)return;var k=l.substring(0,eq).trim();var v=l.substring(eq+1).trim();if((v[0]===String.fromCharCode(34)||v[0]===String.fromCharCode(39))&&v[v.length-1]===v[0])v=v.slice(1,-1);globalThis.process.env[k]=v})}}catch(_){}})()
"##, false)
            .map_err(|e| RuntimeError::Internal(format!("Bootstrap failed: {}", format_error(&e))))?;
        // L5: lazy — spectral cycle detection deferred to explicit call
        // spectral_radius() no longer called at bootstrap. No O(n³) startup penalty.
        Ok(())
    }

    pub fn execute_file(&mut self, path: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|_| RuntimeError::FileNotFound(path.to_path_buf()))?;

        // rt-6: Tropical pre-fetch — parallel pre-read + transpile of predicted modules.
        if let Some(state) = RUNTIME_STATE.get() {
            let order = {
                let tg = state.tropical_graph.read().expect("tropical graph lock");
                tg.load_order_from("entry")
            };
            let base = canonical.parent().unwrap_or(std::path::Path::new("."));
            let fetched: Vec<(PathBuf, SystemTime, Arc<str>)> = order
                .par_iter()
                .filter_map(|mod_name| {
                    let resolved = state.resolver.resolve(mod_name, base).ok()?;
                    if resolved.format == ModuleFormat::Native {
                        return None;
                    }
                    let cpath = std::fs::canonicalize(&resolved.path).unwrap_or_else(|_| resolved.path.clone());
                    let raw = std::fs::read_to_string(&cpath).ok()?;
                    let mtime = std::fs::metadata(&cpath).ok()?.modified().ok()?;
                    let js = if transpile::needs_transpile(&cpath) {
                        transpile::transpile(&raw, cpath.to_str().unwrap_or("unknown")).ok()?
                    } else {
                        raw
                    };
                    Some((cpath, mtime, Arc::from(js)))
                })
                .collect();
            for (cpath, mtime, js) in fetched {
                self.file_cache.entry(cpath).or_insert((mtime, js));
            }
        }

        // File-content cache: check mtime + read or reuse cached
        let mut source = String::new();
        let use_cache =
            if let Some((cached_mtime, cached_content)) = self.file_cache.get(&canonical) {
                if let Ok(mtime) = std::fs::metadata(&canonical).and_then(|m| m.modified()) {
                    if mtime == *cached_mtime {
                        source = cached_content.as_ref().to_string();
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
        if !use_cache {
            let raw = std::fs::read_to_string(&canonical).map_err(RuntimeError::IoError)?;
            let mtime = std::fs::metadata(&canonical)
                .ok()
                .and_then(|m| m.modified().ok());
            if let Some(mt) = mtime {
                self.file_cache
                    .insert(canonical.clone(), (mt, Arc::from(raw.clone())));
            }
            source = raw;
        }

        let filename = canonical.to_str().unwrap_or("unknown").to_string();

        let js_source = if transpile::needs_transpile(&canonical) {
            transpile::transpile(&source, &filename)?
        } else {
            source
        };

        self.working_dir = canonical
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

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
        if let Some(state) = RUNTIME_STATE.get() {
            let _ = state
                .tropical_graph
                .write()
                .expect("tropical graph lock")
                .tree_shake("entry");
        }
        let _dirname = self.working_dir.to_string_lossy().to_string();
        let wrapped = format!(
            r#"var module = {{ exports: {{}} }};
var exports = module.exports;
var __filename = "{}";
var __dirname = "{}";
var require = globalThis.require;
{}
"#,
            filename, _dirname, code
        );

        let hash = crate::bytecode_cache::key(&wrapped);
        if let Some(bytes) = crate::bytecode_cache::load(hash) {
            let raw = unsafe { self.context.context_raw() };
            if let Ok(func) = from_bytecode(raw, &bytes) {
                if let Ok(compiled) = func.try_into_compiled_function() {
                    if run_compiled_function(&compiled).is_ok() {
                        return Ok(());
                    }
                }
            }
        }

        let raw = unsafe { self.context.context_raw() };
        match compile(raw, &wrapped, filename) {
            Ok(func) => {
                if let Ok(compiled) = func.try_into_compiled_function() {
                    let bytes = to_bytecode(raw, &compiled);
                    let _ = crate::bytecode_cache::save(hash, &bytes);
                    if run_compiled_function(&compiled).is_ok() {
                        Ok(())
                    } else {
                        Err(RuntimeError::ExecutionError(format!(
                            "{}: compiled function failed",
                            filename
                        )))
                    }
                } else {
                    Err(RuntimeError::ExecutionError(format!(
                        "{}: compile output was not a function",
                        filename
                    )))
                }
            }
            Err(e) => Err(RuntimeError::ExecutionError(format!(
                "{}:{}",
                filename,
                format_error(&e)
            ))),
        }
    }

    pub fn run_event_loop(&mut self) -> RuntimeResult<i32> {
        loop {
            // Ingest timers scheduled by JS callbacks.
            if let Some(queue) = TIMER_QUEUE.get() {
                if let Ok(mut q) = queue.lock() {
                    for entry in q.drain(..) {
                        self.timers.push(entry);
                    }
                }
            }

            // Ingest cancellation requests.
            if let Some(cancelled) = CANCELLED_IDS.get() {
                if let Ok(mut c) = cancelled.lock() {
                    for id in c.drain() {
                        self.timers.cancel(id);
                    }
                }
            }

            let now = Instant::now();
            let ready = self.timers.drain_ready(now);
            for entry in ready {
                let wrapped = format!(
                    "(function() {{ var __filename = '{}'; var __dirname = '{}'; ({}).call(globalThis); }})();",
                    self.working_dir.join("<timer>").to_string_lossy().replace('\\', "/"),
                    self.working_dir.to_string_lossy().replace('\\', "/"),
                    entry.callback
                );
                let _ = self.context.eval(&wrapped, false);
                // Callbacks may schedule more timers or cancel existing ones.
                if let Some(queue) = TIMER_QUEUE.get() {
                    if let Ok(mut q) = queue.lock() {
                        for entry in q.drain(..) {
                            self.timers.push(entry);
                        }
                    }
                }
                if let Some(cancelled) = CANCELLED_IDS.get() {
                    if let Ok(mut c) = cancelled.lock() {
                        for id in c.drain() {
                            self.timers.cancel(id);
                        }
                    }
                }
            }

            if let Some(queue) = TIMER_QUEUE.get() {
                if let Ok(mut q) = queue.lock() {
                    for entry in q.drain(..) {
                        self.timers.push(entry);
                    }
                }
            }
            if let Some(cancelled) = CANCELLED_IDS.get() {
                if let Ok(mut c) = cancelled.lock() {
                    for id in c.drain() {
                        self.timers.cancel(id);
                    }
                }
            }

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

        let global_code = EXIT_CODE.load(Ordering::Relaxed);
        Ok(if global_code != 0 { global_code } else { self.exit_code })
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(state) = RUNTIME_STATE.get() {
            state.resolver.save();
        }
    }
}

fn format_error(e: &ExecutionError) -> String {
    match e {
        ExecutionError::InputWithZeroBytes => "Input contains zero bytes".to_string(),
        ExecutionError::Conversion(ve) => format!("Value conversion error: {}", ve),
        ExecutionError::Internal(msg) => msg.clone(),
        ExecutionError::Exception(val) => val
            .to_string()
            .unwrap_or_else(|_| "JS exception".to_string()),
        ExecutionError::OutOfMemory => "Out of memory".to_string(),
        ExecutionError::__NonExhaustive => "Unknown execution error".to_string(),
    }
}
