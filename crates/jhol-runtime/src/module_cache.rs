//! Module pre-compilation cache.
//!
//! Provides an install-time / warm-up API that walks a directory of
//! JavaScript/TypeScript modules, transpiles them, wraps them in the Node.js
//! module function, compiles them to QuickJS bytecode, and persists the
//! bytecode in `bytecode_cache`.  At runtime the existing `engine::evaluate`
//! path loads and runs the cached bytecode directly, skipping read/transpile/
//! compile for unchanged modules.

use std::path::{Path, PathBuf};

use quickjs_rusty::compile::{compile, to_bytecode};
use quickjs_rusty::Context;

use crate::errors::{RuntimeError, RuntimeResult};
use crate::transpile;

/// Pre-compile a single module file to QuickJS bytecode and store it in the
/// on-disk bytecode cache.  Returns the number of bytes written.
pub fn precompile_file(path: &Path) -> RuntimeResult<usize> {
    let canonical =
        std::fs::canonicalize(path).map_err(|_| RuntimeError::FileNotFound(path.to_path_buf()))?;
    let source = std::fs::read_to_string(&canonical).map_err(RuntimeError::IoError)?;
    let filename = canonical.to_str().unwrap_or("unknown").to_string();

    let js_source = if transpile::needs_transpile(&canonical) {
        transpile::transpile(&source, &filename)?
    } else {
        source
    };

    // Wrap in the same Node-like module function used at runtime.
    let wrapped = format!(
        r#"var module = {{ exports: {{}} }};
var exports = module.exports;
var __filename = "{}";
var __dirname = "{}";
var require = globalThis.require;
{}
"#,
        filename,
        canonical
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| ".".to_string()),
        js_source
    );

    let hash = crate::bytecode_cache::key(&wrapped);

    let ctx = Context::builder()
        .build()
        .map_err(|e| RuntimeError::Internal(format!("precompile context: {}", e)))?;
    let raw = unsafe { ctx.context_raw() };

    let func = compile(raw, &wrapped, &filename)
        .map_err(|e| RuntimeError::Internal(format!("precompile compile: {:?}", e)))?;
    let compiled = func
        .try_into_compiled_function()
        .map_err(|_| RuntimeError::Internal("precompile output was not a function".to_string()))?;
    let bytes = to_bytecode(raw, &compiled);
    crate::bytecode_cache::save(hash, &bytes)?;
    Ok(bytes.len())
}

/// Pre-compile every supported module under `dir` to bytecode.  Returns the
/// total number of files processed.
pub fn precompile_directory(dir: &Path) -> RuntimeResult<usize> {
    let mut count = 0;
    for path in list_module_files(dir) {
        if precompile_file(&path).is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

/// Return a list of module files that are candidates for pre-compilation.
pub fn list_module_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && is_module_file(&path) {
                files.push(path);
            }
        }
    }
    files
}

fn is_module_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_lowercase().as_str(),
                "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts"
            )
        })
        .unwrap_or(false)
}
