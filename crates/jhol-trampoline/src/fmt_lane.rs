//! `jhol fmt <files>` front-door lane: answer "already formatted" without loading the
//! 18 MB CLI image.
//!
//! Measured: `jhol fmt --check one.js` costs ~4.5 ms, of which 0.4 ms is formatting and
//! ~3.4 ms is process start (dyld, frameworks, ~800 page faults). This binary is tiny,
//! so a verified no-op costs only its own start.
//!
//! The lane is **fresh-only**: it answers only when it can prove the full CLI would
//! print `0 changed`. Every doubt falls through (returns `None`) and the real CLI runs,
//! so this can make a run faster but never different:
//!
//! * argv is exactly `fmt [--check] <file>...`; any other flag, a directory, or a
//!   missing path goes to the CLI (its errors, ignore rules and tools stay in one place);
//! * only in-process formatter extensions (external rustfmt/gofmt/ruff never cache);
//! * no `.prettierrc`, `.prettierrc.json`, `.jholrc` or `.prettierignore` in the cwd, so
//!   the config is the default one whose digest is computed here;
//! * `.jhol/fmt-cache.json` has this exact engine string and config digest, and holds
//!   the file under the same key, with the SHA-256 of the file's current bytes.
//!
//! Nothing is written: a fresh file means the CLI would not have written either.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Must equal `jhol_bundle::fmt::FMT_ENGINE`. Guarded by a test that reads that source.
pub const FMT_ENGINE: &str = concat!("jhol-fmt-", env!("CARGO_PKG_VERSION"), ".sem6");

const EXTS: [&str; 10] = [
    "js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts", "json", "css",
];

/// `FmtConfig::default().digest()` from jhol-bundle, field order and types preserved.
fn default_digest() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    80usize.hash(&mut h); // print_width
    2usize.hash(&mut h); // tab_width
    false.hash(&mut h); // use_tabs
    true.hash(&mut h); // semi
    false.hash(&mut h); // single_quote
    "all".to_string().hash(&mut h); // trailing_comma
    h.finish()
}

/// Number of files proven fresh, or `None` to fall through to the full CLI.
pub fn try_fresh(args: &[String], cwd: &Path) -> Option<usize> {
    if args.first().map(String::as_str) != Some("fmt") {
        return None;
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for a in &args[1..] {
        if a == "--check" {
            continue;
        }
        if a.starts_with('-') {
            return None;
        }
        paths.push(PathBuf::from(a));
    }
    // Directory (or whole-project) run: prove the recorded walk still holds.
    let dir_arg = match paths.len() {
        0 => Some(".".to_string()),
        1 if std::fs::metadata(&paths[0]).map_or(false, |m| m.is_dir()) => {
            let a = paths[0].to_string_lossy().to_string();
            Some(if a == "." { ".".to_string() } else { a })
        }
        _ => None,
    };
    for cfg in [".prettierrc", ".prettierrc.json", ".jholrc", ".prettierignore"] {
        if cwd.join(cfg).exists() {
            return None;
        }
    }
    // The CLI sorts and dedups before counting.
    paths.sort();
    paths.dedup();

    let raw = std::fs::read_to_string(cwd.join(".jhol").join("fmt-cache.json")).ok()?;
    let cache: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if cache.get("engine")?.as_str()? != FMT_ENGINE
        || cache.get("config")?.as_u64()? != default_digest()
    {
        return None;
    }
    let files = cache.get("files")?.as_object()?;

    if let Some(key) = dir_arg {
        return verify_walk(&cache, files, &key, cwd);
    }
    for p in &paths {
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        if !EXTS.contains(&ext.as_str()) || !std::fs::metadata(p).ok()?.is_file() {
            return None;
        }
        // Same key the CLI uses: path relative to cwd when it is under it, else as given.
        let rel = p.strip_prefix(cwd).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let want = files.get(&rel)?.as_str()?;
        let bytes = std::fs::read(p).ok()?;
        std::str::from_utf8(&bytes).ok()?; // the CLI reads a String; invalid UTF-8 is its error
        if format!("{:x}", Sha256::digest(&bytes)) != want {
            return None;
        }
    }
    Some(paths.len())
}

fn mtime_ns(md: &std::fs::Metadata) -> Option<u64> {
    let d = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_nanos() as u64)
}

/// The CLI recorded which directories its walk visited (mtime read before listing) and the
/// files it found. If no recorded directory changed, no entry was added, removed or renamed
/// there, so the walk returns the same list; if every listed file still hashes to its cache
/// entry, each one is fresh.
fn verify_walk(
    cache: &serde_json::Value,
    files: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    cwd: &Path,
) -> Option<usize> {
    let walk = cache.get("walks")?.get(key)?;
    for d in walk.get("dirs")?.as_array()? {
        let md = std::fs::metadata(cwd.join(d.get(0)?.as_str()?)).ok()?;
        if !md.is_dir() || mtime_ns(&md)? != d.get(1)?.as_u64()? {
            return None;
        }
    }
    let list = walk.get("files")?.as_array()?;
    for f in list {
        let rel = f.as_str()?;
        let want = files.get(rel)?.as_str()?;
        let bytes = std::fs::read(cwd.join(rel)).ok()?;
        std::str::from_utf8(&bytes).ok()?;
        if format!("{:x}", Sha256::digest(&bytes)) != want {
            return None;
        }
    }
    Some(list.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn project(src: &str, cached: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.js"), src).unwrap();
        std::fs::create_dir(d.path().join(".jhol")).unwrap();
        let h = format!("{:x}", Sha256::digest(cached.as_bytes()));
        let cache = serde_json::json!({"engine": FMT_ENGINE, "config": default_digest(),
                                       "files": {"a.js": h}});
        std::fs::write(d.path().join(".jhol/fmt-cache.json"), cache.to_string()).unwrap();
        d
    }

    /// The digest of the default config, as written by the real CLI into a real cache.
    #[test]
    fn default_digest_matches_the_one_the_cli_wrote() {
        assert_eq!(default_digest(), 2238493620898128545);
    }

    /// The engine string must track jhol-bundle, or the lane silently never hits (or,
    /// worse, hits against a cache an older engine wrote).
    #[test]
    fn engine_string_matches_jhol_bundle() {
        let here = env!("CARGO_MANIFEST_DIR");
        let src = std::fs::read_to_string(format!("{here}/../jhol-bundle/src/fmt.rs")).unwrap();
        assert!(src.contains(r#"concat!("jhol-fmt-", env!("CARGO_PKG_VERSION"), ".sem6")"#));
        let ver = |p: &str| {
            std::fs::read_to_string(format!("{here}/{p}"))
                .unwrap()
                .lines()
                .find(|l| l.starts_with("version"))
                .unwrap()
                .to_string()
        };
        assert_eq!(ver("Cargo.toml"), ver("../jhol-bundle/Cargo.toml"));
    }

    #[test]
    fn fresh_file_is_answered() {
        let d = project("a = 1;\n", "a = 1;\n");
        assert_eq!(try_fresh(&args(&["fmt", "--check", "a.js"]), d.path()), None, "relative to process cwd, not tempdir");
        let abs = d.path().join("a.js");
        let a = args(&["fmt", abs.to_str().unwrap()]);
        assert_eq!(try_fresh(&a, d.path()), Some(1));
    }

    #[test]
    fn every_doubt_falls_through() {
        let d = project("a = 1;\n", "a = 1;\n");
        let abs = d.path().join("a.js");
        let f = abs.to_str().unwrap();
        // edited file: hash differs
        std::fs::write(&abs, "a=1\n").unwrap();
        assert_eq!(try_fresh(&args(&["fmt", f]), d.path()), None);
        std::fs::write(&abs, "a = 1;\n").unwrap();
        // unknown flag, no files, directory, other subcommand
        assert_eq!(try_fresh(&args(&["fmt", "--no-cache", f]), d.path()), None);
        assert_eq!(try_fresh(&args(&["fmt", "--check"]), d.path()), None);
        assert_eq!(try_fresh(&args(&["fmt", d.path().to_str().unwrap()]), d.path()), None);
        assert_eq!(try_fresh(&args(&["install", f]), d.path()), None);
        // any config file changes what "formatted" means
        for cfg in [".prettierrc", ".prettierrc.json", ".jholrc", ".prettierignore"] {
            std::fs::write(d.path().join(cfg), "{}").unwrap();
            assert_eq!(try_fresh(&args(&["fmt", f]), d.path()), None, "{cfg}");
            std::fs::remove_file(d.path().join(cfg)).unwrap();
        }
        assert_eq!(try_fresh(&args(&["fmt", f]), d.path()), Some(1));
        // wrong engine in the cache
        let c = d.path().join(".jhol/fmt-cache.json");
        let t = std::fs::read_to_string(&c).unwrap().replace(FMT_ENGINE, "old-engine");
        std::fs::write(&c, t).unwrap();
        assert_eq!(try_fresh(&args(&["fmt", f]), d.path()), None);
    }

    #[test]
    fn extension_list_matches_the_cli() {
        let here = env!("CARGO_MANIFEST_DIR");
        let main = std::fs::read_to_string(format!("{here}/../../src/main.rs")).unwrap();
        let line = main.lines().skip_while(|l| !l.contains("const FMT_FRONT_EXTS")).nth(1).unwrap();
        let cli: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        assert_eq!(cli, EXTS.to_vec());
    }

    #[test]
    fn duplicates_count_once_like_the_cli() {
        let d = project("a = 1;\n", "a = 1;\n");
        let f = d.path().join("a.js");
        let f = f.to_str().unwrap();
        assert_eq!(try_fresh(&args(&["fmt", f, f]), d.path()), Some(1));
    }
}
