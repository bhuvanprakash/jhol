//! # JholVM
//!
//! Jhol's own native JavaScript engine.
//!
//! ```text
//!   source ──lex──▶ tokens ──parse──▶ AST ──compile──▶ bytecode ──run──▶ VM
//! ```
//!
//! Every stage is hand-written and owned by this crate; nothing is embedded,
//! vendored or generated. The pieces that make it different from the engines it
//! shares a host with:
//!
//! * **Single-word values.** 64-bit NaN-boxed with a 4-bit subtag and a 44-bit
//!   offset into one 16 TiB arena (`value.rs`, `arena.rs`).
//! * **Shape-tagged property stores.** Property-addition histories intern into
//!   `u32` shape ids; object stores stay insertion-ordered vectors and only
//!   promote to a hash index past 12 keys.
//! * **Inline caches on loads and calls.** Every `GetProp`/`SetProp`/`GetGlobal`
//!   carries per-site state that cheapens the second call onwards — the thing
//!   QuickJS does not have at all.
//! * **Cell-based closures.** Captured locals are boxed once, so frames are
//!   plain `Value` arrays and no upvalue can dangle.

pub mod arena;
pub mod bridge;
pub mod ast;
pub mod bytecode;
pub mod builtins;
pub mod compiler;
pub mod errors;
pub mod formatter;
pub mod jit;
pub mod lexer;
pub mod objects;
pub mod parser;
pub mod value;
pub mod vm;

pub use errors::{VmError, VmResult};
pub use value::Value;

/// Version of the engine itself.
pub const JHOLVM_VERSION: &str = "1.0.2";

/// Env-gated stderr tracing, used to diagnose engine internals without a
/// debugger. `JHOL_VM_TRACE=1` turns it on.
#[doc(hidden)]
pub fn trace(msg: &str) {
    if std::env::var("JHOL_VM_TRACE").is_ok() {
        eprintln!("[trace] {}", msg);
    }
}

#[cfg(test)]
mod tests;
