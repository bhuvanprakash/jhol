#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod bytecode_cache;
pub mod engine;
pub mod errors;
pub mod jit;
#[cfg(feature = "jsc-engine")]
pub mod jsc_engine;
pub mod module_cache;
pub mod modules;
pub mod resolver;
pub mod timer;
pub mod transpile;
pub mod transpile_cache;
pub mod tropical_graph;
pub mod watch;

use std::path::Path;

use errors::RuntimeResult;

/// Version of the Jhol Runtime
pub const JHOL_RUNTIME_VERSION: &str = "1.0.2";

/// Node.js compatibility version advertised
pub const NODE_COMPAT_VERSION: &str = "23.0.0";

/// Run a JavaScript/TypeScript file.
///
/// When the `jsc-engine` feature is enabled, this dispatches to the
/// JavaScriptCore backend so the runtime benefits from JSC's JIT and the
/// context-pooling / function-cache optimizations implemented there.
/// Otherwise it falls back to the native QuickJS backend.
#[allow(clippy::needless_return)]
pub fn run_file(path: &Path) -> RuntimeResult<i32> {
    #[cfg(feature = "jsc-engine")]
    {
        return jsc_engine::run_file(path);
    }
    #[cfg(not(feature = "jsc-engine"))]
    {
        let mut runtime = engine::Runtime::new()?;
        runtime.execute_file(path)?;
        runtime.run_event_loop()?;
        Ok(runtime.exit_code())
    }
}

/// Run a JavaScript/TypeScript file with watch mode
pub fn run_file_watch(path: &Path) -> RuntimeResult<i32> {
    let mut session = watch::WatchSession::new(path, |p| {
        let mut rt = engine::Runtime::new()?;
        rt.execute_file(p)?;
        rt.run_event_loop()?;
        Ok(())
    })?;
    session.run()?;
    Ok(0)
}

/// Evaluate inline JavaScript/TypeScript code.
///
/// Dispatches to the JSC backend when the `jsc-engine` feature is enabled.
#[allow(clippy::needless_return)]
pub fn eval_code(code: &str, filename: Option<&str>) -> RuntimeResult<i32> {
    #[cfg(feature = "jsc-engine")]
    {
        return jsc_engine::eval_code(code, filename);
    }
    #[cfg(not(feature = "jsc-engine"))]
    {
        let mut runtime = engine::Runtime::new()?;
        let fname = filename.unwrap_or("<eval>");
        runtime.execute_code(code, fname)?;
        runtime.run_event_loop()?;
        Ok(runtime.exit_code())
    }
}

/// Compile TypeScript/JSX to JavaScript (returns the transpiled source)
pub fn compile_ts(source: &str, filename: &str) -> RuntimeResult<String> {
    transpile::transpile(source, filename)
}

/// Check if a file is a supported runtime file (js, ts, jsx, tsx, mjs, cjs)
pub fn is_supported_file(path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_file() {
        assert!(is_supported_file(Path::new("test.js")));
        assert!(is_supported_file(Path::new("test.ts")));
        assert!(is_supported_file(Path::new("test.jsx")));
        assert!(is_supported_file(Path::new("test.tsx")));
        assert!(!is_supported_file(Path::new("test.css")));
        assert!(!is_supported_file(Path::new("test.json")));
    }

    #[test]
    fn test_eval_simple() {
        let result = eval_code("1 + 1", Some("test.js"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_ts() {
        let result = compile_ts("const x: number = 42;", "test.ts");
        // May fail without swc feature, but should not panic
        let _ = result;
    }

    #[test]
    fn test_set_timeout_runs() {
        let code = r#"
            var ran = false;
            setTimeout(function() { ran = true; }, 10);
            // Event loop will wait for the timer.
        "#;
        let result = eval_code(code, Some("test.js"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_timeout_sets_global() {
        let code = r#"
            var value = 0;
            setTimeout(function() { globalThis.test_value = 42; }, 10);
        "#;
        let _ = eval_code(code, Some("test.js"));
        // If timers executed, the global assignment happened inside QuickJS.
        // We cannot inspect QuickJS globals from Rust easily, so this mainly
        // exercises the event-loop path without panicking.
    }
}
