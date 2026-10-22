//! Safe file operations with error handling

pub fn log_warning(msg: &str) {
    eprintln!("WARNING: {}", msg);
}

pub fn log_error(msg: &str) {
    eprintln!("ERROR: {}", msg);
}

pub fn ensure_dir(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn atomic_write_json<T: serde::Serialize>(
    path: &std::path::Path,
    data: &T,
) -> Result<(), String> {
    let content = serde_json::to_string(data).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(())
}

use std::path::{Path, PathBuf};

/// RAII guard for temporary files. Automatically deletes the file on drop
/// unless explicitly disarmed via `.disarm()`.
#[derive(Debug)]
pub struct TmpFileGuard {
    path: PathBuf,
    active: bool,
}

impl TmpFileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for TmpFileGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// RAII guard for temporary directories. Automatically removes the directory
/// and all its contents on drop unless explicitly disarmed via `.disarm()`.
#[derive(Debug)]
pub struct TmpDirGuard {
    path: PathBuf,
    active: bool,
}

impl TmpDirGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for TmpDirGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tmp_file_guard_drops_when_active() {
        let tmp = std::env::temp_dir().join(format!("jhol-test-file-drop-{}", std::process::id()));
        std::fs::write(&tmp, b"test").unwrap();
        assert!(tmp.exists());
        {
            let _guard = TmpFileGuard::new(tmp.clone());
        }
        assert!(!tmp.exists());
    }

    #[test]
    fn test_tmp_file_guard_persists_when_disarmed() {
        let tmp =
            std::env::temp_dir().join(format!("jhol-test-file-disarm-{}", std::process::id()));
        std::fs::write(&tmp, b"test").unwrap();
        assert!(tmp.exists());
        {
            let mut guard = TmpFileGuard::new(tmp.clone());
            guard.disarm();
        }
        assert!(tmp.exists());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_tmp_dir_guard_drops_when_active() {
        let tmp = std::env::temp_dir().join(format!("jhol-test-dir-drop-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("file.txt"), b"test").unwrap();
        assert!(tmp.exists());
        {
            let _guard = TmpDirGuard::new(tmp.clone());
        }
        assert!(!tmp.exists());
    }

    #[test]
    fn test_tmp_dir_guard_persists_when_disarmed() {
        let tmp = std::env::temp_dir().join(format!("jhol-test-dir-disarm-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(tmp.exists());
        {
            let mut guard = TmpDirGuard::new(tmp.clone());
            guard.disarm();
        }
        assert!(tmp.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
