use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths::expand_tilde;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub dotfiles_dir: String,
    #[serde(default)]
    pub vars: Vec<String>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileEntry {
    pub src: String,
    pub target: Option<String>,
    #[serde(default = "default_true")]
    pub template: bool,
    #[serde(default)]
    pub vars: Vec<String>,
}

impl FileEntry {
    /// Return the target path string, defaulting to `~/.config/{src}` when unset.
    pub fn target(&self) -> String {
        self.target
            .clone()
            .unwrap_or_else(|| format!("~/.config/{}", self.src))
    }
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config =
            toml::from_str(&contents).with_context(|| "Failed to parse config file")?;
        Ok(config)
    }

    /// Return the default config file path.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| expand_tilde("~/.config"))
            .join("janus")
            .join("config.toml")
    }

    /// Return the expanded dotfiles directory path.
    pub fn dotfiles_dir(&self) -> PathBuf {
        expand_tilde(&self.dotfiles_dir)
    }

    /// Return the .generated directory path.
    pub fn generated_dir(&self) -> PathBuf {
        self.dotfiles_dir().join(".generated")
    }

    /// Return the .staged directory path.
    pub fn staged_dir(&self) -> PathBuf {
        self.dotfiles_dir().join(".staged")
    }

    /// Filter file entries by the given file/glob patterns.
    /// `None` means all entries; `Some` filters to matching entries.
    pub fn filter_files(&self, patterns: Option<&[String]>) -> Vec<&FileEntry> {
        let Some(patterns) = patterns else {
            return self.files.iter().collect();
        };
        self.files
            .iter()
            .filter(|entry| {
                patterns.iter().any(|pattern| {
                    if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                        glob_pattern.matches(&entry.src)
                    } else {
                        entry.src == *pattern
                    }
                })
            })
            .collect()
    }
}
