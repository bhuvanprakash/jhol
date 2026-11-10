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
        let started = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30);
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
                    // pid is a diagnostic; the lock is the create_new itself. No flush.
                    return Ok(Self {
                        _temp_file: file,
                        path: lock_path,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= timeout {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("timed out waiting for cache lock {}", lock_path.display()),
                        ));
                    }
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
                // pid is a diagnostic; the lock is the create_new itself. No flush.
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
        // fsync(2), not `sync_all`: on macOS Rust's `sync_all` is F_FULLFSYNC (a drive-cache
        // flush, ~3 ms each, measured) while fsync(2) is ~0.02 ms. fsync keeps the data
        // ordered before the rename against process and kernel failure; it does not promise
        // the drive's own cache survives a sudden power cut. Every caller writes a cache
        // index or a "complete" marker that can be rebuilt; lockfiles do not use this.
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(unix))]
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    }
}
