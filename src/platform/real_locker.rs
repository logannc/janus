use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::Locker;

/// Production file-lock implementation backed by `fslock`.
pub struct RealLocker {
    inner: fslock::LockFile,
    path: PathBuf,
}

impl RealLocker {
    pub fn new(path: PathBuf) -> Result<Self> {
        let inner = fslock::LockFile::open(&path)
            .with_context(|| format!("Failed to open lock file at {}", path.display()))?;
        Ok(Self { inner, path })
    }
}

impl Locker for RealLocker {
    fn try_lock(&mut self) -> Result<bool> {
        self.inner
            .try_lock_with_pid()
            .with_context(|| format!("Failed to acquire lock at {}", self.path.display()))
    }

    fn unlock(&mut self) -> Result<()> {
        self.inner
            .unlock()
            .with_context(|| format!("Failed to release lock at {}", self.path.display()))
    }

    fn read_lock_owner(&self) -> Result<Option<u32>> {
        let contents = std::fs::read_to_string(&self.path).unwrap_or_default();
        let trimmed = contents.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        match trimmed.parse::<u32>() {
            Ok(pid) => Ok(Some(pid)),
            Err(_) => Ok(None),
        }
    }

    fn lock_path(&self) -> &Path {
        &self.path
    }
}
