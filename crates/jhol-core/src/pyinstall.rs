//! Materialising a resolved Python tree — the install half of P0.
//!
//! `pysolve` decides *which* artifacts run here; this unpacks them and records what it
//! wrote, so the environment can be verified like every other part of jhol.
//!
//! Scope, stated plainly: wheels are verified (SHA-256 from the index), cached by
//! content, extracted into `site-packages`, and recorded per-file. Console scripts are
//! not rewritten into `bin/` and there is no RECORD-driven uninstall, so a pure-Python
//! package is importable on `PYTHONPATH=.jhol/py/site-packages` while an extension
//! module is not guaranteed to load. The value here is that the tree is deduplicated,
//! verified and reproducible — not that it reimplements pip.

use crate::pyresolve::{ArtifactFile, Resolved};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const PY_LOCK_PATH: &str = ".jhol/py/lock.json";
pub const SITE_PACKAGES: &str = ".jhol/py/site-packages";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct PyLock {
    #[serde(default = "one")]
    pub schema: u32,
    #[serde(default)]
    pub distributions: BTreeMap<String, Distribution>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Distribution {
    pub version: String,
    pub artifact: String,
    pub sha256: String,
    /// Every extracted file: relative path -> digest.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyState {
    Match,
    Missing { what: String },
    Wrong { what: String, expected: String, got: String },
    NoLock,
}

impl PyState {
    pub fn is_match(&self) -> bool {
        matches!(self, PyState::Match)
    }
    pub fn describe(&self) -> String {
        match self {
            PyState::Match => "python tree matches its lock".into(),
            PyState::Missing { what } => format!("{what} is missing"),
            PyState::Wrong { what, expected, got } => format!(
                "{what} is {} but the lock pins {}",
                &got[..12.min(got.len())],
                &expected[..12.min(expected.len())]
            ),
            PyState::NoLock => "no python lock".into(),
        }
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Normalise a distribution name the way wheel metadata does (`Foo.Bar` -> `foo_bar`).
pub fn normalise_dist(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

impl PyLock {
    pub fn path(root: &Path) -> PathBuf {
        root.join(PY_LOCK_PATH)
    }

    pub fn load(root: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(Self::path(root)).ok()?).ok()
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn verify(&self, root: &Path) -> PyState {
        let site = root.join(SITE_PACKAGES);
        for (name, dist) in &self.distributions {
            for (rel, want) in &dist.files {
                // site-packages is flat: a wheel already contains its package
                // directory, so files are recorded relative to the site root.
                let _ = name;
                let p = site.join(rel);
                let Ok(meta) = std::fs::metadata(&p) else {
                    return PyState::Missing {
                        what: format!("{} ({})", p.display(), dist.artifact),
                    };
                };
                // No zero-length heuristic here: an empty `__init__.py` or `py.typed`
                // is normal in a wheel, and the digest already covers it. (The store
                // guard rejects an empty *manifest*, which is a different question.)
                let _ = meta;
                match sha256_file(&p) {
                    Ok(got) if &got == want => {}
                    Ok(got) => {
                        return PyState::Wrong {
                            what: p.display().to_string(),
                            expected: want.clone(),
                            got,
                        }
                    }
                    Err(e) => {
                        return PyState::Missing {
                            what: format!("{} ({e})", p.display()),
                        }
                    }
                }
            }
        }
        PyState::Match
    }
}

/// Extract a wheel into `dest`, returning `relative path -> sha256`.
///
/// A wheel is downloaded content, so its archive is untrusted input: absolute paths,
/// `..` and symlinks are refused by `enclosed_name`, and `*.data/` (scripts, headers) is
/// skipped rather than placed where it would look installable.
pub fn extract_wheel(wheel: &Path, dest: &Path) -> Result<BTreeMap<String, String>, String> {
    let file = std::fs::File::open(wheel).map_err(|e| format!("{}: {e}", wheel.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", wheel.display()))?;
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!(
                "{}: unsafe path {:?} inside the wheel",
                wheel.display(),
                entry.name()
            ));
        };
        if entry.is_dir() {
            continue;
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.contains(".data/") {
            continue;
        }
        let target = dest.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::fs::write(&target, &buf).map_err(|e| format!("{}: {e}", target.display()))?;
        let digest = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(&buf);
            format!("{:x}", h.finalize())
        };
        out.insert(rel_str, digest);
    }
    Ok(out)
}

/// Install a resolved tree. `fetch` writes one artifact's bytes to a path; it is a
/// parameter so the whole install path is testable without a network.
pub fn install<F>(
    root: &Path,
    resolved: &Resolved,
    artifacts: &BTreeMap<String, ArtifactFile>,
    store: &Path,
    mut fetch: F,
) -> Result<PyLock, String>
where
    F: FnMut(&ArtifactFile, &Path) -> Result<(), String>,
{
    let site = root.join(SITE_PACKAGES);
    std::fs::create_dir_all(&site).map_err(|e| e.to_string())?;
    let mut lock = PyLock {
        schema: 1,
        distributions: BTreeMap::new(),
    };
    for (name, (version, chosen)) in &resolved.packages {
        let Some(artifact) = chosen.as_ref().and_then(|c| artifacts.get(c)) else {
            continue;
        };
        let cached = store.join(&artifact.sha256);
        if !cached.is_file() {
            std::fs::create_dir_all(store).map_err(|e| e.to_string())?;
            let tmp = store.join(format!("{}.part", artifact.sha256));
            fetch(artifact, &tmp)?;
            if !artifact.sha256.is_empty() {
                let got = sha256_file(&tmp)?;
                if got != artifact.sha256 {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(format!(
                        "{}: digest mismatch (index {}, downloaded {})",
                        artifact.filename, artifact.sha256, got
                    ));
                }
            }
            std::fs::rename(&tmp, &cached).map_err(|e| e.to_string())?;
        }
        let files = extract_wheel(&cached, &site)?;
        lock.distributions.insert(
            name.clone(),
            Distribution {
                version: version.clone(),
                artifact: artifact.filename.clone(),
                sha256: artifact.sha256.clone(),
                files,
            },
        );
    }
    lock.save(root).map_err(|e| e.to_string())?;
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyresolve::Resolved;
    use std::io::Write;

    fn make_wheel(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    fn artifact(name: &str, sha: &str) -> ArtifactFile {
        ArtifactFile {
            filename: name.into(),
            url: format!("https://example.invalid/{name}"),
            sha256: sha.into(),
            size: 0,
        }
    }

    #[test]
    fn installs_extracts_verifies_and_detects_tampering() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        let wheels = tmp.path().join("wheels");
        std::fs::create_dir_all(&wheels).unwrap();
        let wheel = wheels.join("demo-1.0.0-py3-none-any.whl");
        make_wheel(
            &wheel,
            &[
                ("demo/__init__.py", b"VERSION = '1.0.0'\n"),
                ("demo-1.0.0.dist-info/METADATA", b"Name: demo\nVersion: 1.0.0\n"),
            ],
        );
        let sha = sha256_file(&wheel).unwrap();
        let artifacts = BTreeMap::from([(
            "demo-1.0.0-py3-none-any.whl".to_string(),
            artifact("demo-1.0.0-py3-none-any.whl", &sha),
        )]);
        let resolved = Resolved {
            packages: BTreeMap::from([(
                "demo".to_string(),
                ("1.0.0".to_string(), Some("demo-1.0.0-py3-none-any.whl".to_string())),
            )]),
            skipped: Vec::new(),
        };
        let store = tmp.path().join("store");
        let lock = install(&root, &resolved, &artifacts, &store, |_a, dest| {
            std::fs::copy(&wheel, dest).map(|_| ()).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(lock.distributions["demo"].version, "1.0.0");
        assert!(root.join(SITE_PACKAGES).join("demo/__init__.py").is_file());
        assert_eq!(lock.verify(&root), PyState::Match);
        // The wheel is cached by content, so a second project costs no download.
        assert!(store.join(&sha).is_file());

        std::fs::write(root.join(SITE_PACKAGES).join("demo/__init__.py"), b"evil\n").unwrap();
        assert!(matches!(lock.verify(&root), PyState::Wrong { .. }));

        let bad = BTreeMap::from([(
            "demo-1.0.0-py3-none-any.whl".to_string(),
            artifact("demo-1.0.0-py3-none-any.whl", &"0".repeat(64)),
        )]);
        let err = install(&root, &resolved, &bad, &tmp.path().join("store2"), |_a, dest| {
            std::fs::copy(&wheel, dest).map(|_| ()).map_err(|e| e.to_string())
        })
        .unwrap_err();
        assert!(err.contains("digest mismatch"), "{err}");
    }

    #[test]
    fn refuses_unsafe_paths_inside_a_wheel() {
        let tmp = tempfile::tempdir().unwrap();
        let wheel = tmp.path().join("evil-1.0.0-py3-none-any.whl");
        make_wheel(&wheel, &[("../escape.py", b"nope")]);
        let err = extract_wheel(&wheel, &tmp.path().join("dest")).unwrap_err();
        assert!(err.contains("unsafe path"), "{err}");
    }

    #[test]
    fn skips_data_scripts_but_keeps_package_files() {
        let tmp = tempfile::tempdir().unwrap();
        let wheel = tmp.path().join("p-1.0.0-py3-none-any.whl");
        make_wheel(
            &wheel,
            &[("p/__init__.py", b"x=1"), ("p-1.0.0.data/scripts/tool", b"#!/bin/sh\n")],
        );
        let dest = tmp.path().join("dest");
        let files = extract_wheel(&wheel, &dest).unwrap();
        assert!(files.contains_key("p/__init__.py"));
        assert!(!files.keys().any(|k| k.contains(".data/")));
    }

    #[test]
    fn dist_names_are_normalised() {
        assert_eq!(normalise_dist("Foo.Bar-baz"), "foo_bar_baz");
        assert_eq!(normalise_dist("zope.interface"), "zope_interface");
    }
}
