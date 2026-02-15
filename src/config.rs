//! Configuration types parsed from `~/.config/janus/config.toml`.
//!
//! The [`Config`] struct represents the top-level config, and [`FileEntry`]
//! represents a single managed file with its source path, target path,
//! template flag, and optional per-file variable overrides.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use strsim::jaro_winkler;

use crate::paths::expand_tilde;

/// Top-level janus configuration, loaded from a TOML file.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Path to the dotfiles directory (may contain `~`).
    pub dotfiles_dir: String,
    /// Global template variable files, relative to `dotfiles_dir`.
    #[serde(default)]
    pub vars: Vec<String>,
    /// Global secret config files, relative to `dotfiles_dir`.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Managed file entries.
    #[serde(default)]
    pub files: Vec<FileEntry>,
    /// Named groups of file patterns for batch operations.
    #[serde(default)]
    pub filesets: HashMap<String, FilesetEntry>,
}

/// A named fileset: file patterns with optional vars and secrets overrides.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilesetEntry {
    /// Glob patterns that select files in this set.
    pub patterns: Vec<String>,
    /// Variable files applied to files matching this fileset.
    #[serde(default)]
    pub vars: Vec<String>,
    /// Secret config files applied to files matching this fileset.
    #[serde(default)]
    pub secrets: Vec<String>,
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
    /// Per-file secret config files that override globals, relative to `dotfiles_dir`.
    #[serde(default)]
    pub secrets: Vec<String>,
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

    /// Resolve fileset names to their constituent file/glob patterns.
    ///
    /// Errors if any fileset name is not defined in config.
    pub fn resolve_filesets(&self, names: &[String]) -> Result<Vec<String>> {
        let mut patterns = Vec::new();
        for name in names {
            match self.filesets.get(name) {
                Some(entry) => patterns.extend(entry.patterns.iter().cloned()),
                None => {
                    if let Some(suggestion) = self.suggest_fileset(name) {
                        bail!("Unknown fileset: {name}. Did you mean: {suggestion}?");
                    }
                    bail!("Unknown fileset: {name}");
                }
            }
        }
        Ok(patterns)
    }

    /// Return all filesets whose patterns match the given source path.
    ///
    /// Used by generate to inherit fileset-level vars and secrets.
    pub fn matching_filesets(&self, src: &str) -> Vec<&FilesetEntry> {
        self.filesets
            .values()
            .filter(|entry| {
                entry.patterns.iter().any(|pattern| {
                    if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                        glob_pattern.matches(src)
                    } else {
                        src == pattern
                    }
                })
            })
            .collect()
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

    /// Find the closest matching `entry.src` values for each unmatched user pattern.
    ///
    /// Uses Jaro-Winkler similarity with a threshold of 0.8.
    pub fn suggest_files(&self, patterns: &[String]) -> Vec<String> {
        const THRESHOLD: f64 = 0.8;
        let mut suggestions = Vec::new();
        for pattern in patterns {
            let mut best: Option<(&str, f64)> = None;
            for entry in &self.files {
                let score = jaro_winkler(pattern, &entry.src);
                if score > THRESHOLD && (best.is_none() || score > best.unwrap().1) {
                    best = Some((&entry.src, score));
                }
            }
            if let Some((src, _)) = best
                && !suggestions.contains(&src.to_string())
            {
                suggestions.push(src.to_string());
            }
        }
        suggestions
    }

    /// Find the closest matching fileset name for a given input.
    ///
    /// Uses Jaro-Winkler similarity with a threshold of 0.8.
    pub fn suggest_fileset(&self, name: &str) -> Option<String> {
        const THRESHOLD: f64 = 0.8;
        let mut best: Option<(&str, f64)> = None;
        for key in self.filesets.keys() {
            let score = jaro_winkler(name, key);
            if score > THRESHOLD && (best.is_none() || score > best.unwrap().1) {
                best = Some((key, score));
            }
        }
        best.map(|(k, _)| k.to_string())
    }

    /// Bail with fuzzy-match suggestions when explicit patterns matched no files.
    ///
    /// When `patterns` is `None` (`--all`), returns `Ok(())` — the caller handles
    /// the info log for "no configured files". When `Some`, bails with suggestions
    /// if any are close enough, or a plain "no matching files" error otherwise.
    pub fn bail_unmatched(&self, patterns: Option<&[String]>) -> Result<()> {
        let Some(patterns) = patterns else {
            return Ok(());
        };
        let suggestions = self.suggest_files(patterns);
        if suggestions.is_empty() {
            bail!("No matching files found in config");
        } else {
            bail!(
                "No matching files found in config. Did you mean: {}?",
                suggestions.join(", ")
            );
        }
    }
}
