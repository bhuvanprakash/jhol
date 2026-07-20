//! File locking utilities for protecting concurrent cache/store accesses.
//! Uses simple lock file approach for cross-platform compatibility.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process;

pub struct FileLock {
    _temp_file: File,
    path: PathBuf,
}

impl FileLock {
    pub fn lock(path: &Path) -> io::Result<Self> {
        let lock_path = Self::lock_path(path);
        loop {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    let pid = process::id();
                    use std::io::Write;
                    writeln!(file, "{}", pid)?;
                    file.sync_all()?;
                    return Ok(Self {
                        _temp_file: file,
                        path: lock_path,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn try_lock(path: &Path) -> io::Result<Option<Self>> {
        let lock_path = Self::lock_path(path);
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let pid = process::id();
                use std::io::Write;
                writeln!(file, "{}", pid)?;
                file.sync_all()?;
                Ok(Some(Self {
                    _temp_file: file,
                    path: lock_path,
                }))
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn lock_path(path: &Path) -> PathBuf {
        let mut lock_path = path.to_path_buf();
        lock_path.set_extension("lock");
        lock_path
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct AtomicWrite {
    _temp_path: PathBuf,
}

impl AtomicWrite {
    pub fn write_to_file(path: &Path, data: &[u8]) -> io::Result<()> {
        let temp_path = path.with_extension("tmp");
        let mut file = File::create(&temp_path)?;
        use std::io::Write;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    }
}
