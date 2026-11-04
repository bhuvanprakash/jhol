//! `jhol publish` — the registry write path.
//!
//! Packs the directory with the deterministic packer (`pack.rs`), builds a
//! CouchDB-style packument (the protocol the npm registry ecosystem speaks:
//! `PUT {registry}/{name}` with `versions`, `dist-tags`, and the tarball as
//! a base64 `_attachments` entry), and sends it with bearer auth resolved the
//! same way installs resolve it (`registry_auth_token_for_url`).
//!
//! Nothing here is copied from any client implementation: the wire shape is
//! the published npm registry protocol, and the tarball itself is produced by
//! our own deterministic packer.

use base64::Engine as _;
use std::path::Path;

/// What a publish did (or would do, in a dry run).
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub name: String,
    pub version: String,
    pub registry: String,
    pub tarball_bytes: usize,
    pub sha256: String,
    pub dry_run: bool,
}

/// Basename used inside tarball paths: `@scope/pkg` -> `pkg` (npm convention
/// for `dist.tarball` URLs: `{registry}/{name}/-/{basename}-{version}.tgz`).
fn tarball_basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Publish the package at `dir` to `registry`.
///
/// * `token` — bearer token; when `None` it is resolved from the environment
///   / `.npmrc` exactly as installs resolve it.
/// * `dry_run` — build the full request and return it without sending.
pub fn publish(
    dir: &Path,
    registry: &str,
    token: Option<&str>,
    dry_run: bool,
) -> Result<PublishOutcome, String> {
    let registry = registry.trim_end_matches('/');
    let pj_path = dir.join("package.json");
    let pj: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&pj_path).map_err(|e| format!("read package.json: {}", e))?,
    )
    .map_err(|e| format!("parse package.json: {}", e))?;
    let name = pj
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or("package.json has no name — nothing to publish")?
        .to_string();
    let version = pj
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or("package.json has no version — bump it first")?
        .to_string();
    if name.starts_with('.') || name.contains('\0') {
        return Err(format!("refusing to publish suspicious name {:?}", name));
    }

    let (tgz, label, _files) = crate::pack::pack_bytes(dir)?;
    let sha256 = label
        .split_once(':')
        .map(|(_, h)| h.to_string())
        .unwrap_or_default();
    let integrity = crate::registry::sri_of_bytes(&tgz);
    let base = tarball_basename(&name);
    let filename = format!("{}-{}.tgz", base, version);
    let tarball_url = format!("{}/{}/-/{}", registry, name, filename);

    // Consumer-relevant manifest fields pass through so the resolver sees the
    // same graph installs see (mirrors what build_manifest_synth preserves).
    let mut ver = serde_json::Map::new();
    ver.insert("name".into(), serde_json::Value::String(name.clone()));
    ver.insert("version".into(), serde_json::Value::String(version.clone()));
    for key in [
        "dependencies",
        "optionalDependencies",
        "peerDependencies",
        "peerDependenciesMeta",
        "os",
        "cpu",
        "libc",
        "engines",
        "bin",
        "main",
        "exports",
        "types",
        "type",
    ] {
        if let Some(v) = pj.get(key) {
            ver.insert(key.into(), v.clone());
        }
    }
    ver.insert(
        "dist".into(),
        serde_json::json!({
            "tarball": tarball_url,
            "integrity": integrity,
        }),
    );

    let mut versions = serde_json::Map::new();
    versions.insert(version.clone(), serde_json::Value::Object(ver));
    let mut dist_tags = serde_json::Map::new();
    dist_tags.insert("latest".into(), serde_json::Value::String(version.clone()));

    let mut attachment = serde_json::Map::new();
    attachment.insert(
        "content_type".into(),
        serde_json::Value::String("application/octet-stream".into()),
    );
    attachment.insert(
        "data".into(),
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(&tgz)),
    );
    let mut attachments = serde_json::Map::new();
    attachments.insert(filename.clone(), serde_json::Value::Object(attachment));

    let mut body = serde_json::Map::new();
    body.insert("_id".into(), serde_json::Value::String(name.clone()));
    body.insert("name".into(), serde_json::Value::String(name.clone()));
    body.insert("versions".into(), serde_json::Value::Object(versions));
    body.insert("dist-tags".into(), serde_json::Value::Object(dist_tags));
    body.insert(
        "_attachments".into(),
        serde_json::Value::Object(attachments),
    );
    let body = serde_json::to_vec(&serde_json::Value::Object(body)).map_err(|e| e.to_string())?;

    let outcome = PublishOutcome {
        name: name.clone(),
        version,
        registry: registry.to_string(),
        tarball_bytes: tgz.len(),
        sha256,
        dry_run,
    };
    if dry_run {
        return Ok(outcome);
    }

    let token = token
        .map(|t| t.to_string())
        .or_else(|| crate::config::registry_auth_token_for_url(dir, registry));
    let url = format!("{}/{}", registry, name.replace('/', "%2F"));
    let client = crate::http_client::HttpClient::new(4);
    client.put_json_with_bearer(&url, &body, token.as_deref())?;
    Ok(outcome)
}

/// Upsert `//{host}/:_authToken={token}` into the npmrc at `dir`
/// (project `.npmrc` when `global` is false, `~/.npmrc` when true).
/// Returns the path written.
pub fn login(
    dir: &Path,
    registry: &str,
    token: &str,
    global: bool,
) -> Result<std::path::PathBuf, String> {
    let registry = registry.trim_end_matches('/');
    let host = registry
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(registry)
        .trim_end_matches('/');
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err(format!("bad registry URL {:?}", registry));
    }
    if token.trim().is_empty() {
        return Err("empty token".into());
    }
    let path = if global {
        let home = std::env::var("HOME").map_err(|_| "HOME not set")?;
        std::path::PathBuf::from(home).join(".npmrc")
    } else {
        dir.join(".npmrc")
    };
    let line = format!("//{}/:_authToken={}", host, token.trim());
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let prefix = format!("//{}:/", host);
    let mut replaced = false;
    for l in existing.lines() {
        if l.starts_with(&prefix) && l.contains("_authToken") {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !replaced {
        out.push(line.clone());
    }
    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}
