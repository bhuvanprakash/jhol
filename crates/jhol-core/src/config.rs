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
