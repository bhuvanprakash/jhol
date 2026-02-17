//! Optional config from .jholrc or ~/.jholrc (JSON). Merged with env and CLI.

use std::path::Path;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use crate::Backend;

/// Optional config from file. CLI and env override these.
#[derive(Default)]
pub struct Config {
    pub backend: Option<Backend>,
    pub cache_dir: Option<String>,
    pub offline: Option<bool>,
    pub frozen: Option<bool>,
}

/// Minimal .npmrc settings used by Jhol native registry paths.
#[derive(Default, Clone, Debug)]
pub struct NpmRcConfig {
    pub registry: Option<String>,
    pub auth_token: Option<String>,
    pub scoped_registries: HashMap<String, String>,
    pub auth_tokens_by_host_prefix: HashMap<String, String>,
    pub always_auth: Option<bool>,
    pub proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub strict_ssl: Option<bool>,
    pub cafile: Option<String>,
}

static NPMRC_CACHE: OnceLock<RwLock<HashMap<String, NpmRcConfig>>> = OnceLock::new();

fn npmrc_cache_key(dir: &Path) -> String {
    dir.to_string_lossy().to_string()
}

/// Load config from .jholrc in dir, then ~/.jholrc. Missing or invalid file = default.
pub fn load_config(dir: &Path) -> Config {
    let mut cfg = Config::default();
    let home = dirs_home();
    let candidates = [
        dir.join(".jholrc"),
        home.map(|h| h.join(".jholrc")).unwrap_or_else(|| dir.join(".none")),
    ];
    for path in &candidates {
        if path.is_file() {
            if let Ok(s) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(b) = v.get("backend").and_then(|x| x.as_str()) {
                        cfg.backend = match b {
                            "bun" => Some(Backend::Bun),
                            "npm" => Some(Backend::Npm),
                            _ => None,
                        };
                    }
                    if let Some(c) = v.get("cacheDir").and_then(|x| x.as_str()) {
                        cfg.cache_dir = Some(c.to_string());
                    }
                    if let Some(o) = v.get("offline").and_then(|x| x.as_bool()) {
                        cfg.offline = Some(o);
                    }
                    if let Some(f) = v.get("frozen").and_then(|x| x.as_bool()) {
                        cfg.frozen = Some(f);
                    }
                }
            }
            break;
        }
    }
    cfg
}

fn dirs_home() -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        std::env::var("HOME").ok().map(std::path::PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(std::path::PathBuf::from)
    }
}

fn read_npmrc(path: &Path) -> NpmRcConfig {
    let mut out = NpmRcConfig::default();
    let Ok(s) = std::fs::read_to_string(path) else {
        return out;
    };
    for raw in s.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let mut value = v.trim().to_string();
        if value.starts_with("${") && value.ends_with('}') && value.len() > 3 {
            let env_key = &value[2..value.len() - 1];
            if let Ok(env_val) = std::env::var(env_key) {
                value = env_val;
            }
        }
        if key == "registry" {
            out.registry = Some(value.trim_end_matches('/').to_string());
            continue;
        }
        if let Some(scope) = key.strip_suffix(":registry") {
            if scope.starts_with('@') && !scope.trim().is_empty() {
                out.scoped_registries
                    .insert(scope.to_string(), value.trim_end_matches('/').to_string());
            }
            continue;
        }
        if key.ends_with(":_authToken") {
            out.auth_token = Some(value.clone());
            out.auth_tokens_by_host_prefix.insert(
                key.trim_end_matches(":_authToken")
                    .trim_start_matches("//")
                    .trim_end_matches('/')
                    .to_string(),
                value,
            );
            continue;
        }
        if key == "always-auth" {
            out.always_auth = match value.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            continue;
        }
        if key == "proxy" {
            out.proxy = Some(value);
            continue;
        }
        if key == "https-proxy" || key == "https_proxy" {
            out.https_proxy = Some(value);
            continue;
        }
        if key == "noproxy" || key == "no-proxy" || key == "no_proxy" {
            out.no_proxy = Some(value);
            continue;
        }
        if key == "strict-ssl" {
            out.strict_ssl = match value.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            continue;
        }
        if key == "cafile" {
            out.cafile = Some(value);
            continue;
        }
    }
    out
}

/// Load .npmrc from project then home, with project taking precedence.
pub fn load_npmrc(dir: &Path) -> NpmRcConfig {
    let cache_key = npmrc_cache_key(dir);
    if let Some(cache) = NPMRC_CACHE.get() {
        if let Ok(guard) = cache.read() {
            if let Some(cached) = guard.get(&cache_key) {
                return cached.clone();
            }
        }
    }

    let home = dirs_home();
    let project = read_npmrc(&dir.join(".npmrc"));
    let home_cfg = home
        .as_ref()
        .map(|h| read_npmrc(&h.join(".npmrc")))
        .unwrap_or_default();
    let mut scoped_registries = home_cfg.scoped_registries;
    scoped_registries.extend(project.scoped_registries);

    let mut auth_tokens_by_host_prefix = home_cfg.auth_tokens_by_host_prefix;
    auth_tokens_by_host_prefix.extend(project.auth_tokens_by_host_prefix);

    let merged = NpmRcConfig {
        registry: project.registry.or(home_cfg.registry),
        auth_token: project.auth_token.or(home_cfg.auth_token),
        scoped_registries,
        auth_tokens_by_host_prefix,
        always_auth: project.always_auth.or(home_cfg.always_auth),
        proxy: project.proxy.or(home_cfg.proxy),
        https_proxy: project.https_proxy.or(home_cfg.https_proxy),
        no_proxy: project.no_proxy.or(home_cfg.no_proxy),
        strict_ssl: project.strict_ssl.or(home_cfg.strict_ssl),
        cafile: project.cafile.or(home_cfg.cafile),
    };

    let cache = NPMRC_CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Ok(mut guard) = cache.write() {
        guard.insert(cache_key, merged.clone());
    }

    merged
}

/// Effective registry URL used by native metadata/tarball flows.
pub fn effective_registry_url(dir: &Path) -> String {
    if let Ok(v) = std::env::var("NPM_CONFIG_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }
    if let Ok(v) = std::env::var("JHOL_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }
    load_npmrc(dir)
        .registry
        .unwrap_or_else(|| "https://registry.npmjs.org".to_string())
}

/// Resolve registry URL for a package, supporting scoped registry entries in .npmrc.
pub fn effective_registry_url_for_package(dir: &Path, package_name: &str) -> String {
    let cfg = load_npmrc(dir);
    if package_name.starts_with('@') {
        if let Some((scope, _)) = package_name.split_once('/') {
            if let Some(url) = cfg.scoped_registries.get(scope) {
                return url.trim_end_matches('/').to_string();
            }
        }
    }

    if let Ok(v) = std::env::var("NPM_CONFIG_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }
    if let Ok(v) = std::env::var("JHOL_REGISTRY") {
        if !v.trim().is_empty() {
            return v.trim().trim_end_matches('/').to_string();
        }
    }

    cfg.registry
        .unwrap_or_else(|| "https://registry.npmjs.org".to_string())
}

/// Best-effort auth token for registry API/tarball requests.
pub fn registry_auth_token(dir: &Path) -> Option<String> {
    if let Ok(v) = std::env::var("NODE_AUTH_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("NPM_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    load_npmrc(dir).auth_token
}

/// Resolve best auth token for a given registry/tarball URL using .npmrc host-specific tokens.
pub fn registry_auth_token_for_url(dir: &Path, url: &str) -> Option<String> {
    if let Ok(v) = std::env::var("NODE_AUTH_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("NPM_TOKEN") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }

    let cfg = load_npmrc(dir);
    let normalized = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');

    let mut best: Option<(usize, String)> = None;
    for (prefix, token) in &cfg.auth_tokens_by_host_prefix {
        let p = prefix.trim().trim_end_matches('/');
        if p.is_empty() {
            continue;
        }
        if normalized.starts_with(p) {
            let score = p.len();
            match &best {
                Some((best_score, _)) if *best_score >= score => {}
                _ => best = Some((score, token.clone())),
            }
        }
    }
    best.map(|(_, t)| t).or(cfg.auth_token)
}

/// Apply enterprise network-related npmrc settings to process env (best-effort).
pub fn apply_enterprise_network_env(dir: &Path) {
    let cfg = load_npmrc(dir);
    if let Some(v) = cfg.https_proxy {
        if std::env::var("HTTPS_PROXY").is_err() && std::env::var("https_proxy").is_err() {
            std::env::set_var("HTTPS_PROXY", v.clone());
            std::env::set_var("https_proxy", v);
        }
    }
    if let Some(v) = cfg.proxy {
        if std::env::var("HTTP_PROXY").is_err() && std::env::var("http_proxy").is_err() {
            std::env::set_var("HTTP_PROXY", v.clone());
            std::env::set_var("http_proxy", v);
        }
    }
    if let Some(v) = cfg.no_proxy {
        if std::env::var("NO_PROXY").is_err() && std::env::var("no_proxy").is_err() {
            std::env::set_var("NO_PROXY", v.clone());
            std::env::set_var("no_proxy", v);
        }
    }
    if matches!(cfg.strict_ssl, Some(false)) && std::env::var("NODE_TLS_REJECT_UNAUTHORIZED").is_err() {
        std::env::set_var("NODE_TLS_REJECT_UNAUTHORIZED", "0");
    }
    if let Some(v) = cfg.cafile {
        if std::env::var("SSL_CERT_FILE").is_err() {
            std::env::set_var("SSL_CERT_FILE", v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_npmrc_scoped_registry_and_token_resolution() {
        let tmp = std::env::temp_dir().join(format!(
            "jhol-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join(".npmrc"),
            "registry=https://registry.npmjs.org/\n@myco:registry=https://npm.myco.local/\n//npm.myco.local/:_authToken=abc123\n",
        )
        .unwrap();

        let cfg = load_npmrc(&tmp);
        assert_eq!(cfg.registry.as_deref(), Some("https://registry.npmjs.org"));
        assert_eq!(
            cfg.scoped_registries.get("@myco").map(String::as_str),
            Some("https://npm.myco.local")
        );

        let scoped = effective_registry_url_for_package(&tmp, "@myco/foo");
        assert_eq!(scoped, "https://npm.myco.local");

        assert_eq!(
            cfg.auth_tokens_by_host_prefix
                .get("npm.myco.local")
                .map(String::as_str),
            Some("abc123")
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
