use super::BuiltinModule;
use std::collections::HashMap;

pub struct ProcessModule;

impl BuiltinModule for ProcessModule {
    fn name(&self) -> &'static str {
        "process"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        // Version info
        let versions = serde_json::json!({
            "node": "23.0.0-jhol",
            "jhol": "1.0.2",
            "v8": "12.4",
            "uv": "1.48",
            "zlib": "1.3",
            "brotli": "1.1",
            "ares": "1.28",
            "modules": "127",
            "nghttp2": "1.61",
            "napi": "9",
            "llhttp": "9.2",
            "libuv": "1.48",
        });
        m.insert(
            "versions".to_string(),
            format!(
                "JSON.parse('{}')",
                versions.to_string().replace('\'', "\\'")
            ),
        );

        // Architecture and platform
        let arch = std::env::consts::ARCH;
        let platform = std::env::consts::OS;
        m.insert("arch".to_string(), format!("'{}'", arch));
        m.insert("platform".to_string(), format!("'{}'", platform));

        // Environment
        let env_json = {
            let mut env_map = serde_json::Map::new();
            for (k, v) in std::env::vars() {
                env_map.insert(k, serde_json::Value::String(v));
            }
            serde_json::Value::Object(env_map).to_string()
        };
        m.insert(
            "env".to_string(),
            format!("JSON.parse('{}')", env_json.replace('\'', "\\'")),
        );

        // argv
        let argv: Vec<String> = std::env::args().collect();
        let argv_json = serde_json::to_string(&argv).unwrap_or_default();
        m.insert("argv".to_string(), argv_json);

        // PID
        m.insert("pid".to_string(), std::process::id().to_string());

        // cwd
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());
        m.insert(
            "cwd".to_string(),
            format!("() => '{}'", cwd.replace('\'', "\\'")),
        );
        m.insert("chdir".to_string(),
            "(dir) => { try { process._binding('chdir', dir); return undefined; } catch(e) { throw e; } }".to_string()
        );
        m.insert(
            "exit".to_string(),
            "(code = 0) => { process._binding('exit', code); }".to_string(),
        );
        m.insert(
            "uptime".to_string(),
            "() => process._binding('uptime')".to_string(),
        );
        m.insert("hrtime".to_string(),
            "(time) => { const now = Date.now() * 1e6; if (time) return [now - time[0], 0]; return [now, 0]; }".to_string()
        );

        // nextTick
        m.insert("nextTick".to_string(),
            "(cb, ...args) => { if (typeof cb !== 'function') throw new TypeError('callback is not a function'); queueMicrotask(() => cb(...args)); }".to_string()
        );

        // stdout, stderr, stdin
        m.insert("stdout".to_string(),
            "{ write: (s) => { const bytes = new TextEncoder().encode(String(s)); process._binding('stdout_write', bytes); }, _isStdio: true }".to_string()
        );
        m.insert("stderr".to_string(),
            "{ write: (s) => { const bytes = new TextEncoder().encode(String(s)); process._binding('stderr_write', bytes); }, _isStdio: true }".to_string()
        );
        m.insert(
            "stdin".to_string(),
            "{ _isStdio: true, setRawMode: () => {}, resume: () => {}, pause: () => {} }"
                .to_string(),
        );

        // process.argv0
        m.insert(
            "argv0".to_string(),
            format!("'{}'", argv.first().map(|s| s.as_str()).unwrap_or("jhol")),
        );

        // process.on / process.once
        m.insert("on".to_string(),
            "(event, listener) => { if (event === 'beforeExit' || event === 'exit') { if (!process._exitHandlers) process._exitHandlers = []; process._exitHandlers.push({ event, listener, once: false }); } return process; }".to_string()
        );
        m.insert("once".to_string(),
            "(event, listener) => { if (event === 'beforeExit' || event === 'exit') { if (!process._exitHandlers) process._exitHandlers = []; process._exitHandlers.push({ event, listener, once: true }); } return process; }".to_string()
        );

        // process.memoryUsage
        m.insert(
            "memoryUsage".to_string(),
            "() => ({ rss: 0, heapTotal: 0, heapUsed: 0, external: 0, arrayBuffers: 0 })"
                .to_string(),
        );

        // process.cpuUsage
        m.insert(
            "cpuUsage".to_string(),
            "() => ({ user: 0, system: 0 })".to_string(),
        );

        // process.features
        m.insert("features".to_string(),
            "{ cachedData: false, inspector: false, debug: false, uv: true, ipv6: true, tls_alpn: true, tls_sni: true, tls_ocsp: true, tls: true }".to_string()
        );

        // process.release
        m.insert("release".to_string(),
            "{ name: 'jhol', sourceUrl: 'https://github.com/bhuvanprakash/jhol', headersUrl: 'https://github.com/bhuvanprakash/jhol' }".to_string()
        );

        // process.title
        m.insert("title".to_string(), "'jhol'".to_string());
        m.insert("config".to_string(), "{}".to_string());

        // process.kill
        m.insert(
            "kill".to_string(),
            "(pid, signal) => { process._binding('kill', pid, signal || 'SIGTERM'); }".to_string(),
        );

        // process.umask
        m.insert(
            "umask".to_string(),
            "(mask) => { if (mask === undefined) return 0o022; return 0o022; }".to_string(),
        );

        // process.getuid / getgid
        m.insert("getuid".to_string(), "() => 0".to_string());
        m.insert("getgid".to_string(), "() => 0".to_string());
        m.insert("geteuid".to_string(), "() => 0".to_string());
        m.insert("getegid".to_string(), "() => 0".to_string());

        // Resource usage
        m.insert("resourceUsage".to_string(),
            "() => ({ userCPUTime: 0, systemCPUTime: 0, maxRSS: 0, sharedMemorySize: 0, unsharedDataSize: 0, unsharedStackSize: 0, minorPageFault: 0, majorPageFault: 0, swappedOut: 0, fsRead: 0, fsWrite: 0, ipcSent: 0, ipcReceived: 0, signalsCount: 0, voluntaryContextSwitches: 0, involuntaryContextSwitches: 0 })".to_string()
        );

        // process.version
        m.insert("version".to_string(), "'v23.0.0'".to_string());

        // process.allowedNodeEnvironmentFlags
        m.insert(
            "allowedNodeEnvironmentFlags".to_string(),
            "new Set()".to_string(),
        );

        // process.mainModule (deprecated but used)
        m.insert("mainModule".to_string(), "undefined".to_string());

        m
    }
}
