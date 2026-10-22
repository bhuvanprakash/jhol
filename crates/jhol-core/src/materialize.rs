//! # jhol's materializer
//!
//! One projection: a resolved tree plus the content origin becomes
//! `node_modules`.
//!
//! Before this file the per-package materialization was scattered:
//! the placement decision (link / clone / copy) lived inside
//! `selective_extract`, the extract entry lived there too, and the
//! `.bin` shims lived in `bin_links`. Every caller had to know the
//! whole sequence and repeat it. `materialize` owns the sequence and
//! the placement policy in one place; the mechanical tree-walking
//! stays where it is — it is proven, and the store's symlink cycles
//! make it too fragile to move casually.
//!
//! The policy is jhol's own: a package is a content-addressed tree,
//! and the project is a projection of it. Placement mode is decided
//! once, here, from the same env knobs the rest of the installer reads.

use std::fs;
use std::path::Path;

/// How a package's files reach `node_modules`.
///
/// The order matters: `Copy` (`JHOL_LINK=0`) is the explicit opt-out
/// of linking, `Symlink` (`JHOL_LINK=1`) is the explicit one-syscall
/// mode, and `Clone` (the default) is a copy-on-write clone so Node
/// resolves self-imports from a real path inside the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Symlink,
    Clone,
    Copy,
}

impl Placement {
    /// Decide from the environment, exactly as the installer would.
    pub fn from_env() -> Self {
        if !crate::utils::use_link() {
            return Self::Copy;
        }
        if crate::utils::use_symlink() {
            return Self::Symlink;
        }
        Self::Clone
    }
}

pub fn place(src: &Path, dest: &Path) -> Result<usize, String> {
    let source_metadata = fs::symlink_metadata(src).map_err(|e| e.to_string())?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(format!("copy source is not a directory: {}", src.display()));
    }
    remove_existing_path(dest)?;
    // `JHOL_LINK=0` asks for copies, not links. This path used to symlink
    // unconditionally (only falling back to copying if the symlink call itself
    // failed), so the documented "copy from store instead of linking" mode was
    // silently ignored for every package that came through here.
    if !crate::utils::use_link() {
        let mut count = 0usize;
        let r = copy_tree_recursive(src, dest, &mut count);
        if std::env::var("JHOL_DEBUG_LAYOUT").is_ok() {
            crate::utils::log(&format!(
                "[extract]   COPY {} -> {} => {:?} ({} files), dest exists: {}",
                src.display(),
                dest.display(),
                r.as_ref().map(|_| ()).map_err(|e| e.as_str()),
                count,
                dest.exists()
            ));
        }
        r?;
        return Ok(count);
    }
    // Preserve opt-in symlinks, but default to a copy-on-write clone: Node
    // resolves package self-imports from a symlink target's real path, outside
    // this project's node_modules tree.
    if !crate::utils::use_symlink() {
        #[cfg(feature = "pipeline")]
        crate::jgrid::materialize::Cloner::clone_directory(src, dest)?;
        #[cfg(not(feature = "pipeline"))]
        copy_tree(src, dest)?;
        if std::env::var("JHOL_DEBUG_LAYOUT").is_ok() {
            crate::utils::log(&format!(
                "[extract]   CLONE {} -> {}",
                src.display(),
                dest.display()
            ));
        }
        return Ok(1);
    }
    // Explicit JHOL_LINK=1 preserves the one-syscall symlink mode.
    #[cfg(unix)]
    {
        let linked = std::os::unix::fs::symlink(src, dest).is_ok();
        if std::env::var("JHOL_DEBUG_LAYOUT").is_ok() {
            crate::utils::log(&format!(
                "[extract]   SYMLINK {} -> {} => {}",
                dest.display(),
                src.display(),
                linked
            ));
        }
        if linked {
            return Ok(1);
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(src, dest).is_ok() {
            return Ok(1);
        }
    }
    // Fall back to recursive hardlink/copy.
    let mut count = 0usize;
    copy_tree_recursive(src, dest, &mut count)?;
    Ok(count)
}

/// Copy a package tree, **never following symlinks**.
///
/// `Path::is_dir()` follows symlinks, and the store's relink scaffolding contains
/// cycles (`store/A/node_modules/B` -> `store/B`, and back). Walking those with
/// `is_dir()` recurses forever — observed as an unbounded traversal when a recursive
/// glob hit the same structure. A symlink is therefore reproduced as a symlink (which
/// preserves legitimately shipped links) instead of being descended into, and `depth`
/// bounds anything pathological.
pub(crate) fn copy_tree_recursive(
    src: &Path,
    dest: &Path,
    count: &mut usize,
) -> Result<(), String> {
    copy_tree_recursive_at(src, dest, count, 0, None)
}

pub fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    let mut count = 0;
    copy_tree_recursive(src, dest, &mut count)
}

pub fn copy_tree_excluding_root_file(
    src: &Path,
    dest: &Path,
    excluded_name: &std::ffi::OsStr,
) -> Result<(), String> {
    let mut count = 0;
    copy_tree_recursive_at(src, dest, &mut count, 0, Some(excluded_name))
}

fn copy_tree_recursive_at(
    src: &Path,
    dest: &Path,
    count: &mut usize,
    depth: usize,
    excluded_root_file: Option<&std::ffi::OsStr>,
) -> Result<(), String> {
    if depth > 64 {
        return Err(format!("copy tree too deep at {}", src.display()));
    }
    let source_metadata = fs::symlink_metadata(src).map_err(|e| e.to_string())?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(format!("copy source is not a directory: {}", src.display()));
    }
    if depth == 0 {
        remove_existing_path(dest)?;
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for ent in fs::read_dir(src).map_err(|e| e.to_string())? {
        let ent = ent.map_err(|e| e.to_string())?;
        if depth == 0 && excluded_root_file.is_some_and(|excluded| ent.file_name() == excluded) {
            continue;
        }
        let sp = ent.path();
        let dp = dest.join(ent.file_name());
        let meta = fs::symlink_metadata(&sp).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&sp).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dp).map_err(|e| e.to_string())?;
            #[cfg(windows)]
            {
                if fs::metadata(&sp).map(|m| m.is_dir()).unwrap_or(false) {
                    std::os::windows::fs::symlink_dir(&target, &dp)
                } else {
                    std::os::windows::fs::symlink_file(&target, &dp)
                }
                .map_err(|e| e.to_string())?;
            }
            #[cfg(not(any(unix, windows)))]
            return Err(format!(
                "cannot preserve symlink {} on this platform",
                sp.display()
            ));
            *count += 1;
        } else if meta.is_dir() {
            copy_tree_recursive_at(&sp, &dp, count, depth + 1, excluded_root_file)?;
        } else {
            // Reflink gives copy-on-write where supported; byte-copy otherwise. Never hard-link
            // installed package files to the store: package managers and lifecycle scripts may
            // edit node_modules, and must not mutate shared cache contents.
            if reflink_copy::reflink(&sp, &dp).is_err() {
                remove_existing_path(&dp)?;
                fs::copy(&sp, &dp).map_err(|e| e.to_string())?;
            }
            fs::set_permissions(&dp, meta.permissions()).map_err(|e| e.to_string())?;
            *count += 1;
        }
    }
    fs::set_permissions(dest, source_metadata.permissions()).map_err(|e| e.to_string())?;
    Ok(())
}

fn remove_existing_path(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        #[cfg(windows)]
        {
            fs::remove_dir(path)
                .or_else(|_| fs::remove_file(path))
                .map_err(|e| e.to_string())
        }
        #[cfg(not(windows))]
        {
            fs::remove_file(path).map_err(|e| e.to_string())
        }
    } else if !metadata.is_dir() {
        fs::remove_file(path).map_err(|e| e.to_string())
    } else {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    }
}

/// Extract a tarball into `dest` under the active feature sheaf.
pub fn extract(tarball: &Path, dest: &Path) -> Result<usize, String> {
    crate::selective_extract::extract_selective_to_path(tarball, dest)
}

/// Link a package's declared `bin` entries into `node_modules/.bin`.
///
/// Targets are made executable — a 644 `bin` script fails with
/// "Permission denied" when run through `.bin`.
pub fn link_bins(node_modules: &Path, package_name: &str) -> Result<(), String> {
    crate::bin_links::link_bins_for_package(node_modules, package_name)
}

/// The tail every materialization path shares: link the package's bins.
///
/// Callers that already placed the tree (from the store, from a
/// capsule, or by extraction) finish with this. It is the single place
/// `.bin` shims are created, so the four install paths cannot drift
/// apart.
pub fn finish(node_modules: &Path, package_name: &str) -> Result<(), String> {
    link_bins(node_modules, package_name)
}

/// Materialize one package: place its tree, then link its bins.
///
/// `src` is the package's directory in the content origin; `dest` is
/// its slot in `node_modules`; `node_modules` is the project root used
/// for `.bin`.
pub fn package(
    src: &Path,
    dest: &Path,
    node_modules: &Path,
    package_name: &str,
) -> Result<usize, String> {
    let placed = place(src, dest)?;
    finish(node_modules, package_name)?;
    Ok(placed)
}

/// Project a package from the content origin into a destination.
///
/// The sheaf selects which files are kept — legal + types + runtime +
/// bin by default, or the whole tree for `Full`. This is the
/// warm/capsule path's placement entry, so every projection goes
/// through the materializer instead of reaching into
/// `selective_extract` directly.
pub fn project(
    src: &Path,
    dest: &Path,
    sheaf: crate::selective_extract::FeatureSheaf,
) -> Result<usize, String> {
    crate::selective_extract::project_from_store(src, dest, sheaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("jhol-mat-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn placement_reads_the_env_knobs() {
        // Defaults: link on, symlink off -> Clone.
        std::env::remove_var("JHOL_LINK");
        assert_eq!(Placement::from_env(), Placement::Clone);
        std::env::set_var("JHOL_LINK", "0");
        assert_eq!(Placement::from_env(), Placement::Copy);
        std::env::set_var("JHOL_LINK", "1");
        assert_eq!(Placement::from_env(), Placement::Symlink);
        std::env::remove_var("JHOL_LINK");
    }

    #[test]
    fn copy_tree_reproduces_a_symlink_as_a_symlink() {
        let src = tmp("src");
        let dest = tmp("dest");
        fs::write(src.join("f.txt"), b"hi").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("f.txt", src.join("link")).unwrap();
        }
        copy_tree(&src, &dest).unwrap();
        assert!(dest.join("f.txt").exists());
        #[cfg(unix)]
        {
            assert!(dest
                .join("link")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink());
        }
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn place_links_or_copies_by_mode() {
        let src = tmp("psrc");
        let dest = tmp("pdest");
        fs::write(src.join("f.txt"), b"hi").unwrap();
        // Copy mode always produces a real directory, never a symlink.
        std::env::set_var("JHOL_LINK", "0");
        let n = place(&src, &dest).unwrap();
        assert!(n >= 1);
        assert!(dest.join("f.txt").exists());
        assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
        std::env::remove_var("JHOL_LINK");
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dest);
    }
}
