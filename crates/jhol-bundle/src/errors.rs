use thiserror::Error;

#[derive(Error, Debug)]
pub enum BundleError {
    #[error("Parse error in {path}:{line}:{column} — {message}")]
    ParseError {
        path: String,
        line: usize,
        column: usize,
        message: String,
    },

    #[error("Module not found: {specifier} — resolved from {base}")]
    ModuleNotFound { specifier: String, base: String },

    #[error("Module resolution failed for {specifier}: {message}")]
    ModuleResolutionFailed { specifier: String, message: String },

    #[error("Transform error in {0}: {1}")]
    TransformError(String, String),

    #[error("Code generation error: {0}")]
    CodegenError(String),

    #[error("CSS error: {0}")]
    CssError(String),

    #[error("Source map error: {0}")]
    SourceMapError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Cycle detected: {0}")]
    CycleDetected(String),

    #[error("Plugin error: {0}")]
    PluginError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    Generic(String),
}

pub type BundleResult<T> = Result<T, BundleError>;
