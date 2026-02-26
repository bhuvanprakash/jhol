//! Enterprise configuration: proxy, SSL, authentication, and policy settings.
//! Supports .npmrc parsing, environment variables, and enterprise SSO integration points.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::env;
use std::fs;
use serde::{Deserialize, Serialize};

/// Enterprise configuration loaded from .npmrc, .jholrc, or environment
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnterpriseConfig {
    /// HTTP proxy URL (e.g. http://proxy.company.com:8080)
    pub proxy: Option<String>,
    /// HTTPS proxy URL
    pub https_proxy: Option<String>,
    /// Comma-separated list of hosts to bypass proxy
    pub no_proxy: Option<String>,
    /// Path to custom CA certificate bundle
    pub ca_cert_path: Option<String>,
    /// Skip SSL certificate verification (NOT recommended for production)
    pub strict_ssl: bool,
    /// Registry URL (can be private npm registry)
    pub registry: Option<String>,
    /// Authentication token for registry
    pub auth_token: Option<String>,
    /// Username for basic auth
    pub username: Option<String>,
    /// Password for basic auth
    pub password: Option<String>,
    /// SSO type (e.g. "saml", "oauth", "oidc")
    pub sso_type: Option<String>,
    /// SSO provider URL
    pub sso_provider_url: Option<String>,
    /// Package allowlist (only these packages can be installed)
    pub allowlist: Option<Vec<String>>,
    /// Package blocklist (these packages cannot be installed)
    pub blocklist: Option<Vec<String>>,
    /// License policy (e.g. "permissive", "copyleft-allowed", "proprietary-allowed")
    pub license_policy: Option<String>,
    /// Audit policy severity level (e.g. "critical", "high", "medium", "low")
    pub audit_severity: Option<String>,
    /// Offline mirror directory
    pub offline_mirror: Option<String>,
    /// Enable verbose enterprise logging
    pub verbose_logging: bool,
    /// Log directory for enterprise audits
    pub log_dir: Option<String>,
}

impl EnterpriseConfig {
    /// Load enterprise config from .npmrc, .jholrc, and environment
    pub fn load(project_root: &Path) -> Self {
        let mut config = Self::default();
        
        // Load from .npmrc
        if let Some(npmrc_config) = Self::load_npmrc(project_root) {
            config.merge(npmrc_config);
        }
        
        // Load from .jholrc
        if let Some(jholrc_config) = Self::load_jholrc(project_root) {
            config.merge(jholrc_config);
        }
        
        // Override with environment variables
        config.load_from_env();
        
        config
    }
    
    /// Load from .npmrc file
    fn load_npmrc(project_root: &Path) -> Option<Self> {
        let npmrc_path = project_root.join(".npmrc");
        if !npmrc_path.exists() {
            return None;
        }
        
        let content = fs::read_to_string(&npmrc_path).ok()?;
        let mut config = Self::default();
        
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                
                match key {
                    "proxy" | "http-proxy" => config.proxy = Some(value.to_string()),
                    "https-proxy" => config.https_proxy = Some(value.to_string()),
                    "no-proxy" => config.no_proxy = Some(value.to_string()),
                    "ca" | "cafile" => config.ca_cert_path = Some(value.to_string()),
                    "strict-ssl" => config.strict_ssl = value.parse().unwrap_or(true),
                    "registry" => config.registry = Some(value.to_string()),
                    "//registry.npmjs.org/:_authToken" | "_authToken" => {
                        config.auth_token = Some(value.to_string());
                    }
                    "username" | "_username" => config.username = Some(value.to_string()),
                    "_password" => {
                        // .npmrc passwords are often base64 encoded
                        config.password = Some(value.to_string());
                    }
                    "sso-type" => config.sso_type = Some(value.to_string()),
                    "sso-provider" => config.sso_provider_url = Some(value.to_string()),
                    "offline-cache" | "offline-mirror" => {
                        config.offline_mirror = Some(value.to_string());
                    }
                    _ => {}
                }
            }
        }
        
        Some(config)
    }
    
    /// Load from .jholrc file (JSON format)
    fn load_jholrc(project_root: &Path) -> Option<Self> {
        let jholrc_path = project_root.join(".jholrc");
        if !jholrc_path.exists() {
            return None;
        }
        
        let content = fs::read_to_string(&jholrc_path).ok()?;
        serde_json::from_str(&content).ok()
    }
    
    /// Override config with environment variables
    fn load_from_env(&mut self) {
        if let Ok(val) = env::var("HTTP_PROXY") {
            self.proxy = Some(val);
        }
        if let Ok(val) = env::var("HTTPS_PROXY") {
            self.https_proxy = Some(val);
        }
        if let Ok(val) = env::var("NO_PROXY") {
            self.no_proxy = Some(val);
        }
        if let Ok(val) = env::var("JHOL_CA_CERT_PATH") {
            self.ca_cert_path = Some(val);
        }
        if let Ok(val) = env::var("JHOL_STRICT_SSL") {
            self.strict_ssl = val.parse().unwrap_or(true);
        }
        if let Ok(val) = env::var("JHOL_REGISTRY") {
            self.registry = Some(val);
        }
        if let Ok(val) = env::var("JHOL_AUTH_TOKEN") {
            self.auth_token = Some(val);
        }
        if let Ok(val) = env::var("JHOL_USERNAME") {
            self.username = Some(val);
        }
        if let Ok(val) = env::var("JHOL_PASSWORD") {
            self.password = Some(val);
        }
        if let Ok(val) = env::var("JHOL_SSO_TYPE") {
            self.sso_type = Some(val);
        }
        if let Ok(val) = env::var("JHOL_SSO_PROVIDER_URL") {
            self.sso_provider_url = Some(val);
        }
        if let Ok(val) = env::var("JHOL_OFFLINE_MIRROR") {
            self.offline_mirror = Some(val);
        }
        if let Ok(val) = env::var("JHOL_ALLOWLIST") {
            self.allowlist = Some(val.split(',').map(|s| s.trim().to_string()).collect());
        }
        if let Ok(val) = env::var("JHOL_BLOCKLIST") {
            self.blocklist = Some(val.split(',').map(|s| s.trim().to_string()).collect());
        }
        if let Ok(val) = env::var("JHOL_LICENSE_POLICY") {
            self.license_policy = Some(val);
        }
        if let Ok(val) = env::var("JHOL_AUDIT_SEVERITY") {
            self.audit_severity = Some(val);
        }
        if let Ok(val) = env::var("JHOL_LOG_DIR") {
            self.log_dir = Some(val);
        }
        if let Ok(val) = env::var("JHOL_VERBOSE_LOGGING") {
            self.verbose_logging = val.parse().unwrap_or(false);
        }
    }
    
    /// Merge another config into this one (other takes precedence)
    fn merge(&mut self, other: Self) {
        if other.proxy.is_some() { self.proxy = other.proxy; }
        if other.https_proxy.is_some() { self.https_proxy = other.https_proxy; }
        if other.no_proxy.is_some() { self.no_proxy = other.no_proxy; }
        if other.ca_cert_path.is_some() { self.ca_cert_path = other.ca_cert_path; }
        self.strict_ssl = other.strict_ssl;
        if other.registry.is_some() { self.registry = other.registry; }
        if other.auth_token.is_some() { self.auth_token = other.auth_token; }
        if other.username.is_some() { self.username = other.username; }
        if other.password.is_some() { self.password = other.password; }
        if other.sso_type.is_some() { self.sso_type = other.sso_type; }
        if other.sso_provider_url.is_some() { self.sso_provider_url = other.sso_provider_url; }
        if other.allowlist.is_some() { self.allowlist = other.allowlist; }
        if other.blocklist.is_some() { self.blocklist = other.blocklist; }
        if other.license_policy.is_some() { self.license_policy = other.license_policy; }
        if other.audit_severity.is_some() { self.audit_severity = other.audit_severity; }
        if other.offline_mirror.is_some() { self.offline_mirror = other.offline_mirror; }
        self.verbose_logging = other.verbose_logging;
        if other.log_dir.is_some() { self.log_dir = other.log_dir; }
    }
    
    /// Check if a package is allowed by policy
    pub fn is_package_allowed(&self, package_name: &str) -> bool {
        // Check blocklist first
        if let Some(blocklist) = &self.blocklist {
            if blocklist.iter().any(|blocked| {
                blocked == package_name || 
                blocked.ends_with("/*") && package_name.starts_with(&blocked[..blocked.len()-2])
            }) {
                return false;
            }
        }
        
        // Check allowlist
        if let Some(allowlist) = &self.allowlist {
            if !allowlist.is_empty() {
                return allowlist.iter().any(|allowed| {
                    allowed == package_name ||
                    allowed.ends_with("/*") && package_name.starts_with(&allowed[..allowed.len()-2])
                });
            }
        }
        
        true
    }
    
    /// Get proxy URL for a given URL
    pub fn get_proxy_for_url(&self, url: &str) -> Option<&str> {
        // Check no_proxy
        if let Some(no_proxy) = &self.no_proxy {
            for host in no_proxy.split(',') {
                let host = host.trim();
                if url.contains(host) {
                    return None; // Don't use proxy for this host
                }
            }
        }
        
        // Return appropriate proxy based on URL scheme
        if url.starts_with("https://") {
            self.https_proxy.as_deref().or(self.proxy.as_deref())
        } else {
            self.proxy.as_deref()
        }
    }
}

/// SSO Token manager for enterprise authentication
pub struct SsoTokenManager {
    token: Option<String>,
    expires_at: Option<u64>,
    token_source: Option<String>,
}

impl SsoTokenManager {
    pub fn new() -> Self {
        Self {
            token: None,
            expires_at: None,
            token_source: None,
        }
    }
    
    /// Load SSO token from standard locations
    pub fn load_token(&mut self) -> Option<&str> {
        // Try environment variable first
        if let Ok(token) = env::var("JHOL_SSO_TOKEN") {
            self.token = Some(token);
            self.token_source = Some("env".to_string());
            return self.token.as_deref();
        }
        
        // Try .jhol-ssotoken file
        if let Ok(token) = fs::read_to_string(".jhol-ssotoken") {
            self.token = Some(token.trim().to_string());
            self.token_source = Some("file".to_string());
            return self.token.as_deref();
        }
        
        None
    }
    
    /// Set a new token (e.g., after SSO flow)
    pub fn set_token(&mut self, token: String, expires_in_seconds: Option<u64>) {
        self.token = Some(token);
        self.expires_at = expires_in_seconds.map(|secs| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() + secs
        });
    }
    
    /// Check if token is expired
    pub fn is_token_expired(&self) -> bool {
        self.expires_at.map_or(false, |expires| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() >= expires
        })
    }
    
    /// Get current token if valid
    pub fn get_valid_token(&mut self) -> Option<&str> {
        if self.is_token_expired() {
            self.token = None;
            return None;
        }
        self.token.as_deref()
    }
}

impl Default for SsoTokenManager {
    fn default() -> Self {
        Self::new()
    }
}

/// License checker for enterprise compliance
pub struct LicenseChecker {
    policy: String,
    allowed_licenses: Vec<String>,
}

impl LicenseChecker {
    pub fn new(policy: &str) -> Self {
        let allowed = match policy {
            "permissive" => vec!["MIT", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0", "ISC"],
            "copyleft-allowed" => vec!["MIT", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0", "ISC", "GPL-3.0", "LGPL-3.0", "AGPL-3.0"],
            "proprietary-allowed" => vec!["MIT", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0", "ISC", "Proprietary", "UNLICENSED"],
            _ => vec![], // Custom policy - empty means check against allowlist
        };
        
        Self {
            policy: policy.to_string(),
            allowed_licenses: allowed.iter().map(|s| s.to_string()).collect(),
        }
    }
    
    /// Check if a license is compliant with policy
    pub fn is_license_compliant(&self, license: &str) -> bool {
        if self.allowed_licenses.is_empty() {
            return true; // Custom policy with no restrictions
        }
        
        self.allowed_licenses.iter().any(|allowed| {
            license == allowed || license.starts_with(&format!("{}-", allowed))
        })
    }
    
    /// Get list of allowed licenses
    pub fn get_allowed_licenses(&self) -> &[String] {
        &self.allowed_licenses
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_enterprise_config_allowlist() {
        let mut config = EnterpriseConfig::default();
        config.allowlist = Some(vec!["lodash".to_string(), "@babel/*".to_string()]);
        
        assert!(config.is_package_allowed("lodash"));
        assert!(config.is_package_allowed("@babel/core"));
        assert!(!config.is_package_allowed("react"));
        assert!(!config.is_package_allowed("@types/node"));
    }
    
    #[test]
    fn test_enterprise_config_blocklist() {
        let mut config = EnterpriseConfig::default();
        config.blocklist = Some(vec!["malicious-pkg".to_string(), "unsafe/*".to_string()]);
        
        assert!(!config.is_package_allowed("malicious-pkg"));
        assert!(!config.is_package_allowed("unsafe/lib"));
        assert!(config.is_package_allowed("lodash"));
    }
    
    #[test]
    fn test_license_checker_permissive() {
        let checker = LicenseChecker::new("permissive");
        assert!(checker.is_license_compliant("MIT"));
        assert!(checker.is_license_compliant("Apache-2.0"));
        assert!(!checker.is_license_compliant("GPL-3.0"));
    }
}
