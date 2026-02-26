//! Comprehensive error handling and logging for jhol.
//! Provides structured error types, logging utilities, and error recovery mechanisms.

use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Main error type for jhol operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JholError {
    /// I/O related errors
    Io {
        operation: String,
        path: Option<String>,
        source: String,
    },
    /// Network/HTTP errors
    Network {
        operation: String,
        url: Option<String>,
        status: Option<u16>,
        source: String,
    },
    /// Registry/package resolution errors
    Registry {
        operation: String,
        package: Option<String>,
        version: Option<String>,
        source: String,
    },
    /// Dependency resolution errors
    Resolution {
        operation: String,
        package: Option<String>,
        conflict_details: Option<String>,
        source: String,
    },
    /// Cache related errors
    Cache {
        operation: String,
        key: Option<String>,
        source: String,
    },
    /// Configuration errors
    Config {
        operation: String,
        field: Option<String>,
        source: String,
    },
    /// Security/permission errors
    Security {
        operation: String,
        path: Option<String>,
        reason: String,
    },
    /// Performance/timeout errors
    Performance {
        operation: String,
        duration: Option<u64>,
        limit: Option<u64>,
        source: String,
    },
    /// Generic application errors
    Application {
        operation: String,
        details: Option<String>,
        source: String,
    },
}

impl fmt::Display for JholError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JholError::Io { operation, path, source } => {
                write!(f, "I/O error in {}: {}", operation, source)?;
                if let Some(path) = path {
                    write!(f, " (path: {})", path)?;
                }
                Ok(())
            }
            JholError::Network { operation, url, status, source } => {
                write!(f, "Network error in {}: {}", operation, source)?;
                if let Some(url) = url {
                    write!(f, " (url: {})", url)?;
                }
                if let Some(status) = status {
                    write!(f, " (status: {})", status)?;
                }
                Ok(())
            }
            JholError::Registry { operation, package, version, source } => {
                write!(f, "Registry error in {}: {}", operation, source)?;
                if let Some(package) = package {
                    write!(f, " (package: {})", package)?;
                }
                if let Some(version) = version {
                    write!(f, " (version: {})", version)?;
                }
                Ok(())
            }
            JholError::Resolution { operation, package, conflict_details, source } => {
                write!(f, "Resolution error in {}: {}", operation, source)?;
                if let Some(package) = package {
                    write!(f, " (package: {})", package)?;
                }
                if let Some(details) = conflict_details {
                    write!(f, " (details: {})", details)?;
                }
                Ok(())
            }
            JholError::Cache { operation, key, source } => {
                write!(f, "Cache error in {}: {}", operation, source)?;
                if let Some(key) = key {
                    write!(f, " (key: {})", key)?;
                }
                Ok(())
            }
            JholError::Config { operation, field, source } => {
                write!(f, "Configuration error in {}: {}", operation, source)?;
                if let Some(field) = field {
                    write!(f, " (field: {})", field)?;
                }
                Ok(())
            }
            JholError::Security { operation, path, reason } => {
                write!(f, "Security error in {}: {} (reason: {})", operation, path.as_deref().unwrap_or("unknown"), reason)
            }
            JholError::Performance { operation, duration, limit, source } => {
                write!(f, "Performance error in {}: {}", operation, source)?;
                if let Some(duration) = duration {
                    write!(f, " (duration: {}ms)", duration)?;
                }
                if let Some(limit) = limit {
                    write!(f, " (limit: {}ms)", limit)?;
                }
                Ok(())
            }
            JholError::Application { operation, details, source } => {
                write!(f, "Application error in {}: {}", operation, source)?;
                if let Some(details) = details {
                    write!(f, " (details: {})", details)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for JholError {}

/// Error severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorSeverity {
    /// Informational - not actually an error
    Info,
    /// Warning - operation completed but with issues
    Warning,
    /// Error - operation failed but can be retried
    Error,
    /// Critical - operation failed and cannot be retried
    Critical,
}

/// Error context for enhanced error reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorContext {
    pub timestamp: u64,
    pub operation: String,
    pub duration: Option<u64>,
    pub retry_count: u32,
    pub user_action: Option<String>,
    pub related_packages: Vec<String>,
    pub system_info: Option<String>,
}

/// Performance monitoring and logging
pub struct PerformanceLogger {
    start_time: Instant,
    operation: String,
    context: ErrorContext,
}

impl PerformanceLogger {
    pub fn new(operation: &str) -> Self {
        Self {
            start_time: Instant::now(),
            operation: operation.to_string(),
            context: ErrorContext {
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                operation: operation.to_string(),
                duration: None,
                retry_count: 0,
                user_action: None,
                related_packages: Vec::new(),
                system_info: None,
            },
        }
    }

    pub fn add_package(&mut self, package: &str) {
        if !self.context.related_packages.contains(&package.to_string()) {
            self.context.related_packages.push(package.to_string());
        }
    }

    pub fn add_user_action(&mut self, action: &str) {
        self.context.user_action = Some(action.to_string());
    }

    pub fn add_system_info(&mut self, info: &str) {
        self.context.system_info = Some(info.to_string());
    }

    pub fn finish(mut self) -> u64 {
        let duration = self.start_time.elapsed().as_millis() as u64;
        self.context.duration = Some(duration);
        
        // Log performance metrics
        if duration > 5000 {
            crate::utils::log(&format!("WARNING: Slow operation detected: {} took {}ms", self.operation, duration));
        }
        
        duration
    }
}

/// Error recovery strategies
#[derive(Debug, Clone)]
pub enum RecoveryStrategy {
    /// Retry the operation with exponential backoff
    Retry { max_attempts: u32, backoff_factor: u64 },
    /// Fall back to an alternative implementation
    Fallback { alternative: String },
    /// Skip the operation and continue
    Skip { reason: String },
    /// Use cached data instead
    UseCache { cache_key: String },
    /// Manual intervention required
    ManualIntervention { instructions: String },
    /// Continue with partial results
    PartialResult { missing_items: Vec<String> },
    /// Degrade functionality gracefully
    DegradeFunctionality { degraded_features: Vec<String> },
}

/// Error handler with recovery capabilities
pub struct ErrorHandler {
    max_retries: u32,
    retry_delay: u64,
    enable_logging: bool,
}

impl ErrorHandler {
    pub fn new() -> Self {
        Self {
            max_retries: 3,
            retry_delay: 1000, // 1 second
            enable_logging: true,
        }
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_delay(mut self, delay_ms: u64) -> Self {
        self.retry_delay = delay_ms;
        self
    }

    pub fn with_logging(mut self, enable: bool) -> Self {
        self.enable_logging = enable;
        self
    }

    /// Handle an error with appropriate recovery strategy
    pub fn handle_error(&self, error: JholError, strategy: RecoveryStrategy) -> Result<(), JholError> {
        if self.enable_logging {
            self.log_error(&error, ErrorSeverity::Error);
        }

        match strategy {
            RecoveryStrategy::Retry { max_attempts, backoff_factor } => {
                self.retry_with_backoff(error, max_attempts, backoff_factor)
            }
            RecoveryStrategy::Fallback { alternative } => {
                if self.enable_logging {
                    crate::utils::log(&format!("Falling back to alternative: {}", alternative));
                }
                Ok(())
            }
            RecoveryStrategy::Skip { reason } => {
                if self.enable_logging {
                    crate::utils::log(&format!("Skipping operation: {}", reason));
                }
                Ok(())
            }
            RecoveryStrategy::UseCache { cache_key } => {
                if self.enable_logging {
                    crate::utils::log(&format!("Using cached data for key: {}", cache_key));
                }
                Ok(())
            }
            RecoveryStrategy::ManualIntervention { instructions } => {
                Err(JholError::Application {
                    operation: "manual_intervention".to_string(),
                    details: Some(instructions),
                    source: "Manual intervention required".to_string(),
                })
            }
            RecoveryStrategy::PartialResult { missing_items } => {
                if self.enable_logging {
                    crate::utils::log(&format!("Continuing with partial results, missing: {:?}", missing_items));
                }
                Ok(())
            }
            RecoveryStrategy::DegradeFunctionality { degraded_features } => {
                if self.enable_logging {
                    crate::utils::log(&format!("Degrading functionality, disabled features: {:?}", degraded_features));
                }
                Ok(())
            }
        }
    }

    /// Retry operation with exponential backoff
    fn retry_with_backoff(
        &self,
        error: JholError,
        max_attempts: u32,
        backoff_factor: u64,
    ) -> Result<(), JholError> {
        for attempt in 1..=max_attempts {
            if self.enable_logging {
                crate::utils::log(&format!("Retry attempt {} for error: {}", attempt, error));
            }

            // Exponential backoff delay with jitter to avoid thundering herd
            if attempt > 1 {
                let base_delay = backoff_factor * (2_u64.pow(attempt - 2));
                let jitter = base_delay / 4; // Add up to 25% jitter
                let random_jitter = if jitter > 0 {
                    rand::random::<u64>() % (jitter * 2)
                } else {
                    0
                };
                let delay = base_delay + random_jitter;
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }

            // In a real implementation, this would retry the actual operation
            // For now, we'll just return the original error to demonstrate the pattern
            if attempt == max_attempts {
                return Err(error);
            }
        }

        Ok(())
    }

    /// Log error with appropriate severity
    fn log_error(&self, error: &JholError, severity: ErrorSeverity) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let log_entry = format!(
            "[{}] {:?}: {}",
            timestamp,
            severity,
            error
        );

        match severity {
            ErrorSeverity::Info => crate::utils::log(&log_entry),
            ErrorSeverity::Warning => crate::utils::log(&format!("WARNING: {}", log_entry)),
            ErrorSeverity::Error => crate::utils::log(&format!("ERROR: {}", log_entry)),
            ErrorSeverity::Critical => crate::utils::log(&format!("CRITICAL: {}", log_entry)),
        }
    }

    /// Create a performance logger for an operation
    pub fn start_performance_logging(&self, operation: &str) -> PerformanceLogger {
        PerformanceLogger::new(operation)
    }
}

/// Utility functions for common error patterns
pub mod utils {
    use super::*;

    /// Convert std::io::Error to JholError
    pub fn io_error(operation: &str, path: Option<&str>, source: std::io::Error) -> JholError {
        JholError::Io {
            operation: operation.to_string(),
            path: path.map(String::from),
            source: source.to_string(),
        }
    }

    /// Convert network error to JholError
    pub fn network_error(operation: &str, url: Option<&str>, source: String) -> JholError {
        JholError::Network {
            operation: operation.to_string(),
            url: url.map(String::from),
            status: None,
            source,
        }
    }

    /// Create a resolution error with conflict details
    pub fn resolution_error(
        operation: &str,
        package: Option<&str>,
        conflict_details: Option<&str>,
        source: &str,
    ) -> JholError {
        JholError::Resolution {
            operation: operation.to_string(),
            package: package.map(String::from),
            conflict_details: conflict_details.map(String::from),
            source: source.to_string(),
        }
    }

    /// Create a timeout error
    pub fn timeout_error(operation: &str, duration: u64, limit: u64) -> JholError {
        JholError::Performance {
            operation: operation.to_string(),
            duration: Some(duration),
            limit: Some(limit),
            source: "Operation timed out".to_string(),
        }
    }

    /// Create a security error
    pub fn security_error(operation: &str, path: Option<&str>, reason: &str) -> JholError {
        JholError::Security {
            operation: operation.to_string(),
            path: path.map(String::from),
            reason: reason.to_string(),
        }
    }

    /// Create a cache error
    pub fn cache_error(operation: &str, key: Option<&str>, source: &str) -> JholError {
        JholError::Cache {
            operation: operation.to_string(),
            key: key.map(String::from),
            source: source.to_string(),
        }
    }

    /// Create a config error
    pub fn config_error(operation: &str, field: Option<&str>, source: &str) -> JholError {
        JholError::Config {
            operation: operation.to_string(),
            field: field.map(String::from),
            source: source.to_string(),
        }
    }

    /// Create a registry error with package context
    pub fn registry_error_with_package(operation: &str, package: &str, version: Option<&str>, source: &str) -> JholError {
        JholError::Registry {
            operation: operation.to_string(),
            package: Some(package.to_string()),
            version: version.map(String::from),
            source: source.to_string(),
        }
    }

    /// Create a network error with status code
    pub fn network_error_with_status(operation: &str, url: Option<&str>, status: u16, source: &str) -> JholError {
        JholError::Network {
            operation: operation.to_string(),
            url: url.map(String::from),
            status: Some(status),
            source: source.to_string(),
        }
    }

    /// Create a resolution error with detailed conflict information
    pub fn detailed_resolution_error(operation: &str, package: &str, conflict_details: &str, source: &str) -> JholError {
        JholError::Resolution {
            operation: operation.to_string(),
            package: Some(package.to_string()),
            conflict_details: Some(conflict_details.to_string()),
            source: source.to_string(),
        }
    }

    /// Create an application error
    pub fn application_error(operation: &str, field: Option<&str>, source: &str) -> JholError {
        JholError::Application {
            operation: operation.to_string(),
            details: field.map(String::from),
            source: source.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let error = JholError::Io {
            operation: "read_file".to_string(),
            path: Some("/path/to/file".to_string()),
            source: "Permission denied".to_string(),
        };
        
        let display = format!("{}", error);
        assert!(display.contains("read_file"));
        assert!(display.contains("Permission denied"));
        assert!(display.contains("/path/to/file"));
    }

    #[test]
    fn test_performance_logger() {
        let mut logger = PerformanceLogger::new("test_operation");
        logger.add_package("test-package");
        logger.add_user_action("install");
        
        let duration = logger.finish();
        assert!(duration > 0);
    }

    #[test]
    fn test_error_handler() {
        let handler = ErrorHandler::new()
            .with_max_retries(2)
            .with_logging(false);

        let error = JholError::Network {
            operation: "fetch".to_string(),
            url: Some("https://example.com".to_string()),
            status: Some(404),
            source: "Not Found".to_string(),
        };

        let result = handler.handle_error(
            error,
            RecoveryStrategy::Skip { reason: "Package not found".to_string() }
        );
        
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_recovery_strategies() {
        let handler = ErrorHandler::new().with_logging(false);

        // Test PartialResult strategy
        let error = JholError::Registry {
            operation: "resolve".to_string(),
            package: Some("missing-package".to_string()),
            version: None,
            source: "Package not found".to_string(),
        };

        let result = handler.handle_error(
            error,
            RecoveryStrategy::PartialResult { missing_items: vec!["missing-package".to_string()] }
        );
        assert!(result.is_ok());

        // Test DegradeFunctionality strategy
        let error = JholError::Cache {
            operation: "read_cache".to_string(),
            key: Some("corrupted_cache".to_string()),
            source: "Cache corrupted".to_string(),
        };

        let result = handler.handle_error(
            error,
            RecoveryStrategy::DegradeFunctionality { degraded_features: vec!["caching".to_string()] }
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_error_utils() {
        // Test cache_error
        let cache_err = utils::cache_error("read_cache", Some("test_key"), "Cache miss");
        match cache_err {
            JholError::Cache { operation, key, source } => {
                assert_eq!(operation, "read_cache");
                assert_eq!(key, Some("test_key".to_string()));
                assert_eq!(source, "Cache miss");
            }
            _ => panic!("Expected Cache error"),
        }

        // Test registry_error_with_package
        let registry_err = utils::registry_error_with_package("resolve", "test-pkg", Some("1.0.0"), "Not found");
        match registry_err {
            JholError::Registry { operation, package, version, source } => {
                assert_eq!(operation, "resolve");
                assert_eq!(package, Some("test-pkg".to_string()));
                assert_eq!(version, Some("1.0.0".to_string()));
                assert_eq!(source, "Not found");
            }
            _ => panic!("Expected Registry error"),
        }

        // Test detailed_resolution_error
        let resolution_err = utils::detailed_resolution_error("install", "conflicted-pkg", "Version conflict: needs 2.0.0, found 1.0.0", "Conflict");
        match resolution_err {
            JholError::Resolution { operation, package, conflict_details, source } => {
                assert_eq!(operation, "install");
                assert_eq!(package, Some("conflicted-pkg".to_string()));
                assert_eq!(conflict_details, Some("Version conflict: needs 2.0.0, found 1.0.0".to_string()));
                assert_eq!(source, "Conflict");
            }
            _ => panic!("Expected Resolution error"),
        }
    }

    #[test]
    fn test_error_display_formatting() {
        // Test that all error variants display properly
        let errors = vec![
            utils::io_error("read_file", Some("/test/path"), std::io::Error::new(std::io::ErrorKind::NotFound, "File not found")),
            utils::network_error_with_status("fetch", Some("https://example.com"), 404, "Not Found"),
            utils::registry_error_with_package("resolve", "test-pkg", Some("1.0.0"), "Not found"),
            utils::detailed_resolution_error("install", "conflict-pkg", "Version mismatch", "Conflict"),
            utils::cache_error("read_cache", Some("cache_key"), "Cache miss"),
            utils::config_error("load_config", Some("registry"), "Invalid URL"),
            utils::security_error("verify_integrity", Some("/package.tgz"), "Checksum mismatch"),
            utils::timeout_error("download", 10000, 5000),
        ];

        for error in errors {
            let display = format!("{}", error);
            assert!(!display.is_empty(), "Error display should not be empty");
            // Basic check that the display contains expected elements
            assert!(display.contains(":"), "Error display should contain colon separator");
        }
    }
}