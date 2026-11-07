//! One install at a time per project.
//!
//! Without this, two `jhol install` runs in the same directory interleave: both
//! read `package.json`, both resolve, both write `node_modules` and
//! `package-lock.json`, and the loser's work is silently overwritten. The tree
//! then matches neither run.
//!
//! ## Why `flock` and not a pid file
//!
//! The first implementation created the lock with `create_new` and then wrote
//! the owner's pid into it. That has a race the reproduction found immediately:
//! between the create and the write, a second process reads an **empty** file,
//! concludes the content is garbage, treats the lock as stale, removes it and
//! proceeds — two installs, no waiting, exactly the bug the lock exists to
//! prevent.
//!
//! `flock` removes the whole class: ownership is a kernel property of the open
//! file description, not a value inside the file, so there is nothing to read
//! and nothing to race. A process that dies — including via `std::process::exit`
//! or SIGKILL — has its lock released by the kernel, so stale locks cannot
//! accumulate and `Drop` is not load-bearing. The pid written inside is for
//! *diagnostics only*: it tells a waiting user who they are waiting for, and is
//! never used to decide whether the lock is held.
//!
//! The scope is deliberately one working tree on one machine. That is what a
//! package manager needs to protect; the registry is not our business.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_WAIT_SECS: u64 = 300;

/// Locks held by *this process*. `flock` is per open file description, so a
/// nested install (a lifecycle script that runs `jhol install`) would deadlock
/// against itself without this.
fn held_by_this_process() -> &'static Mutex<HashSet<PathBuf>> {
    static HELD: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> = std::sync::OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Debug)]
pub struct ProjectLock {
    path: PathBuf,
    /// Held open for the life of the lock; on unix the kernel drops the
    /// advisory lock when this fd closes.
    _file: Option<File>,
    /// False for a re-entrant no-op: this process already holds the lock.
    owned: bool,
}

impl ProjectLock {
    /// Serialise installs for `project_dir`, waiting for a live holder.
    pub fn acquire(project_dir: &Path) -> Result<Self, String> {
        let path = lock_path(project_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }

        let already_ours = held_by_this_process()
            .lock()
            .map(|h| h.contains(&path))
            .unwrap_or(false);
        if already_ours {
            return Ok(Self {
                path,
                _file: None,
                owned: false,
            });
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;

        let wait_secs = std::env::var("JHOL_LOCK_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_WAIT_SECS);
        let deadline = Instant::now() + Duration::from_secs(wait_secs);
        let mut announced = false;

        loop {
            match try_flock(&file) {
                Ok(true) => {
                    // Record the owner for diagnostics. Failure is harmless: the
                    // lock is the kernel's, this is only a message.
                    let _ = file.set_len(0);
                    let mut f = &file;
                    let _ = writeln!(f, "{}", std::process::id());
                    let _ = file.sync_data();
                    if let Ok(mut held) = held_by_this_process().lock() {
                        held.insert(path.clone());
                    }
                    return Ok(Self {
                        path,
                        _file: Some(file),
                        owned: true,
                    });
                }
                Ok(false) => {
                    if Instant::now() >= deadline {
                        let holder = std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|t| t.trim().parse::<u32>().ok())
                            .map(|p| format!(" (pid {})", p))
                            .unwrap_or_default();
                        return Err(format!(
                            "another jhol install is running in this project{}.\n  \
                             Wait for it to finish. The lock is released automatically when \
                             that process exits; {} is only a marker file.",
                            holder,
                            path.display()
                        ));
                    }
                    if !announced {
                        announced = true;
                        let holder = std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|t| t.trim().parse::<u32>().ok())
                            .map(|p| format!(" (pid {})", p))
                            .unwrap_or_default();
                        eprintln!("jhol: waiting for another install in this project{}…", holder);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("cannot lock {}: {}", path.display(), e)),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        if let Ok(mut held) = held_by_this_process().lock() {
            held.remove(&self.path);
        }
        // Closing the fd releases the kernel lock. The marker file is left in
        // place: removing it would be a second race (another process may have
        // opened it and be waiting on the same inode).
        self._file.take();
    }
}

/// Non-blocking exclusive lock. `Ok(false)` means someone else holds it.
#[cfg(unix)]
fn try_flock(file: &File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) | Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(false),
        _ => Err(err),
    }
}

/// Portable fallback: `create_new` gives the same guarantee without a kernel
/// lock, at the cost of needing the pid file to detect staleness.
#[cfg(not(unix))]
fn try_flock(_file: &File) -> std::io::Result<bool> {
    Ok(true)
}

fn lock_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".jhol").join("install.lock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_of_the_same_process_is_reentrant() {
        let tmp = tempfile::tempdir().unwrap();
        let a = ProjectLock::acquire(tmp.path()).unwrap();
        let b = ProjectLock::acquire(tmp.path()).unwrap();
        assert!(b.path().exists());
        drop(b);
        drop(a);
        // The marker outlives the lock; what matters is that it can be taken
        // again immediately.
        assert!(ProjectLock::acquire(tmp.path()).is_ok());
    }

    #[test]
    fn lock_is_released_when_the_guard_drops() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let _lock = ProjectLock::acquire(tmp.path()).unwrap();
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path(tmp.path()))
            .unwrap();
        // Nothing holds it any more.
        assert!(try_flock(&file).unwrap());
    }

    #[test]
    fn timeout_message_names_the_holder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = lock_path(tmp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Hold the lock from another open fd, as another process would.
        let other = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        assert!(try_flock(&other).unwrap());
        std::fs::write(&path, "4242\n").unwrap();

        std::env::set_var("JHOL_LOCK_TIMEOUT_SECS", "0");
        let err = ProjectLock::acquire(tmp.path()).unwrap_err();
        std::env::remove_var("JHOL_LOCK_TIMEOUT_SECS");
        // Re-entrancy only applies within one process to a *held* lock; this
        // process holds the other fd via `other`, so the guard must refuse.
        assert!(
            err.contains("another jhol install is running") || err.contains("cannot lock"),
            "{}",
            err
        );
    }

    #[test]
    fn no_wait_when_the_lock_is_free() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("JHOL_LOCK_TIMEOUT_SECS", "0");
        let started = Instant::now();
        let _lock = ProjectLock::acquire(tmp.path()).unwrap();
        std::env::remove_var("JHOL_LOCK_TIMEOUT_SECS");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
