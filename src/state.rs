use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Describes what happened and how to recover when a state save fails
/// after a mutation has already been applied to disk.
pub struct RecoveryInfo {
    pub situation: Vec<String>,
    pub consequence: Vec<String>,
    pub instructions: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    #[serde(default)]
    pub ignored: Vec<IgnoredEntry>,
    #[serde(default)]
    pub deployed: Vec<DeployedEntry>,

    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    ignored_index: HashSet<String>,
    #[serde(skip)]
    deployed_index: HashSet<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IgnoredEntry {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeployedEntry {
    pub src: String,
    pub target: String,
}

impl State {
    fn rebuild_indexes(&mut self) {
        self.ignored_index = self.ignored.iter().map(|e| e.path.clone()).collect();
        self.deployed_index = self.deployed.iter().map(|e| e.src.clone()).collect();
    }

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

    pub fn save(&self) -> Result<()> {
        let contents =
            toml::to_string_pretty(self).with_context(|| "Failed to serialize state")?;
        std::fs::write(&self.path, contents)
            .with_context(|| format!("Failed to write state file: {}", self.path.display()))?;
        Ok(())
    }

    /// Save state, emitting recovery instructions on failure.
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

    pub fn is_ignored(&self, path: &str) -> bool {
        self.ignored_index.contains(path)
    }

    #[allow(dead_code)]
    pub fn is_deployed(&self, src: &str) -> bool {
        self.deployed_index.contains(src)
    }

    pub fn add_ignored(&mut self, path: String, reason: String) {
        if self.ignored_index.insert(path.clone()) {
            self.ignored.push(IgnoredEntry { path, reason });
        }
    }

    pub fn add_deployed(&mut self, src: String, target: String) {
        if self.deployed_index.insert(src.clone()) {
            self.deployed.push(DeployedEntry { src, target });
        } else if let Some(entry) = self.deployed.iter_mut().find(|e| e.src == src) {
            entry.target = target;
        }
    }
}
