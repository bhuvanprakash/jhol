//! Registry read paths and content-addressed store maintenance.
//!
//! - `view`: packument inspection (versions, dist-tags, deps, integrity)
//! - `search`: registry search endpoint
//! - `cache gc`: SRI-reachability sweep over the content store — a tarball
//!   survives only if some lockfile references its integrity. Exact, not
//!   recency-based: an unreferenced tarball is dead no matter how recently
//!   it was touched; a referenced one survives no matter how old.

use std::collections::HashSet;
use std::path::Path;

/// `jhol view <pkg>[@version]` — render a packument (or one version slice).
pub fn view(package: &str) -> Result<String, String> {
    let (name, want_version) = match package.rsplit_once('@') {
        Some((n, v)) if !n.is_empty() && !package.starts_with('@') => (n, Some(v)),
        _ => (package, None),
    };
    let meta = crate::registry::fetch_metadata(name)?;
    let versions = meta
        .get("versions")
        .and_then(|v| v.as_object())
        .ok_or("packument has no versions object")?;

    let latest = meta
        .get("dist-tags")
        .and_then(|d| d.get("latest"))
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_string();

    let chosen = want_version.unwrap_or(&latest);
    let entry = versions.get(chosen);

    let mut out = String::new();
    out.push_str(&format!("{}@{}\n", name, chosen));
    if let Some(desc) = entry.and_then(|e| e.get("description")).and_then(|d| d.as_str()) {
        out.push_str(&format!("  {}\n", desc));
    }
    if let Some(home) = entry.and_then(|e| e.get("homepage")).and_then(|d| d.as_str()) {
        out.push_str(&format!("  {}\n", home));
    }
    if let Some(lic) = entry.and_then(|e| e.get("license")).and_then(|d| d.as_str()) {
        out.push_str(&format!("  license: {}\n", lic));
    }
    let mut all: Vec<&String> = versions.keys().collect();
    all.sort();
    out.push_str(&format!("  versions: {} total\n", all.len()));
    out.push_str(&format!("  dist-tags: latest={}\n", latest));
    if let Some(e) = entry {
        if let Some(deps) = e.get("dependencies").and_then(|d| d.as_object()) {
            if !deps.is_empty() {
                out.push_str("  dependencies:\n");
                let mut kv: Vec<(&String, &serde_json::Value)> = deps.iter().collect();
                kv.sort_by(|a, b| a.0.cmp(b.0));
                for (k, v) in kv.iter().take(12) {
                    out.push_str(&format!("    {}@{}\n", k, v.as_str().unwrap_or("?")));
                }
                if deps.len() > 12 {
                    out.push_str(&format!("    ... and {} more\n", deps.len() - 12));
                }
            }
        }
        if let Some(integ) = e
            .get("dist")
            .and_then(|d| d.get("integrity"))
            .and_then(|d| d.as_str())
        {
            out.push_str(&format!("  integrity: {}\n", integ));
        }
    }
    Ok(out)
}

/// `jhol search <term>` — the registry search endpoint, rendered.
pub fn search(term: &str, limit: usize) -> Result<String, String> {
    let client = crate::http_client::HttpClient::new(4);
    let url = format!(
        "https://registry.npmjs.org/-/v1/search?text={}&size={}",
        urlencoding_encode(term),
        limit.clamp(1, 50)
    );
    let body = client.get(&url)?;
    let v: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("search response: {}", e))?;
    let objects = v
        .get("objects")
        .and_then(|o| o.as_array())
        .ok_or("search response missing objects")?;
    let mut out = String::new();
    for obj in objects {
        let p = obj.get("package").cloned().unwrap_or(serde_json::Value::Null);
        let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let version = p.get("version").and_then(|n| n.as_str()).unwrap_or("?");
        let desc = p.get("description").and_then(|n| n.as_str()).unwrap_or("");
        let date = p.get("date").and_then(|n| n.as_str()).unwrap_or("");
        out.push_str(&format!("{}@{}  {}\n", name, version, date));
        if !desc.is_empty() {
            out.push_str(&format!("  {}\n", desc));
        }
    }
    if out.is_empty() {
        out.push_str("no results\n");
    }
    Ok(out)
}

/// Minimal percent-encoding for query terms (space + reserved chars).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `jhol cache gc` — SRI-reachability sweep over every content-store root the
/// installer writes to (the warm cache dir and the durable `~/.jhol-cache`).
///
/// Marks every store tarball whose sha512-SRI appears in any of the given
/// lockfiles, then deletes unreferenced ones. `dry_run` reports without
/// deleting. Returns (kept, freed_bytes, freed_files).
pub fn store_gc(lockfiles: &[std::path::PathBuf], dry_run: bool) -> Result<(usize, u64, usize), String> {
    // Reference set: every integrity string found in the lockfiles.
    let mut referenced: HashSet<String> = HashSet::new();
    for lf in lockfiles {
        let Ok(text) = std::fs::read_to_string(lf) else {
            continue;
        };
        collect_integries(&text, &mut referenced);
    }

    let mut kept = 0usize;
    let mut freed_bytes: u64 = 0;
    let mut freed_files = 0usize;
    for root in crate::content::roots() {
        let (k, b, f) = sweep_one(&crate::content::store_dir(&root), &referenced, dry_run)?;
        kept += k;
        freed_bytes += b;
        freed_files += f;

        // The unpacked store is what actually dominates the cache (a tarball
        // is deleted once its tree is materialised). Entries are keyed by the
        // tarball content hash; `store_index.json` maps name@version to that
        // hash, so reachability is: a lockfile references the name@version.
        let (k2, b2, f2) =
            sweep_unpacked(&root, &referenced_packages(lockfiles), dry_run)?;
        kept += k2;
        freed_bytes += b2;
        freed_files += f2;
    }
    Ok((kept, freed_bytes, freed_files))
}

/// name@version strings referenced by the given lockfiles (`packages` keys of
/// the form `node_modules/<name>` carrying a `version`).
fn referenced_packages(lockfiles: &[std::path::PathBuf]) -> HashSet<String> {
    let mut out = HashSet::new();
    for lf in lockfiles {
        let Ok(text) = std::fs::read_to_string(lf) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(packages) = v.get("packages").and_then(|p| p.as_object()) else {
            continue;
        };
        for (path, entry) in packages {
            let Some(name) = path.strip_prefix("node_modules/") else {
                continue;
            };
            if let Some(version) = entry.get("version").and_then(|x| x.as_str()) {
                out.insert(format!("{}@{}", name, version));
            }
        }
    }
    out
}

/// Sweep `<root>/store_unpacked/<xx>/<hash>` trees not reachable from any
/// lockfile. Fail-safe: a tree is deleted only when `store_index.json` maps a
/// *referenced* package to a different hash — an entry we cannot classify is
/// kept.
fn sweep_unpacked(
    root: &Path,
    referenced: &HashSet<String>,
    dry_run: bool,
) -> Result<(usize, u64, usize), String> {
    let unpacked = crate::content::unpacked_root(root);
    if !unpacked.exists() {
        return Ok((0, 0, 0));
    }
    let index = load_store_index(root);
    if index.is_empty() {
        // No index means we cannot prove anything is unreferenced: keep all.
        return Ok((0, 0, 0));
    }
    let live_hashes: HashSet<String> = referenced
        .iter()
        .filter_map(|pkg| index.get(pkg).cloned())
        .collect();

    let mut kept = 0usize;
    let mut freed_bytes: u64 = 0;
    let mut freed_files = 0usize;
    let Ok(shards) = std::fs::read_dir(&unpacked) else {
        return Ok((0, 0, 0));
    };
    for shard in shards.flatten() {
        let shard_path = shard.path();
        if !shard_path.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&shard_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let tree = entry.path();
            if !tree.is_dir() {
                continue;
            }
            let Some(hash) = tree.file_name().and_then(|n| n.to_str()).map(String::from) else {
                continue;
            };
            // Fail-safe: only remove when we can name a referenced package
            // whose hash differs, i.e. the tree is definitely not live.
            let definitely_dead = tree_looks_dead(&hash, &index, referenced);
            if !definitely_dead {
                kept += 1;
                continue;
            }
            let (bytes, files) = dir_size(&tree);
            freed_bytes += bytes;
            freed_files += files;
            if !dry_run {
                let _ = std::fs::remove_dir_all(&tree);
            }
        }
    }
    Ok((kept, freed_bytes, freed_files))
}

/// True only when `hash` is absent from the live set AND at least one
/// referenced package resolves through the index (so absence is meaningful).
fn tree_looks_dead(
    hash: &str,
    index: &std::collections::HashMap<String, String>,
    referenced: &HashSet<String>,
) -> bool {
    let live: HashSet<&String> = referenced
        .iter()
        .filter_map(|p| index.get(p))
        .collect();
    !live.is_empty() && !live.iter().any(|h| h.as_str() == hash)
}

fn load_store_index(root: &Path) -> std::collections::HashMap<String, String> {
    let path = root.join("store_index.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return std::collections::HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn dir_size(dir: &Path) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut files = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = e.metadata() {
                bytes += m.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

fn sweep_one(
    store_dir: &Path,
    referenced: &HashSet<String>,
    dry_run: bool,
) -> Result<(usize, u64, usize), String> {
    if !store_dir.exists() {
        return Ok((0, 0, 0));
    }

    let mut kept = 0usize;
    let mut freed_bytes: u64 = 0;
    let mut freed_files = 0usize;
    for entry in std::fs::read_dir(store_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tgz") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        // `sri_of_bytes` already returns the full `sha512-<base64>` string.
        // Prefixing it again produced `sha512-sha512-...`, which matched
        // nothing — so the first version of this function deleted every
        // cached tarball while reporting "0 reachable". The tests below pin
        // the single-prefix invariant.
        let sri = crate::registry::sri_of_bytes(&bytes);
        if referenced.contains(&sri) {
            kept += 1;
        } else {
            freed_bytes += bytes.len() as u64;
            freed_files += 1;
            if !dry_run {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                // sidecar .sri record goes with it
                let _ = std::fs::remove_file(path.with_extension("tgz.sri"));
            }
        }
    }
    Ok((kept, freed_bytes, freed_files))
}

/// Pull every `integrity` string out of a lockfile body (any JSON nesting).
fn collect_integries(text: &str, out: &mut HashSet<String>) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        fn walk(v: &serde_json::Value, out: &mut HashSet<String>) {
            match v {
                serde_json::Value::String(s) => {
                    if s.starts_with("sha512-") || s.starts_with("sha1-") {
                        out.insert(s.clone());
                    }
                }
                serde_json::Value::Object(m) => {
                    for val in m.values() {
                        walk(val, out);
                    }
                }
                serde_json::Value::Array(a) => {
                    for val in a {
                        walk(val, out);
                    }
                }
                _ => {}
            }
        }
        walk(&v, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("jhol-gc-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_store_tarball(store: &Path, body: &[u8]) -> String {
        std::fs::create_dir_all(store).unwrap();
        let sri = format!("sha512-{}", {
            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
            use sha2::Digest as _;
            B64.encode(sha2::Sha512::digest(body))
        });
        let name = format!("{}.tgz", crate::utils::content_hash_bytes(body));
        std::fs::write(store.join(&name), body).unwrap();
        sri
    }

    /// THE safety property: a tarball referenced by a lockfile must survive a
    /// sweep; an unreferenced one must go. The first version of this function
    /// reported "0 reachable" on a real project and deleted 133 cached
    /// tarballs — this test exists so that can never happen silently again.
    #[test]
    fn gc_keeps_referenced_and_removes_unreferenced() {
        let root = tmp("sweep");
        let store = crate::content::store_dir(&root);
        let referenced_sri = write_store_tarball(&store, b"referenced-tarball-bytes");
        let _dead_sri = write_store_tarball(&store, b"unreferenced-tarball-bytes");

        let lockfile = root.join("package-lock.json");
        let mut f = std::fs::File::create(&lockfile).unwrap();
        write!(
            f,
            r#"{{"packages":{{"node_modules/keepme":{{"integrity":"{}"}}}}}}"#,
            referenced_sri
        )
        .unwrap();

        let (kept, freed_bytes, freed_files) =
            sweep_one(&store, &{
                let mut s = std::collections::HashSet::new();
                s.insert(referenced_sri.clone());
                s
            }, false)
            .unwrap();
        assert_eq!(kept, 1, "the referenced tarball must be kept");
        assert_eq!(freed_files, 1, "only the unreferenced tarball is freed");
        assert!(freed_bytes > 0);

        let remaining: Vec<_> = std::fs::read_dir(&store)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tgz"))
            .collect();
        assert_eq!(remaining.len(), 1);
    }

    /// End-to-end: the lockfile on disk is what drives reachability.
    #[test]
    fn gc_reads_integrities_from_a_real_lockfile() {
        let root = tmp("lock");
        let store = crate::content::store_dir(&root);
        let keep_sri = write_store_tarball(&store, b"keep-me-tarball");
        let _drop_sri = write_store_tarball(&store, b"drop-me-tarball");
        let lockfile = root.join("package-lock.json");
        let body = format!(
            r#"{{"name":"demo","packages":{{"":{{"name":"demo"}},"node_modules/a":{{"integrity":"{}","resolved":"https://example.invalid/a.tgz"}}}}}}"#,
            keep_sri
        );
        std::fs::write(&lockfile, body).unwrap();

        // Reachability must be read from the file, not passed in.
        let mut referenced = std::collections::HashSet::new();
        collect_integries(&std::fs::read_to_string(&lockfile).unwrap(), &mut referenced);
        assert_eq!(referenced.len(), 1, "lockfile integrity must be collected");
        let (kept, _b, files) = sweep_one(&store, &referenced, false).unwrap();
        assert_eq!(kept, 1);
        assert_eq!(files, 1);
    }

    /// The unpacked store's safety property: a tree reachable through the
    /// store index survives; an unreachable one is reclaimed; and with no
    /// index nothing is touched (fail-safe — an unclassifiable entry is kept).
    #[test]
    fn unpacked_sweep_keeps_referenced_trees() {
        let root = tmp("unpacked");
        let unpacked = crate::content::unpacked_root(&root);
        let live_hash = "a".repeat(64);
        let dead_hash = "b".repeat(64);
        for h in [&live_hash, &dead_hash] {
            let d = unpacked.join(&h[..2]).join(h);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("index.js"), b"module.exports = 1;").unwrap();
            std::fs::write(d.join(".jhol-extract-complete"), b"").unwrap();
        }
        // Index: `demo@1.0.0` is the live tree.
        std::fs::write(
            root.join("store_index.json"),
            format!(r#"{{"demo@1.0.0":"{}"}}"#, live_hash),
        )
        .unwrap();

        let mut referenced = HashSet::new();
        referenced.insert("demo@1.0.0".to_string());
        let (kept, freed_bytes, _files) = sweep_unpacked(&root, &referenced, false).unwrap();
        assert_eq!(kept, 1, "the referenced tree must be kept");
        assert!(freed_bytes > 0, "the unreferenced tree must be reclaimed");
        assert!(unpacked.join(&live_hash[..2]).join(&live_hash).exists());
        assert!(!unpacked.join(&dead_hash[..2]).join(&dead_hash).exists());
    }

    /// No index → no deletions, whatever the reference set says.
    #[test]
    fn unpacked_sweep_without_index_is_a_noop() {
        let root = tmp("noidx");
        let unpacked = crate::content::unpacked_root(&root);
        let h = "c".repeat(64);
        let d = unpacked.join(&h[..2]).join(&h);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("f.js"), b"x").unwrap();
        let referenced = HashSet::new();
        let (kept, freed, _) = sweep_unpacked(&root, &referenced, false).unwrap();
        assert_eq!(kept, 0, "nothing is provably live without an index");
        assert_eq!(freed, 0, "nothing may be freed without an index");
        assert!(d.exists(), "the tree must survive");
    }

    /// dry-run reports without deleting.
    #[test]
    fn gc_dry_run_deletes_nothing() {
        let root = tmp("dry");
        let store = crate::content::store_dir(&root);
        let _ = write_store_tarball(&store, b"a");
        let _ = write_store_tarball(&store, b"b");
        let empty = std::collections::HashSet::new();
        let (_kept, _b, files) = sweep_one(&store, &empty, true).unwrap();
        assert_eq!(files, 2, "dry run counts what it would free");
        let on_disk = std::fs::read_dir(&store)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tgz"))
            .count();
        assert_eq!(on_disk, 2, "dry run must not delete");
    }
}
