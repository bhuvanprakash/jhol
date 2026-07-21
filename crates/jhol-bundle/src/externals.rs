/// Node.js built-in module names
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Check if a module specifier is a Node.js built-in
pub fn is_node_builtin(specifier: &str) -> bool {
    let clean = specifier.strip_prefix("node:").unwrap_or(specifier);
    NODE_BUILTINS.contains(&clean)
}

/// Check if a module should be treated as external
pub fn should_externalize(
    specifier: &str,
    config_external: &[String],
    platform_externals: &[String],
    target: &crate::config::Target,
) -> bool {
    // node: prefix always externalizes
    if specifier.starts_with("node:") {
        return true;
    }

    // Check explicit external list
    if config_external
        .iter()
        .any(|e| specifier == e || specifier.starts_with(&format!("{}/", e)))
    {
        return true;
    }

    // Check platform externals
    if platform_externals
        .iter()
        .any(|e| specifier == e || specifier.starts_with(&format!("{}/", e)))
    {
        return true;
    }

    // Node.js builtins
    if is_node_builtin(specifier) {
        return true;
    }

    // Target-specific
    match target {
        crate::config::Target::Node => {
            // For Node.js target, common dependencies might be external
            // if they're already available in the Node.js environment
        }
        crate::config::Target::Browser => {
            // For browser target, nothing is external by default
        }
        crate::config::Target::Jhol => {
            // For Jhol target, built-in modules are external
            if is_node_builtin(specifier) {
                return true;
            }
        }
    }

    false
}

/// Generate external module shim code
pub fn generate_external_shim(name: &str, config: &crate::config::BundleConfig) -> String {
    match config.target {
        crate::config::Target::Node => {
            format!("module.exports = require('{}');\n", name)
        }
        crate::config::Target::Browser => {
            // For browser, create an empty module
            format!("export default {};\n", "undefined")
        }
        crate::config::Target::Jhol => {
            format!("module.exports = require('{}');\n", name)
        }
    }
}
