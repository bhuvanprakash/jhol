use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("JS parse error: {0}")]
    ParseError(String),

    #[error("JS execution error: {0}")]
    ExecutionError(String),

    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    #[error("Module resolution failed: {0} — searched in {1:?}")]
    ModuleResolutionFailed(String, Vec<PathBuf>),

    #[error("TypeScript transpilation failed: {0}")]
    TranspileError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Unsupported file type: {0}")]
    UnsupportedFileType(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    Generic(String),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;
