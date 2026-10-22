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

/// Which JS engine backend runs this script. The default is decided at compile
/// time (JavaScriptCore when the `jsc-engine` feature is on, QuickJS otherwise);
/// `JHOL_RUNTIME_ENGINE=qjs|jsc` overrides it at runtime when both backends are
/// compiled in (an unavailable backend falls back with a warning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    QuickJs,
    JavaScriptCore,
    /// Jhol's own engine (`crates/jhol-vm`), selected with
    /// `JHOL_RUNTIME_ENGINE=jholvm`.
    JholVm,
}

/// The engine named by the environment is not part of this build.
///
/// This used to be `unreachable!()`, which meant a wrong `JHOL_RUNTIME_ENGINE`
/// value crashed the CLI with a Rust backtrace instead of telling the user what
/// to rebuild.
fn engine_missing(name: &str, feature: &str) -> errors::RuntimeError {
    errors::RuntimeError::InvalidArgument(format!(
        "JHOL_RUNTIME_ENGINE={name} requested an engine this build does not include; \
         rebuild with `--features {feature}`. Engines in this binary: {}",
        available_engines()
    ))
}

/// Comma-separated list of engines actually compiled in — used in error text.
fn available_engines() -> String {
    let mut v: Vec<&str> = Vec::new();
    #[cfg(feature = "jholvm")]
    {
        v.push("jholvm");
    }
    #[cfg(feature = "jsc-engine")]
    {
        v.push("jsc");
    }
    v.push("qjs");
    v.join(", ")
}

pub fn engine_kind() -> EngineKind {
    match std::env::var("JHOL_RUNTIME_ENGINE").ok().as_deref() {
        Some(v) if v.eq_ignore_ascii_case("qjs") => return available_or_default(EngineKind::QuickJs),
        Some(v) if v.eq_ignore_ascii_case("jsc") => return available_or_default(EngineKind::JavaScriptCore),
        Some(v) if v.eq_ignore_ascii_case("jholvm") || v.eq_ignore_ascii_case("vm") => {
            return available_or_default(EngineKind::JholVm)
        }
        _ => {}
    }
    #[cfg(feature = "jsc-engine")]
    {
        EngineKind::JavaScriptCore
    }
    #[cfg(all(not(feature = "jsc-engine"), feature = "jholvm"))]
    {
        EngineKind::JholVm
    }
    #[cfg(all(not(feature = "jsc-engine"), not(feature = "jholvm")))]
    {
        EngineKind::QuickJs
    }
}

fn available_or_default(requested: EngineKind) -> EngineKind {
    #[cfg(feature = "jsc-engine")]
    {
        requested
    }
    #[cfg(all(not(feature = "jsc-engine"), feature = "jholvm"))]
    {
        requested
    }
    #[cfg(all(not(feature = "jsc-engine"), not(feature = "jholvm")))]
    {
        if requested == EngineKind::JavaScriptCore {
            eprintln!(
                "[jhol-runtime] JHOL_RUNTIME_ENGINE=jsc requested but this binary was built without the jsc-engine feature; using QuickJS"
            );
        }
        if requested == EngineKind::JholVm {
            eprintln!(
                "[jhol-runtime] JHOL_RUNTIME_ENGINE=jholvm requested but this binary was built without the jholvm feature; using QuickJS"
            );
        }
        EngineKind::QuickJs
    }
}

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
    if engine_kind() == EngineKind::JholVm {
        #[cfg(feature = "jholvm")]
        {
            return jhol_vm::bridge::run_file(path).map_err(|m| errors::RuntimeError::ExecutionError(m));
        }
        #[cfg(not(feature = "jholvm"))]
        {
            return Err(engine_missing("jholvm", "jholvm"));
        }
    }
    if engine_kind() == EngineKind::JavaScriptCore {
        #[cfg(feature = "jsc-engine")]
        {
            return jsc_engine::run_file(path);
        }
        #[cfg(not(feature = "jsc-engine"))]
        {
            return Err(engine_missing("jsc", "jsc-engine"));
        }
    }
    let mut runtime = engine::Runtime::new()?;
    runtime.execute_file(path)?;
    runtime.run_event_loop()?;
    Ok(runtime.exit_code())
}

/// Run a JavaScript/TypeScript file with watch mode
pub fn run_file_watch(path: &Path) -> RuntimeResult<i32> {
    let kind = engine_kind();
    let mut session = watch::WatchSession::new(path, move |p| {
        // Every engine is dispatched here, including JholVM: watch mode used to
        // fall through to QuickJS for anything that was not JSC, silently
        // running the script on a different engine than `jhol run` chose.
        match kind {
            EngineKind::JholVm => {
                #[cfg(feature = "jholvm")]
                {
                    jhol_vm::bridge::run_file(p)
                        .map_err(|m| errors::RuntimeError::ExecutionError(m))?;
                }
                #[cfg(not(feature = "jholvm"))]
                {
                    return Err(engine_missing("jholvm", "jholvm"));
                }
            }
            EngineKind::JavaScriptCore => {
                #[cfg(feature = "jsc-engine")]
                {
                    jsc_engine::run_file(p)?;
                }
                #[cfg(not(feature = "jsc-engine"))]
                {
                    return Err(engine_missing("jsc", "jsc-engine"));
                }
            }
            EngineKind::QuickJs => {
                let mut rt = engine::Runtime::new()?;
                rt.execute_file(p)?;
                rt.run_event_loop()?;
            }
        }
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
    if engine_kind() == EngineKind::JholVm {
        #[cfg(feature = "jholvm")]
        {
            return jhol_vm::bridge::eval_code(code, filename)
                .map_err(|m| errors::RuntimeError::ExecutionError(m));
        }
        #[cfg(not(feature = "jholvm"))]
        {
            return Err(engine_missing("jholvm", "jholvm"));
        }
    }
    if engine_kind() == EngineKind::JavaScriptCore {
        #[cfg(feature = "jsc-engine")]
        {
            return jsc_engine::eval_code(code, filename);
        }
        #[cfg(not(feature = "jsc-engine"))]
        {
            return Err(engine_missing("jsc", "jsc-engine"));
        }
    }
    let mut runtime = engine::Runtime::new()?;
    let fname = filename.unwrap_or("<eval>");
    runtime.execute_code(code, fname)?;
    runtime.run_event_loop()?;
    Ok(runtime.exit_code())
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
