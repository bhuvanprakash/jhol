use super::BuiltinModule;
use std::collections::HashMap;

pub struct PathModule;

impl BuiltinModule for PathModule {
    fn name(&self) -> &'static str {
        "path"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        m.insert("sep".to_string(), "'/'".to_string());
        m.insert("delimiter".to_string(), "':'".to_string());
        m.insert("posix".to_string(), "module.exports".to_string());

        m.insert(
            "win32".to_string(),
            "JSON.parse('{\"sep\":\"/\",\"delimiter\":\":\"}')".to_string(),
        );

        // Core path functions
        m.insert("basename".to_string(),
            "(p, ext) => { const parts = p.replace(/\\\\/g, '/').split('/').filter(Boolean); const base = parts[parts.length - 1] || ''; if (ext && base.endsWith(ext)) return base.slice(0, -ext.length); return base; }".to_string()
        );
        m.insert("dirname".to_string(),
            "(p) => { const normalized = p.replace(/\\\\/g, '/'); const idx = normalized.lastIndexOf('/'); if (idx === -1) return '.'; if (idx === 0) return '/'; return normalized.slice(0, idx); }".to_string()
        );
        m.insert("extname".to_string(),
            "(p) => { const base = p.split('/').pop() || ''; const idx = base.lastIndexOf('.'); if (idx <= 0) return ''; return base.slice(idx); }".to_string()
        );
        m.insert("join".to_string(),
            "(...args) => { const parts = args.flatMap(a => (a || '').replace(/\\\\/g, '/').split('/')).filter(Boolean); if (args[0] && args[0].startsWith('/')) return '/' + parts.join('/'); return parts.join('/') || '.'; }".to_string()
        );
        m.insert("resolve".to_string(),
            "(...args) => { if (args.length === 0) return process.cwd(); const parts = []; let isAbsolute = false; for (const a of args) { const s = a.replace(/\\\\/g, '/'); if (s.startsWith('/')) { parts.length = 0; isAbsolute = true; } s.split('/').filter(Boolean).forEach(p => { if (p === '..') parts.pop(); else if (p !== '.') parts.push(p); }); } const joined = parts.join('/'); return isAbsolute ? '/' + joined : process.cwd() + '/' + joined; }".to_string()
        );
        m.insert("normalize".to_string(),
            "(p) => { const parts = p.replace(/\\\\/g, '/').split('/'); const result = []; for (const part of parts) { if (part === '..') { if (result.length > 0 && result[result.length - 1] !== '..') result.pop(); else result.push('..'); } else if (part !== '.' && part !== '') { result.push(part); } } const prefix = p.startsWith('/') ? '/' : ''; return prefix + result.join('/') || '.'; }".to_string()
        );
        m.insert(
            "isAbsolute".to_string(),
            "(p) => p.replace(/\\\\/g, '/').startsWith('/') || /^[a-zA-Z]:\\\\/.test(p)"
                .to_string(),
        );
        m.insert("relative".to_string(),
            "(from, to) => { const f = path.resolve(from).split('/'); const t = path.resolve(to).split('/'); let i = 0; while (i < f.length && i < t.length && f[i] === t[i]) i++; let up = ''; for (let j = i; j < f.length; j++) up += '../'; return up + t.slice(i).join('/'); }".to_string()
        );
        m.insert("format".to_string(),
            "(obj) => { const dir = obj.dir || path.dirname(obj.base || ''); const base = obj.base || (obj.name || '') + (obj.ext || ''); return path.join(dir, base); }".to_string()
        );
        m.insert("parse".to_string(),
            "(p) => { const root = p.startsWith('/') ? '/' : ''; const dir = path.dirname(p); const base = path.basename(p); const ext = path.extname(p); const name = base.slice(0, -ext.length || base.length); return { root, dir, base, ext, name }; }".to_string()
        );
        m.insert("toNamespacedPath".to_string(), "(p) => p".to_string());

        m
    }
}
