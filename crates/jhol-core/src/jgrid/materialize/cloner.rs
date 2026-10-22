use std::fs;
use std::path::Path;

/// Platform abstraction for directory cloning (copy-on-write)
///
/// Uses the fastest available mechanism:
/// - macOS APFS: clonefile(2)
/// - Linux Btrfs/XFS: FICLONE/FICLONERANGE  
/// - Fallback: recursive fs::copy (byte-for-byte)
pub struct Cloner;

impl Cloner {
    /// Clone a directory tree atomically (COW if supported).
    /// Source must be a directory, destination must not exist.
    pub fn clone_directory(src: &Path, dst: &Path) -> Result<(), String> {
        // Try platform-specific COW clone first
        #[cfg(target_os = "macos")]
        {
            match Self::clonefile_dir(src, dst) {
                Ok(()) => return Ok(()),
                Err(_) => {
                    // Retry after ensuring parent directory exists (e.g. @scope/ in node_modules)
                    if let Some(parent) = dst.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if dst.exists() {
                        let _ = fs::remove_dir_all(dst);
                        let _ = fs::remove_file(dst);
                    }
                    if Self::clonefile_dir(src, dst).is_ok() {
                        return Ok(());
                    }
                }
            }
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if Self::ficlonerange_dir(src, dst).is_ok() {
                return Ok(());
            }
        }

        if !src.is_dir() {
            return Err(format!("source not a directory: {src:?}"));
        }

        // Fallback: recursive copy
        Self::copy_dir_recursive(src, dst)
    }

    /// Clone a single file using COW if supported
    pub fn clone_file(src: &Path, dst: &Path) -> Result<(), String> {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create parent for {dst:?}: {e}"))?;
        }

        #[cfg(target_os = "macos")]
        {
            if Self::clonefile_file(src, dst).is_ok() {
                return Ok(());
            }
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if Self::ficlone_file(src, dst).is_ok() {
                return Ok(());
            }
        }

        fs::copy(src, dst).map_err(|e| format!("copy {src:?} → {dst:?}: {e}"))?;
        Ok(())
    }

    /// macOS: clonefile(2) for directories
    #[cfg(target_os = "macos")]
    fn clonefile_dir(src: &Path, dst: &Path) -> Result<(), String> {
        use std::os::unix::ffi::OsStrExt;
        let src_cstr = std::ffi::CString::new(src.as_os_str().as_bytes())
            .map_err(|e| format!("CString: {e}"))?;
        let dst_cstr = std::ffi::CString::new(dst.as_os_str().as_bytes())
            .map_err(|e| format!("CString: {e}"))?;

        let ret = unsafe { libc::clonefile(src_cstr.as_ptr(), dst_cstr.as_ptr(), 0) };
        if ret == 0 {
            Ok(())
        } else {
            Err(format!(
                "clonefile failed: {} (errno: {})",
                std::io::Error::last_os_error(),
                ret
            ))
        }
    }

    /// macOS: clonefile(2) for single files
    #[cfg(target_os = "macos")]
    fn clonefile_file(src: &Path, dst: &Path) -> Result<(), String> {
        use std::os::unix::ffi::OsStrExt;
        let src_cstr = std::ffi::CString::new(src.as_os_str().as_bytes())
            .map_err(|e| format!("CString: {e}"))?;
        let dst_cstr = std::ffi::CString::new(dst.as_os_str().as_bytes())
            .map_err(|e| format!("CString: {e}"))?;

        let ret = unsafe { libc::clonefile(src_cstr.as_ptr(), dst_cstr.as_ptr(), 0) };
        if ret == 0 {
            Ok(())
        } else {
            Err(format!(
                "clonefile failed: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    /// Linux: FICLONE ioctl for directories (recursive)
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn ficlonerange_dir(src: &Path, dst: &Path) -> Result<(), String> {
        // Walk the source directory and clone each file
        Self::copy_recursive_with(src, dst, &|s, d| Self::ficlone_file(s, d))
    }

    /// Linux: FICLONE ioctl for single file
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn ficlone_file(src: &Path, dst: &Path) -> Result<(), String> {
        use std::os::unix::io::AsRawFd; // trait provides as_raw_fd() on unix
        let src_file = std::fs::File::open(src).map_err(|e| format!("open {src:?}: {e}"))?;
        let dst_file = std::fs::File::create(dst).map_err(|e| format!("create {dst:?}: {e}"))?;

        let ret = unsafe { libc::ioctl(dst_file.as_raw_fd(), libc::FICLONE, src_file.as_raw_fd()) };
        if ret == 0 {
            Ok(())
        } else {
            Err(format!(
                "FICLONE failed: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    /// Recursive copy with per-file operation
    fn copy_recursive_with(
        src: &Path,
        dst: &Path,
        op: &dyn Fn(&Path, &Path) -> Result<(), String>,
    ) -> Result<(), String> {
        fs::create_dir_all(dst).map_err(|e| format!("create {dst:?}: {e}"))?;

        for entry in fs::read_dir(src).map_err(|e| format!("read dir {src:?}: {e}"))? {
            let entry = entry.map_err(|e| format!("entry: {e}"))?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                Self::copy_recursive_with(&src_path, &dst_path, op)?;
            } else if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                let target =
                    fs::read_link(&src_path).map_err(|e| format!("readlink {src_path:?}: {e}"))?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &dst_path)
                    .map_err(|e| format!("symlink {dst_path:?}: {e}"))?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_file(&target, &dst_path)
                    .map_err(|e| format!("symlink {dst_path:?}: {e}"))?;
            } else {
                op(&src_path, &dst_path)?;
            }
        }

        Ok(())
    }

    /// Fallback: byte-for-byte recursive copy
    fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
        Self::copy_recursive_with(src, dst, &|s, d| {
            fs::copy(s, d).map_err(|e| format!("copy {s:?} → {d:?}: {e}"))?;
            Ok(())
        })
    }

    /// Check if COW clone is supported on this system
    pub fn is_cow_supported() -> bool {
        #[cfg(target_os = "macos")]
        {
            true
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            return true;
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
        {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_clone_directory() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/b.txt"), b"world").unwrap();

        Cloner::clone_directory(&src, &dst).unwrap();
        assert!(dst.join("a.txt").exists());
        assert!(dst.join("sub/b.txt").exists());
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"hello");
    }

    #[test]
    fn test_clone_single_file() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("file.txt");
        let dst = dir.path().join("sub/file.txt");

        fs::write(&src, b"content").unwrap();
        Cloner::clone_file(&src, &dst).unwrap();
        assert!(dst.exists());
        assert_eq!(fs::read(dst).unwrap(), b"content");
    }

    #[test]
    fn test_clone_nonexistent_source() {
        let dir = tempdir().unwrap();
        let result =
            Cloner::clone_directory(&dir.path().join("nonexistent"), &dir.path().join("dst"));
        assert!(result.is_err());
    }
}
