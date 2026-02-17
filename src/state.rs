//! Persistent state tracking for deployed symlinks and ignored import paths.
//!
//! State is stored in `.janus_state.toml` within the dotfiles directory.
//! Both `deployed` and `ignored` vectors have companion `HashSet` indexes
//! for O(1) lookups; add/remove methods keep both in sync.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::platform::Fs;

/// Structured recovery instructions emitted when a state save fails after a
/// mutation has already been applied to the filesystem.
///
/// Logged via `warn!` so the user can manually fix the desync between disk
/// state and the state file.
pub struct RecoveryInfo {
    /// What operation was performed (e.g. "hypr.conf has been deployed to ...").
    pub situation: Vec<String>,
    /// What will be broken if not fixed (e.g. "janus will not know ... is deployed").
    pub consequence: Vec<String>,
    /// Steps the user can take to fix it (e.g. "Add a [[deployed]] entry ...").
    pub instructions: Vec<String>,
}

/// Tracks deployed files and ignored import paths, persisted to `.janus_state.toml`.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    /// Import paths the user chose to skip (persisted so they aren't re-prompted).
    #[serde(default)]
    pub ignored: Vec<IgnoredEntry>,
    /// Files currently deployed as symlinks.
    #[serde(default)]
    pub deployed: Vec<DeployedEntry>,

    /// Filesystem path to the state file (set on load, not serialized).
    #[serde(skip)]
    path: PathBuf,
    /// O(1) lookup index for ignored paths.
    #[serde(skip)]
    ignored_index: HashSet<String>,
    /// O(1) lookup index for deployed src keys.
    #[serde(skip)]
    deployed_index: HashSet<String>,
}

/// An import path the user chose to ignore.
#[derive(Debug, Deserialize, Serialize)]
pub struct IgnoredEntry {
    /// The target path that was skipped (tilde-collapsed).
    pub path: String,
    /// Why it was ignored (e.g. "user_declined").
    pub reason: String,
}

/// A file currently deployed as a symlink.
#[derive(Debug, Deserialize, Serialize)]
pub struct DeployedEntry {
    /// Relative source path within the dotfiles directory.
    pub src: String,
    /// Target path where the symlink lives (may contain `~`).
    pub target: String,
}

impl State {
    /// Rebuild the `HashSet` indexes from the `Vec` data.
    /// Called after deserialization since the indexes are `#[serde(skip)]`.
    fn rebuild_indexes(&mut self) {
        self.ignored_index = self.ignored.iter().map(|e| e.path.clone()).collect();
        self.deployed_index = self.deployed.iter().map(|e| e.src.clone()).collect();
    }

    /// Load state from `.janus_state.toml` in the given dotfiles directory.
    /// Returns a default empty state if the file doesn't exist yet.
    pub fn load(dotfiles_dir: &Path, fs: &impl Fs) -> Result<Self> {
        let path = dotfiles_dir.join(".janus_state.toml");
        if !fs.exists(&path) {
            return Ok(State {
                path,
                ..Default::default()
            });
        }
        let contents = fs
            .read_to_string(&path)
            .with_context(|| format!("Failed to read state file: {}", path.display()))?;
        let mut state: State =
            toml::from_str(&contents).with_context(|| "Failed to parse state file")?;
        state.path = path;
        state.rebuild_indexes();
        Ok(state)
    }

    /// Serialize and write the state file to disk.
    pub fn save(&self, fs: &impl Fs) -> Result<()> {
        let contents = toml::to_string_pretty(self).with_context(|| "Failed to serialize state")?;
        fs.write(&self.path, contents.as_bytes())
            .with_context(|| format!("Failed to write state file: {}", self.path.display()))?;
        Ok(())
    }

    /// Save state, emitting structured recovery instructions on failure.
    ///
    /// Use this after a filesystem mutation (deploy, undeploy) so that if the
    /// save fails, the user gets actionable instructions to fix the desync.
    pub fn save_with_recovery(&self, recovery: RecoveryInfo, fs: &impl Fs) -> Result<()> {
        if let Err(e) = self.save(fs) {
            warn!("Situation:");
            for line in &recovery.situation {
                warn!("  - {line}");
            }
            warn!("  - Update to statefile failed: {e:#}");
            warn!("Result:");
            for line in &recovery.consequence {
                warn!("  - {line}");
            }
            warn!("Instructions to fix:");
            for line in &recovery.instructions {
                warn!("  - {line}");
            }
            return Err(e);
        }
        Ok(())
    }

    /// Check if a path has been marked as ignored (O(1) lookup).
    pub fn is_ignored(&self, path: &str) -> bool {
        self.ignored_index.contains(path)
    }

    /// Check if a source file is currently deployed (O(1) lookup).
    pub fn is_deployed(&self, src: &str) -> bool {
        self.deployed_index.contains(src)
    }

    /// Mark a path as ignored. No-op if already ignored.
    pub fn add_ignored(&mut self, path: String, reason: String) {
        if self.ignored_index.insert(path.clone()) {
            self.ignored.push(IgnoredEntry { path, reason });
        }
    }

    /// Record a file as deployed. Updates the target if already tracked.
    pub fn add_deployed(&mut self, src: String, target: String) {
        if self.deployed_index.insert(src.clone()) {
            self.deployed.push(DeployedEntry { src, target });
        } else if let Some(entry) = self.deployed.iter_mut().find(|e| e.src == src) {
            entry.target = target;
        }
    }

    /// Remove a deployed entry by source path. No-op if not tracked.
    pub fn remove_deployed(&mut self, src: &str) {
        if self.deployed_index.remove(src) {
            self.deployed.retain(|e| e.src != src);
        }
    }

    /// Remove an ignored entry by path. No-op if not tracked.
    #[allow(dead_code)]
    pub fn remove_ignored(&mut self, path: &str) {
        if self.ignored_index.remove(path) {
            self.ignored.retain(|e| e.path != path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeFs;
    use crate::test_helpers::*;

    fn load_state(fs: &FakeFs) -> State {
        State::load(Path::new(DOTFILES), fs).unwrap()
    }

    #[test]
    fn load_missing_returns_default() {
        let fs = FakeFs::new(HOME);
        fs.add_dir(DOTFILES);
        // No state file exists
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(state.deployed.is_empty());
        assert!(state.ignored.is_empty());
    }

    #[test]
    fn load_existing_state() {
        let fs = setup_fs();
        let toml = r#"
[[deployed]]
src = "hypr/hypr.conf"
target = "~/.config/hypr/hypr.conf"

[[ignored]]
path = "~/.bashrc"
reason = "user_declined"
"#;
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), toml);
        let state = load_state(&fs);
        assert_eq!(state.deployed.len(), 1);
        assert_eq!(state.deployed[0].src, "hypr/hypr.conf");
        assert_eq!(state.ignored.len(), 1);
        assert_eq!(state.ignored[0].path, "~/.bashrc");
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let fs = setup_fs();
        let mut state = load_state(&fs);
        state.add_deployed("a.conf".to_string(), "~/.config/a.conf".to_string());
        state.add_ignored("~/.zshrc".to_string(), "user_declined".to_string());
        state.save(&fs).unwrap();

        let reloaded = load_state(&fs);
        assert!(reloaded.is_deployed("a.conf"));
        assert!(reloaded.is_ignored("~/.zshrc"));
    }

    #[test]
    fn add_deployed_new() {
        let mut state = State::default();
        state.add_deployed("a.conf".to_string(), "~/.config/a.conf".to_string());
        assert!(state.is_deployed("a.conf"));
        assert_eq!(state.deployed.len(), 1);
    }

    #[test]
    fn add_deployed_updates_target() {
        let mut state = State::default();
        state.add_deployed("a.conf".to_string(), "~/.config/a.conf".to_string());
        state.add_deployed("a.conf".to_string(), "/new/target".to_string());
        assert_eq!(state.deployed.len(), 1);
        assert_eq!(state.deployed[0].target, "/new/target");
    }

    #[test]
    fn add_deployed_duplicate_noop() {
        let mut state = State::default();
        state.add_deployed("a.conf".to_string(), "~/.config/a.conf".to_string());
        state.add_deployed("a.conf".to_string(), "~/.config/a.conf".to_string());
        assert_eq!(state.deployed.len(), 1);
    }

    #[test]
    fn remove_deployed() {
        let mut state = State::default();
        state.add_deployed("a.conf".to_string(), "~/.config/a.conf".to_string());
        state.remove_deployed("a.conf");
        assert!(!state.is_deployed("a.conf"));
        assert!(state.deployed.is_empty());
    }

    #[test]
    fn remove_deployed_missing_noop() {
        let mut state = State::default();
        state.remove_deployed("nonexistent");
        assert!(state.deployed.is_empty());
    }

    #[test]
    fn is_deployed() {
        let mut state = State::default();
        assert!(!state.is_deployed("a.conf"));
        state.add_deployed("a.conf".to_string(), "target".to_string());
        assert!(state.is_deployed("a.conf"));
    }

    #[test]
    fn add_ignored_new() {
        let mut state = State::default();
        state.add_ignored("~/.bashrc".to_string(), "user_declined".to_string());
        assert!(state.is_ignored("~/.bashrc"));
        assert_eq!(state.ignored.len(), 1);
    }

    #[test]
    fn add_ignored_duplicate_noop() {
        let mut state = State::default();
        state.add_ignored("~/.bashrc".to_string(), "user_declined".to_string());
        state.add_ignored("~/.bashrc".to_string(), "user_declined".to_string());
        assert_eq!(state.ignored.len(), 1);
    }

    #[test]
    fn remove_ignored() {
        let mut state = State::default();
        state.add_ignored("~/.bashrc".to_string(), "user_declined".to_string());
        state.remove_ignored("~/.bashrc");
        assert!(!state.is_ignored("~/.bashrc"));
        assert!(state.ignored.is_empty());
    }

    #[test]
    fn is_ignored() {
        let mut state = State::default();
        assert!(!state.is_ignored("~/.bashrc"));
        state.add_ignored("~/.bashrc".to_string(), "user_declined".to_string());
        assert!(state.is_ignored("~/.bashrc"));
    }

    #[test]
    fn index_consistency() {
        let mut state = State::default();
        state.add_deployed("a".to_string(), "t1".to_string());
        state.add_deployed("b".to_string(), "t2".to_string());
        state.add_deployed("c".to_string(), "t3".to_string());
        state.remove_deployed("b");
        state.add_deployed("d".to_string(), "t4".to_string());
        state.remove_deployed("a");

        // Vec and index should agree
        assert_eq!(state.deployed.len(), 2);
        for entry in &state.deployed {
            assert!(state.deployed_index.contains(&entry.src));
        }
        assert!(!state.is_deployed("a"));
        assert!(!state.is_deployed("b"));
        assert!(state.is_deployed("c"));
        assert!(state.is_deployed("d"));
    }

    #[test]
    fn save_with_recovery_success() {
        let fs = setup_fs();
        let mut state = load_state(&fs);
        state.add_deployed("test.conf".to_string(), "~/.config/test.conf".to_string());
        let recovery = RecoveryInfo {
            situation: vec!["test".to_string()],
            consequence: vec!["test".to_string()],
            instructions: vec!["test".to_string()],
        };
        state.save_with_recovery(recovery, &fs).unwrap();
        // Verify state was actually written to disk
        let reloaded = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(reloaded.is_deployed("test.conf"));
    }

    #[test]
    fn save_with_recovery_failure() {
        let fs = setup_fs();
        let state = load_state(&fs);
        fs.set_fail_writes(true);
        let recovery = RecoveryInfo {
            situation: vec!["deployed file".to_string()],
            consequence: vec!["state desync".to_string()],
            instructions: vec!["fix manually".to_string()],
        };
        let result = state.save_with_recovery(recovery, &fs);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("write") || msg.contains("state"), "got: {msg}");
    }
}
