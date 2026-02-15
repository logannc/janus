//! Configuration types parsed from `~/.config/janus/config.toml`.
//!
//! The [`Config`] struct represents the top-level config, and [`FileEntry`]
//! represents a single managed file with its source path, target path,
//! template flag, and optional per-file variable overrides.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths::expand_tilde;

/// Top-level janus configuration, loaded from a TOML file.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Path to the dotfiles directory (may contain `~`).
    pub dotfiles_dir: String,
    /// Global template variable files, relative to `dotfiles_dir`.
    #[serde(default)]
    pub vars: Vec<String>,
    /// Managed file entries.
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

/// A single managed file entry in the janus config.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileEntry {
    /// Relative path within the dotfiles directory (e.g. `hypr/hypr.conf`).
    pub src: String,
    /// Deployment target path (may contain `~`). Defaults to `~/.config/{src}`.
    pub target: Option<String>,
    /// Whether to render this file as a Tera template. Defaults to `true`.
    #[serde(default = "default_true")]
    pub template: bool,
    /// Per-file variable files that override globals, relative to `dotfiles_dir`.
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
    /// Load and parse a config file from the given path.
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
    ///
    /// `None` returns all entries. `Some` returns only entries whose `src`
    /// matches at least one pattern (glob syntax supported, falls back to
    /// exact match if the pattern is not a valid glob).
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
