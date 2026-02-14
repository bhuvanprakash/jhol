//! Optional config from .jholrc or ~/.jholrc (JSON). Merged with env and CLI.

use std::path::Path;

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
        if key.ends_with(":_authToken") {
            out.auth_token = Some(value);
            continue;
        }
    }
    out
}

/// Load .npmrc from project then home, with project taking precedence.
pub fn load_npmrc(dir: &Path) -> NpmRcConfig {
    let home = dirs_home();
    let project = read_npmrc(&dir.join(".npmrc"));
    let home_cfg = home
        .as_ref()
        .map(|h| read_npmrc(&h.join(".npmrc")))
        .unwrap_or_default();
    NpmRcConfig {
        registry: project.registry.or(home_cfg.registry),
        auth_token: project.auth_token.or(home_cfg.auth_token),
    }
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
