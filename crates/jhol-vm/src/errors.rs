//! Errors produced by the JholVM pipeline (lex → parse → compile → run).

use crate::value::Value;

/// A JavaScript value in flight as a `throw`.
#[derive(Debug, Clone)]
pub struct Thrown {
    pub value: Value,
    pub stack: Vec<String>,
}

impl Thrown {
    pub fn new(value: Value) -> Self {
        Thrown {
            value,
            stack: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum VmError {
    #[error("SyntaxError: {0}")]
    Syntax(String),
    #[error("InternalError: {0}")]
    Internal(String),

    /// A JS-level `throw` carrying an arbitrary value.
    #[error("uncaught exception")]
    Exception(#[from] Box<Thrown>),
}

pub type VmResult<T> = Result<T, VmError>;

impl std::fmt::Display for Thrown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "uncaught exception")
    }
}

impl std::error::Error for Thrown {}

impl From<Thrown> for VmError {
    fn from(t: Thrown) -> Self {
        VmError::Exception(Box::new(t))
    }
}

/// Helper used by builtins to throw a plain `Error`-ish string value.
pub fn throw_string(kind: &str, msg: &str) -> VmError {
    VmError::Exception(Box::new(Thrown {
        value: Value::from_str_owned(format!("{}: {}", kind, msg)),
        stack: Vec::new(),
    }))
}

/// `TypeError: msg` as a thrown JS string value.
pub fn type_error(msg: &str) -> VmError {
    VmError::Exception(Box::new(Thrown {
        value: Value::from_str_owned(format!("TypeError: {}", msg)),
        stack: Vec::new(),
    }))
}

/// `SyntaxError` — used by `JSON.parse` and the parser, so a caught parse
/// failure reports the same name node does. It used to be raised as a
/// `TypeError`, which is a different error class for callers that branch on it.
pub fn syntax_error(msg: &str) -> VmError {
    VmError::Exception(Box::new(Thrown {
        value: Value::from_str_owned(format!("SyntaxError: {}", msg)),
        stack: Vec::new(),
    }))
}

pub fn range_error(msg: &str) -> VmError {
    VmError::Exception(Box::new(Thrown {
        value: Value::from_str_owned(format!("RangeError: {}", msg)),
        stack: Vec::new(),
    }))
}

pub fn reference_error(msg: &str) -> VmError {
    VmError::Exception(Box::new(Thrown {
        value: Value::from_str_owned(format!("ReferenceError: {}", msg)),
        stack: Vec::new(),
    }))
}
