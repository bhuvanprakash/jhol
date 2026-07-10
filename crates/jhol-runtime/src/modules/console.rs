use super::BuiltinModule;
use std::collections::HashMap;

pub struct ConsoleModule;

impl BuiltinModule for ConsoleModule {
    fn name(&self) -> &'static str {
        "console"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        m.insert("Console".to_string(),
            "class Console { \
             constructor(options) { this._stdout = (options && options.stdout) || process.stdout; this._stderr = (options && options.stderr) || process.stderr; } \
             _format(args) { return args.map(a => typeof a === 'object' ? JSON.stringify(a, null, 2) : String(a)).join(' '); } \
             log(...args) { process._binding('stdout_write', new TextEncoder().encode(this._format(args) + '\\n')); } \
             info(...args) { this.log(...args); } \
             warn(...args) { process._binding('stderr_write', new TextEncoder().encode(this._format(args) + '\\n')); } \
             error(...args) { process._binding('stderr_write', new TextEncoder().encode(this._format(args) + '\\n')); } \
             debug(...args) { this.log(...args); } \
             trace(...args) { const err = new Error(); this.warn('Trace:', ...args); this.warn(err.stack ? err.stack.split('\\n').slice(2).join('\\n') : ''); } \
             dir(obj, options) { this.log(JSON.stringify(obj, null, 2)); } \
             time(label) { if (!console._times) console._times = {}; console._times[label || 'default'] = Date.now(); } \
             timeEnd(label) { const key = label || 'default'; const start = (console._times || {})[key]; if (start) { this.log(`${label || 'default'}: ${Date.now() - start}ms`); delete console._times[key]; } else { this.warn(`No such label: ${label}`); } } \
             timeLog(label, ...args) { const key = label || 'default'; const start = (console._times || {})[key]; if (start) { this.log(`${label || 'default'}: ${Date.now() - start}ms`, ...args); } } \
             count(label) { if (!console._counts) console._counts = {}; const key = label || 'default'; console._counts[key] = (console._counts[key] || 0) + 1; this.log(`${key}: ${console._counts[key]}`); } \
             countReset(label) { if (!console._counts) console._counts = {}; delete console._counts[label || 'default']; } \
             group(...args) { if (args.length) this.log(...args); if (!console._groupDepth) console._groupDepth = 0; console._groupDepth++; } \
             groupEnd() { if (!console._groupDepth) console._groupDepth = 0; if (console._groupDepth > 0) console._groupDepth--; } \
             assert(condition, ...args) { if (!condition) { this.error('Assertion failed:', ...args); process.exit(1); } } \
             clear() { process._binding('stdout_write', new TextEncoder().encode('\\x1b[2J\\x1b[0f')); } \
             table(data, columns) { if (Array.isArray(data)) { for (const item of data) this.log(JSON.stringify(item)); } else { this.log(JSON.stringify(data, null, 2)); } } \
             dirxml(...args) { this.log(...args); } \
             groupCollapsed(...args) { this.group(...args); } \
             profile(label) {} \
             profileEnd(label) {} \
             timeStamp(label) {} \
           }".to_string()
        );

        // Global console instance
        m.insert("console".to_string(), "new Console()".to_string());

        m
    }
}
