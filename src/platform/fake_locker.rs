use anyhow::Result;
use std::cell::Cell;
use std::path::{Path, PathBuf};

use super::Locker;

/// Test double for process locking.
pub struct FakeLocker {
    locked: Cell<bool>,
    contended: bool,
    owner_pid: Option<u32>,
    path: PathBuf,
}

impl FakeLocker {
    /// Create a locker that is not contended (lock will succeed).
    pub fn new(path: PathBuf) -> Self {
        Self {
            locked: Cell::new(false),
            contended: false,
            owner_pid: None,
            path,
        }
    }

    /// Create a locker that simulates another process holding the lock.
    pub fn new_contended(path: PathBuf, pid: u32) -> Self {
        Self {
            locked: Cell::new(false),
            contended: true,
            owner_pid: Some(pid),
            path,
        }
    }
}

impl Locker for FakeLocker {
    fn try_lock(&mut self) -> Result<bool> {
        if self.contended {
            return Ok(false);
        }
        self.locked.set(true);
        Ok(true)
    }

    fn unlock(&mut self) -> Result<()> {
        self.locked.set(false);
        Ok(())
    }

    fn read_lock_owner(&self) -> Result<Option<u32>> {
        Ok(self.owner_pid)
    }

    fn lock_path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_lock_and_unlock() {
        let mut locker = FakeLocker::new(PathBuf::from("/tmp/test.lock"));
        assert!(locker.try_lock().unwrap());
        assert!(locker.locked.get());
        locker.unlock().unwrap();
        assert!(!locker.locked.get());
    }

    #[test]
    fn test_contended_lock_fails() {
        let mut locker = FakeLocker::new_contended(PathBuf::from("/tmp/test.lock"), 9999);
        assert!(!locker.try_lock().unwrap());
    }
}
