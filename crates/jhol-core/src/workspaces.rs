//! Workspace detection and listing (package.json "workspaces" field).

use std::path::{Path, PathBuf};

/// List workspace package roots from package.json in dir. Returns paths relative to dir.
/// Supports "workspaces": ["packages/*"] or "workspaces": { "packages": ["packages/*"] }.
pub fn list_workspace_roots(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let pj = dir.join("package.json");
    if !pj.exists() {
        return Ok(Vec::new());
    }
    let s = std::fs::read_to_string(&pj).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    let workspaces = v
        .get("workspaces")
        .and_then(|w| {
            if w.is_array() {
                w.as_array().map(|a| a.to_vec())
            } else {
                w.get("packages").and_then(|p| p.as_array()).map(|a| a.to_vec())
            }
        })
        .unwrap_or_default();
    let mut roots = Vec::new();
    for pattern in workspaces {
        let pattern = pattern.as_str().unwrap_or("").trim();
        if pattern.is_empty() {
            continue;
        }
        if pattern.contains('*') {
            let (prefix, _) = pattern.split_once('*').unwrap_or((pattern, ""));
            let prefix = prefix.trim_end_matches('/');
            let base = dir.join(prefix);
            if base.exists() {
                if let Ok(entries) = std::fs::read_dir(&base) {
                    for e in entries.flatten() {
                        let path = e.path();
                        if path.is_dir() && path.join("package.json").exists() {
                            roots.push(path.strip_prefix(dir).unwrap_or(&path).to_path_buf());
                        }
                    }
                }
            }
        } else if dir.join(pattern).join("package.json").exists() {
            roots.push(PathBuf::from(pattern));
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}
