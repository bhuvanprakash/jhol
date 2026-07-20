use super::BuiltinModule;
use std::collections::HashMap;

pub struct FsModule;

impl BuiltinModule for FsModule {
    fn name(&self) -> &'static str {
        "fs"
    }

    fn exports(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();

        // Synchronous methods using function declarations
        m.insert("readFileSync".to_string(),
            "(path, options) => { const encoding = typeof options === 'string' ? options : (options && options.encoding ? options.encoding : null); try { const fs = process._binding('fs'); if (encoding) return fs.readFileSync(path, encoding); return fs.readFileSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("writeFileSync".to_string(),
            "(path, data, options) => { try { const fs = process._binding('fs'); fs.writeFileSync(path, data); } catch(e) { throw e; } }".to_string()
        );
        m.insert("existsSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); fs.accessSync(path); return true; } catch(e) { return false; } }".to_string()
        );
        m.insert("mkdirSync".to_string(),
            "(path, options) => { const recursive = options && options.recursive === true; try { const fs = process._binding('fs'); fs.mkdirSync(path, recursive); } catch(e) { throw e; } }".to_string()
        );
        m.insert("readdirSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); return fs.readdirSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("statSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); return fs.statSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("unlinkSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); fs.unlinkSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("rmdirSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); fs.rmdirSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("renameSync".to_string(),
            "(oldPath, newPath) => { try { const fs = process._binding('fs'); fs.renameSync(oldPath, newPath); } catch(e) { throw e; } }".to_string()
        );
        m.insert("copyFileSync".to_string(),
            "(src, dest) => { try { const fs = process._binding('fs'); fs.copyFileSync(src, dest); } catch(e) { throw e; } }".to_string()
        );
        m.insert("chmodSync".to_string(),
            "(path, mode) => { try { const fs = process._binding('fs'); fs.chmodSync(path, mode); } catch(e) { throw e; } }".to_string()
        );
        m.insert("realpathSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); return fs.realpathSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("lstatSync".to_string(),
            "(path) => { try { const fs = process._binding('fs'); return fs.lstatSync(path); } catch(e) { throw e; } }".to_string()
        );
        m.insert("accessSync".to_string(),
            "(path, mode) => { try { const fs = process._binding('fs'); fs.accessSync(path, mode); } catch(e) { throw e; } }".to_string()
        );

        // Promise-based API (fs.promises)
        let promises_exports: HashMap<String, String> = [
            ("readFile".to_string(), "(path, options) => Promise.resolve().then(() => { const f = module.require('fs'); return f.readFileSync(path, options); })".to_string()),
            ("writeFile".to_string(), "(path, data, options) => Promise.resolve().then(() => { const f = module.require('fs'); return f.writeFileSync(path, data, options); })".to_string()),
            ("mkdir".to_string(), "(path, options) => Promise.resolve().then(() => { const f = module.require('fs'); return f.mkdirSync(path, options); })".to_string()),
            ("readdir".to_string(), "(path) => Promise.resolve().then(() => { const f = module.require('fs'); return f.readdirSync(path); })".to_string()),
            ("stat".to_string(), "(path) => Promise.resolve().then(() => { const f = module.require('fs'); return f.statSync(path); })".to_string()),
            ("unlink".to_string(), "(path) => Promise.resolve().then(() => { const f = module.require('fs'); return f.unlinkSync(path); })".to_string()),
            ("rmdir".to_string(), "(path) => Promise.resolve().then(() => { const f = module.require('fs'); return f.rmdirSync(path); })".to_string()),
            ("access".to_string(), "(path, mode) => Promise.resolve().then(() => { const f = module.require('fs'); return f.accessSync(path, mode); })".to_string()),
        ].into_iter().collect();

        let promises_json = serde_json::to_string(&promises_exports).unwrap_or_default();
        m.insert("promises".to_string(),
            format!("(() => {{ const p = {{}}; const methods = {}; for (const [k, v] of Object.entries(methods)) {{ p[k] = eval('(' + v + ')'); }} return p; }})()", promises_json)
        );

        // Constants
        m.insert("constants".to_string(),
            "JSON.parse('{\"F_OK\":0,\"R_OK\":4,\"W_OK\":2,\"X_OK\":1,\"COPYFILE_EXCL\":1,\"COPYFILE_FICLONE\":2}')".to_string()
        );

        // Stream aliases
        m.insert(
            "createReadStream".to_string(),
            "() => { throw new Error('fs.createReadStream: not implemented in this version'); }"
                .to_string(),
        );
        m.insert(
            "createWriteStream".to_string(),
            "() => { throw new Error('fs.createWriteStream: not implemented in this version'); }"
                .to_string(),
        );

        // Watch
        m.insert("watch".to_string(),
            "(filename, options, listener) => { if (typeof options === 'function') { listener = options; } const callback = listener || (() => {}); return { on: (evt, handler) => { if (evt === 'change') callback('change', filename); }, close: () => {} }; }".to_string()
        );

        m
    }
}
