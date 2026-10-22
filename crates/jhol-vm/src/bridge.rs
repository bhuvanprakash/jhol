//! Bridge between JholVM and the `jhol` CLI.
//!
//! `jhol-runtime` owns the engine dispatch table; this module is the thin
//! adapter that runs a script and maps the engine's error type, so the engine
//! itself never has to know about the package manager.

use std::path::Path;

use crate::errors::VmError;
use crate::vm::{exit_code_of, Engine};

/// Decode an error, turning `process.exit(n)` into an exit code.
pub fn describe(e: &VmError) -> Result<i32, String> {
    if let VmError::Internal(m) = e {
        if let Some(code) = exit_code_of(m) {
            return Ok(code);
        }
    }
    let message = match e {
        VmError::Exception(t) => format!("uncaught exception ({:?})", t.value),
        VmError::Syntax(m) => format!("SyntaxError: {}", m),
        VmError::Internal(m) => m.clone(),
    };
    Err(message)
}

/// Run a script to completion, draining the timer queue.
pub fn run_file(path: &Path) -> Result<i32, String> {
    let mut engine = Engine::new();
    match engine.run_file(path) {
        Ok(code) => Ok(code),
        Err(e) => describe(&e),
    }
}

/// Evaluate a source string and return a process exit code.
pub fn eval_code(code: &str, filename: Option<&str>) -> Result<i32, String> {
    let mut engine = Engine::new();
    let name = filename.unwrap_or("<eval>");
    match engine.eval_source(code, name) {
        Ok(_) => Ok(engine.vm.exit_code),
        Err(e) => describe(&e),
    }
}
