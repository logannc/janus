use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    #[serde(default)]
    pub ignored: Vec<IgnoredEntry>,
    #[serde(default)]
    pub deployed: Vec<DeployedEntry>,

    #[serde(skip)]
    path: PathBuf,
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
        Ok(state)
    }

    pub fn save(&self) -> Result<()> {
        let contents =
            toml::to_string_pretty(self).with_context(|| "Failed to serialize state")?;
        std::fs::write(&self.path, contents)
            .with_context(|| format!("Failed to write state file: {}", self.path.display()))?;
        Ok(())
    }

    pub fn is_ignored(&self, path: &str) -> bool {
        self.ignored.iter().any(|e| e.path == path)
    }

    #[allow(dead_code)]
    pub fn is_deployed(&self, src: &str) -> bool {
        self.deployed.iter().any(|e| e.src == src)
    }

    pub fn add_ignored(&mut self, path: String, reason: String) {
        if !self.is_ignored(&path) {
            self.ignored.push(IgnoredEntry { path, reason });
        }
    }

    pub fn add_deployed(&mut self, src: String, target: String) {
        // Update existing entry or add new one
        if let Some(entry) = self.deployed.iter_mut().find(|e| e.src == src) {
            entry.target = target;
        } else {
            self.deployed.push(DeployedEntry { src, target });
        }
    }
}
