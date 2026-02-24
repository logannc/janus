use anyhow::{Result, bail};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

use crate::platform::Locker;

/// Attempt to acquire a process lock with retry and timeout.
///
/// Retries every 200ms until the lock is acquired or the timeout elapses.
/// A timeout of zero means fail immediately if the lock is held.
pub fn acquire_lock(locker: &mut impl Locker, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    let path_display = locker.lock_path().display().to_string();

    loop {
        if locker.try_lock()? {
            debug!("Acquired lock at {path_display}");
            return Ok(());
        }

        let elapsed = start.elapsed();
        let owner = locker.read_lock_owner()?;
        match owner {
            Some(pid) => debug!("Lock at {path_display} held by PID {pid}"),
            None => debug!("Lock at {path_display} held by unknown process"),
        }

        if elapsed >= timeout {
            let pid_msg = match owner {
                Some(pid) => format!("Another janus process (PID: {pid}) may be running."),
                None => "Another janus process may be running.".to_string(),
            };
            bail!(
                "Could not acquire lock at {path_display} within {}s.\n\
                 {pid_msg}\n\
                 If no other process is running, delete the lock file and retry.",
                timeout.as_secs(),
            );
        }

        trace!(
            "Lock busy, retrying ({:.1}s / {}s)",
            elapsed.as_secs_f64(),
            timeout.as_secs()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeLocker;
    use std::path::PathBuf;

    #[test]
    fn test_acquire_succeeds() {
        let mut locker = FakeLocker::new(PathBuf::from("/tmp/.janus.lock"));
        let result = acquire_lock(&mut locker, Duration::from_secs(5));
        assert!(result.is_ok());
    }

    #[test]
    fn test_acquire_timeout() {
        let mut locker = FakeLocker::new_contended(PathBuf::from("/tmp/.janus.lock"), 5678);
        let result = acquire_lock(&mut locker, Duration::from_secs(0));
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("/tmp/.janus.lock"), "got: {msg}");
    }

    #[test]
    fn test_timeout_error_includes_pid() {
        let mut locker = FakeLocker::new_contended(PathBuf::from("/tmp/.janus.lock"), 1234);
        let result = acquire_lock(&mut locker, Duration::from_secs(0));
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("1234"), "got: {msg}");
    }
}
