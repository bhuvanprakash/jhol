use super::BuiltinModule;
use std::collections::HashMap;

pub struct OsModule;

impl BuiltinModule for OsModule {
    fn name(&self) -> &'static str {
        "os"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        let platform = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let hostname = hostname().unwrap_or_else(|_| "localhost".to_string());
        let homedir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/".to_string());
        let tmpdir = std::env::temp_dir().to_string_lossy().to_string();
        let eol = if cfg!(windows) { "\\r\\n" } else { "\\n" };

        m.insert("platform".to_string(), format!("() => '{}'", platform));
        m.insert("arch".to_string(), format!("() => '{}'", arch));
        m.insert("type".to_string(), format!("() => '{}'", os_type()));
        m.insert("release".to_string(), format!("() => '{}'", os_release()));
        m.insert(
            "hostname".to_string(),
            format!("() => '{}'", hostname.replace('\'', "\\'")),
        );
        m.insert(
            "homedir".to_string(),
            format!("() => '{}'", homedir.replace('\'', "\\'")),
        );
        m.insert(
            "tmpdir".to_string(),
            format!("() => '{}'", tmpdir.replace('\'', "\\'")),
        );
        m.insert("EOL".to_string(), format!("'{}'", eol));
        m.insert("endianness".to_string(), "() => 'LE'".to_string());

        // CPU info
        m.insert("cpus".to_string(),
            "() => { const cpus = []; const count = 1; for (let i = 0; i < count; i++) { cpus.push({ model: 'Jhol Virtual CPU', speed: 2400, times: { user: 0, nice: 0, sys: 0, idle: 0, irq: 0 } }); } return cpus; }".to_string()
        );

        // Memory
        m.insert("totalmem".to_string(), "() => 8589934592".to_string());
        m.insert("freemem".to_string(), "() => 4294967296".to_string());

        // Network interfaces
        m.insert("networkInterfaces".to_string(),
            "() => ({ lo: [{ address: '127.0.0.1', netmask: '255.0.0.0', family: 'IPv4', mac: '00:00:00:00:00:00', internal: true, cidr: '127.0.0.1/8' }] })".to_string()
        );

        // User info
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        m.insert("userInfo".to_string(),
            format!("(options) => ({{ uid: 0, gid: 0, username: '{}', homedir: '{}', shell: '/bin/bash' }})",
                username.replace('\'', "\\'"),
                homedir.replace('\'', "\\'")
            )
        );

        // Uptime
        m.insert("uptime".to_string(), "() => process.uptime()".to_string());

        // Load average
        m.insert("loadavg".to_string(), "() => [0, 0, 0]".to_string());

        // Constants
        m.insert("constants".to_string(),
            "{ UV_UDP_REUSEADDR: 4, priority: { PRIORITY_LOW: 19, PRIORITY_BELOW_NORMAL: 10, PRIORITY_NORMAL: 0, PRIORITY_ABOVE_NORMAL: -7, PRIORITY_HIGH: -14, PRIORITY_HIGHEST: -20 }, signals: { SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5, SIGABRT: 6, SIGBUS: 7, SIGFPE: 8, SIGKILL: 9, SIGUSR1: 10, SIGSEGV: 11, SIGUSR2: 12, SIGPIPE: 13, SIGALRM: 14, SIGTERM: 15, SIGSTKFLT: 16, SIGCHLD: 17, SIGCONT: 18, SIGTSTP: 19, SIGTTIN: 21, SIGTTOU: 22, SIGURG: 23, SIGXCPU: 24, SIGXFSZ: 25, SIGVTALRM: 26, SIGPROF: 27, SIGWINCH: 28, SIGIO: 29, SIGSYS: 31 } }".to_string()
        );

        // OS version
        m.insert("version".to_string(), "() => 'Monterey 12.7'".to_string());

        // Machine
        m.insert("machine".to_string(), format!("() => '{}'", arch));
        m.insert(
            "devNull".to_string(),
            if cfg!(windows) {
                "'NUL'".to_string()
            } else {
                "'/dev/null'".to_string()
            },
        );

        // Priority
        m.insert(
            "setPriority".to_string(),
            "(pid, priority) => {}".to_string(),
        );
        m.insert("getPriority".to_string(), "(pid) => 0".to_string());

        // tmpdir alias
        m.insert("tmpDir".to_string(), "() => os.tmpdir()".to_string());
        m.insert(
            "homedir_alias".to_string(),
            "() => os.homedir()".to_string(),
        );

        // AvailableParallelism
        m.insert("availableParallelism".to_string(), "() => 1".to_string());

        m
    }
}

fn hostname() -> Result<String, std::io::Error> {
    Ok(std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "localhost".to_string()))
}

fn os_type() -> &'static str {
    if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "windows") {
        "Windows_NT"
    } else {
        "Unknown"
    }
}

fn os_release() -> String {
    if cfg!(target_os = "macos") {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "0.0.0".to_string())
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "0.0.0".to_string())
    } else {
        "0.0.0".to_string()
    }
}
