//! Environment forks — a full tree for the price of a directory entry.
//!
//! "Upgrade and pray" is the default in ML because a second environment means
//! downloading everything again. With a content-addressed store and hard links, the
//! second environment costs inodes, not bytes: fork a 6 GB tree in seconds, run both,
//! compare the numbers, keep the one that works.
//!
//! Deliberately conservative about what a fork is:
//! * regular files are **copy-on-write clones** (APFS `clonefile`, Linux `FICLONE`),
//!   or copied where those are unavailable — never re-downloaded. Hard links were the
//!   first implementation and were wrong: a fork exists to be *edited*, and a hard
//!   link means editing the fork rewrites the original (a patch or native build in the
//!   fork would silently change the environment you forked to protect);
//! * symlinks are recreated as symlinks, never followed (following one would copy a
//!   store tree into the project);
//! * the manifest and lockfiles are copied, because they are the fork's identity;
//! * nothing in the fork is written back to the source: it is a *separate* tree.
//!
//! This is the mechanism behind `jhol env fork`. It is not a git worktree and does not
//! pretend to be one: no history, no merge, just an environment you can switch to.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkStats {
    pub dirs: usize,
    pub files: usize,
    /// Files cloned copy-on-write: isolated for writes, shared for storage.
    pub cloned: usize,
    /// Files copied outright (no clone support for that filesystem).
    pub copied: usize,
    pub bytes: u64,
}

impl ForkStats {
    /// Bytes the fork actually costs: cloned files share blocks until written, copies
    /// cost their full size. Approximation, labelled as one.
    pub fn extra_bytes(&self) -> u64 {
        if self.files == 0 {
            return 0;
        }
        (self.bytes as f64 * self.copied as f64 / self.files as f64) as u64
    }

    pub fn describe(&self) -> String {
        format!(
            "{} dirs, {} files ({} cloned, {} copied), {} bytes referenced, ~{} bytes added",
            self.dirs,
            self.files,
            self.cloned,
            self.copied,
            self.bytes,
            self.extra_bytes()
        )
    }
}

/// Copy a tree, hard-linking regular files so the two trees share blocks.
pub fn fork_tree(src: &Path, dst: &Path) -> Result<ForkStats, String> {
    if !src.is_dir() {
        return Err(format!("{} is not a directory", src.display()));
    }
    if dst.exists() {
        return Err(format!("{} already exists", dst.display()));
    }
    let mut stats = ForkStats::default();
    fork_into(src, dst, &mut stats)?;
    Ok(stats)
}

fn fork_into(src: &Path, dst: &Path, stats: &mut ForkStats) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    stats.dirs += 1;
    let entries = std::fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to: PathBuf = dst.join(entry.file_name());
        let meta = match std::fs::symlink_metadata(&from) {
            Ok(m) => m,
            Err(e) => return Err(format!("{}: {e}", from.display())),
        };
        let ty = meta.file_type();
        if ty.is_symlink() {
            let target = std::fs::read_link(&from).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &to).map_err(|e| format!("{}: {e}", to.display()))?;
            #[cfg(not(unix))]
            {
                // No portable symlink; a copy is the honest fallback.
                std::fs::copy(std::fs::canonicalize(&from).unwrap_or(from.clone()), &to)
                    .map_err(|e| e.to_string())?;
                stats.copied += 1;
            }
            stats.files += 1;
        } else if ty.is_dir() {
            fork_into(&from, &to, stats)?;
        } else if ty.is_file() {
            stats.files += 1;
            stats.bytes += meta.len();
            if clone_file(&from, &to) {
                stats.cloned += 1;
            } else {
                std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
                stats.copied += 1;
            }
        }
        // Sockets/FIFOs are not part of an environment; skip silently rather than
        // fail a fork over a stray device node.
    }
    Ok(())
}

/// Copy-on-write clone of one file. Returns false when the platform or filesystem
/// cannot do it, in which case the caller copies (isolated either way; only the cost
/// differs).
#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dst: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let (Ok(s), Ok(d)) = (
        CString::new(src.as_os_str().as_bytes()),
        CString::new(dst.as_os_str().as_bytes()),
    ) else {
        return false;
    };
    // APFS clonefile(2): shares blocks until either side is written.
    unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) == 0 }
}

#[cfg(target_os = "linux")]
fn clone_file(src: &Path, dst: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    let (Ok(from), Ok(to)) = (std::fs::File::open(src), std::fs::File::create(dst)) else {
        return false;
    };
    // FICLONE = _IOW(0x94, 9, int)
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let ok = unsafe { libc::ioctl(to.as_raw_fd(), FICLONE, from.as_raw_fd()) } == 0;
    if !ok {
        let _ = std::fs::remove_file(dst);
    }
    ok
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_file(_src: &Path, _dst: &Path) -> bool {
    false
}

/// Files that make up a fork's *identity*: manifest plus any lockfiles.
pub fn identity_files(root: &Path) -> Vec<PathBuf> {
    ["package.json", "package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| root.join(n))
        .filter(|p| p.exists())
        .chain(
            [".jhol/models.lock.json"]
                .iter()
                .map(|n| root.join(n))
                .filter(|p| p.exists()),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &Path) {
        std::fs::create_dir_all(root.join("node_modules/a/lib")).unwrap();
        std::fs::write(root.join("node_modules/a/package.json"), r#"{"name":"a","version":"1.0.0"}"#).unwrap();
        std::fs::write(root.join("node_modules/a/lib/index.js"), "module.exports = 1;\n").unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"p"}"#).unwrap();
        std::fs::write(root.join("package-lock.json"), "{}").unwrap();
    }

    #[test]
    fn fork_is_isolated_from_its_source() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        let stats = fork_tree(&src.join("node_modules"), &tmp.path().join("fork/node_modules")).unwrap();
        assert_eq!(stats.dirs, 3, "node_modules, a, a/lib");
        assert_eq!(stats.files, 2);
        assert_eq!(stats.cloned + stats.copied, 2, "every file is isolated");
        if stats.cloned > 0 {
            assert_eq!(stats.copied, 0, "clone available: nothing should be copied");
        }

        // The point of a fork: editing it must not touch the source. This is where
        // hard links failed — the first implementation shared inodes and a write in
        // the fork changed the original environment.
        std::fs::write(tmp.path().join("fork/node_modules/a/lib/index.js"), "changed").unwrap();
        assert!(std::fs::read_to_string(src.join("node_modules/a/lib/index.js"))
            .unwrap()
            .contains("module.exports"));
    }

    #[test]
    fn refuses_to_overwrite_an_existing_fork() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        tree(&src);
        std::fs::create_dir_all(tmp.path().join("fork")).unwrap();
        let err = fork_tree(&src, &tmp.path().join("fork")).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn identity_files_are_the_manifest_and_locks() {
        let tmp = tempfile::tempdir().unwrap();
        tree(tmp.path());
        std::fs::create_dir_all(tmp.path().join(".jhol")).unwrap();
        std::fs::write(tmp.path().join(".jhol/models.lock.json"), "{}").unwrap();
        let names: Vec<String> = identity_files(tmp.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["package.json", "package-lock.json", "models.lock.json"]);
    }
}
