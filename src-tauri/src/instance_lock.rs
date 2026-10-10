//! Second line of defence against two app instances sharing the data dir
//! (the first is `tauri-plugin-single-instance`). An exclusive OS file lock
//! (`File::try_lock`: `LockFileEx` on Windows, `flock` on Unix) is held on
//! `<data dir>/app.lock` for the whole process lifetime.
//!
//! The OS drops the lock when the process dies — even on a crash or a kill —
//! so a leftover lock *file* never blocks startup: only a live holder does.
//! The PID written into the file is for diagnostics only.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// File name inside the app data dir.
pub const LOCK_FILE: &str = "app.lock";

/// Held for the process lifetime; dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    /// Another live process holds the lock.
    Held,
    /// The lock file couldn't be created or locked (e.g. read-only dir).
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held => write!(f, "another IDM Replica instance is running"),
            LockError::Io(e) => write!(f, "could not use the instance lock: {e}"),
        }
    }
}

impl InstanceLock {
    /// Take the lock at `path` (creating the file and its directory).
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(LockError::Io)?;
        }
        // Never truncate before we own the lock: the holder's PID stays readable.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(LockError::Held),
            Err(TryLockError::Error(e)) => return Err(LockError::Io(e)),
        }
        // Ours now (any previous content is a crashed holder's leftover).
        let pid = std::process::id().to_string();
        let _ = file
            .set_len(0)
            .and_then(|_| file.seek(SeekFrom::Start(0)))
            .and_then(|_| file.write_all(pid.as_bytes()))
            .and_then(|_| file.flush());
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn second_holder_is_refused_until_the_first_is_dropped() {
        let dir = temp_dir("lock");
        let path = dir.join(LOCK_FILE);
        let first = InstanceLock::acquire(&path).expect("first lock");
        // Windows forbids reading a locked file through another handle; Unix
        // flock is advisory, so there the holder's PID is readable.
        let content = std::fs::read_to_string(first.path()).ok();
        if cfg!(unix) {
            assert_eq!(content, Some(std::process::id().to_string()));
        }
        // A separate open file description conflicts even within one process.
        assert!(matches!(InstanceLock::acquire(&path), Err(LockError::Held)));
        drop(first);
        let again = InstanceLock::acquire(&path).expect("lock after release");
        drop(again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_lock_file_from_a_crash_does_not_block_startup() {
        let dir = temp_dir("lock_stale");
        let path = dir.join(LOCK_FILE);
        // A crashed instance leaves the file (with its PID) but no OS lock.
        std::fs::write(&path, "999999").unwrap();
        let lock = InstanceLock::acquire(&path).expect("stale file must not block");
        drop(lock);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn creates_missing_directories() {
        let dir = temp_dir("lock_mkdir");
        let path = dir.join("nested").join("deeper").join(LOCK_FILE);
        let lock = InstanceLock::acquire(&path).expect("lock in new dir");
        assert!(path.exists());
        drop(lock);
        let _ = std::fs::remove_dir_all(dir);
    }
}
