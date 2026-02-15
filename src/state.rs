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
    pub fn load(dotfiles_dir: &Path) -> Result<Self> {
        let path = dotfiles_dir.join(".janus_state.toml");
        if !path.exists() {
            return Ok(State {
                path,
                ..Default::default()
            });
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read state file: {}", path.display()))?;
        let mut state: State =
            toml::from_str(&contents).with_context(|| "Failed to parse state file")?;
        state.path = path;
        state.rebuild_indexes();
        Ok(state)
    }

    /// Serialize and write the state file to disk.
    pub fn save(&self) -> Result<()> {
        let contents =
            toml::to_string_pretty(self).with_context(|| "Failed to serialize state")?;
        std::fs::write(&self.path, contents)
            .with_context(|| format!("Failed to write state file: {}", self.path.display()))?;
        Ok(())
    }

    /// Save state, emitting structured recovery instructions on failure.
    ///
    /// Use this after a filesystem mutation (deploy, undeploy) so that if the
    /// save fails, the user gets actionable instructions to fix the desync.
    pub fn save_with_recovery(&self, recovery: RecoveryInfo) -> Result<()> {
        if let Err(e) = self.save() {
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
    pub fn remove_ignored(&mut self, path: &str) {
        if self.ignored_index.remove(path) {
            self.ignored.retain(|e| e.path != path);
        }
    }
}
