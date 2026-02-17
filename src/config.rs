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
use crate::platform::Fs;

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
    pub fn load(path: &Path, fs: &impl Fs) -> Result<Self> {
        let contents = fs
            .read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config =
            toml::from_str(&contents).with_context(|| "Failed to parse config file")?;
        Ok(config)
    }

    /// Return the default config file path.
    pub fn default_path(fs: &impl Fs) -> PathBuf {
        fs.config_dir()
            .unwrap_or_else(|| expand_tilde("~/.config", fs))
            .join("janus")
            .join("config.toml")
    }

    /// Return the expanded dotfiles directory path.
    pub fn dotfiles_dir(&self, fs: &impl Fs) -> PathBuf {
        expand_tilde(&self.dotfiles_dir, fs)
    }

    /// Return the .generated directory path.
    pub fn generated_dir(&self, fs: &impl Fs) -> PathBuf {
        self.dotfiles_dir(fs).join(".generated")
    }

    /// Return the .staged directory path.
    pub fn staged_dir(&self, fs: &impl Fs) -> PathBuf {
        self.dotfiles_dir(fs).join(".staged")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeFs;
    use crate::test_helpers::*;

    #[test]
    fn load_minimal_config() {
        let fs = setup_fs();
        let toml = format!("dotfiles_dir = \"{DOTFILES}\"\n");
        let config = write_and_load_config(&fs, &toml);
        assert_eq!(config.dotfiles_dir, DOTFILES);
        assert!(config.files.is_empty());
        assert!(config.vars.is_empty());
        assert!(config.secrets.is_empty());
    }

    #[test]
    fn load_full_config() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"
vars = ["vars.toml"]
secrets = ["secrets.toml"]

[[files]]
src = "hypr/hypr.conf"
target = "~/.config/hypr/hypr.conf"
template = true
vars = ["hypr-vars.toml"]
secrets = ["hypr-secrets.toml"]

[filesets.desktop]
patterns = ["hypr/*", "waybar/*"]
vars = ["desktop-vars.toml"]
secrets = ["desktop-secrets.toml"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        assert_eq!(config.files.len(), 1);
        assert_eq!(config.vars, vec!["vars.toml"]);
        assert_eq!(config.secrets, vec!["secrets.toml"]);
        assert!(config.filesets.contains_key("desktop"));
        let desktop = &config.filesets["desktop"];
        assert_eq!(desktop.patterns, vec!["hypr/*", "waybar/*"]);
        assert_eq!(desktop.vars, vec!["desktop-vars.toml"]);
    }

    #[test]
    fn load_invalid_toml_errors() {
        let fs = setup_fs();
        fs.add_file(CONFIG_PATH, "not valid toml {{{}}}");
        let result = Config::load(Path::new(CONFIG_PATH), &fs);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("parse") || msg.contains("TOML") || msg.contains("deserialize"), "got: {msg}");
    }

    #[test]
    fn load_missing_file_errors() {
        let fs = setup_fs();
        let result = Config::load(Path::new("/nonexistent/config.toml"), &fs);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("config") || msg.contains("nonexistent"), "got: {msg}");
    }

    #[test]
    fn file_entry_target_default() {
        let entry = FileEntry {
            src: "hypr/hypr.conf".to_string(),
            target: None,
            template: true,
            vars: vec![],
            secrets: vec![],
        };
        assert_eq!(entry.target(), "~/.config/hypr/hypr.conf");
    }

    #[test]
    fn file_entry_target_explicit() {
        let entry = FileEntry {
            src: "bashrc".to_string(),
            target: Some("~/.bashrc".to_string()),
            template: true,
            vars: vec![],
            secrets: vec![],
        };
        assert_eq!(entry.target(), "~/.bashrc");
    }

    #[test]
    fn file_entry_template_defaults_true() {
        let fs = setup_fs();
        let toml = format!(
            "dotfiles_dir = \"{DOTFILES}\"\n\n[[files]]\nsrc = \"foo.conf\"\n"
        );
        let config = write_and_load_config(&fs, &toml);
        assert!(config.files[0].template);
    }

    #[test]
    fn default_path() {
        let fs = FakeFs::new("/home/test");
        let path = Config::default_path(&fs);
        assert_eq!(path, PathBuf::from("/home/test/.config/janus/config.toml"));
    }

    #[test]
    fn dotfiles_dir_expands_tilde() {
        let fs = setup_fs();
        let toml = "dotfiles_dir = \"~/dotfiles\"\n";
        let config = write_and_load_config(&fs, toml);
        assert_eq!(config.dotfiles_dir(&fs), PathBuf::from("/home/test/dotfiles"));
    }

    #[test]
    fn generated_dir() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        assert_eq!(
            config.generated_dir(&fs),
            PathBuf::from(format!("{DOTFILES}/.generated"))
        );
    }

    #[test]
    fn staged_dir() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        assert_eq!(
            config.staged_dir(&fs),
            PathBuf::from(format!("{DOTFILES}/.staged"))
        );
    }

    #[test]
    fn filter_files_none_returns_all() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None)]),
        );
        let filtered = config.filter_files(None);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_files_exact_match() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None)]),
        );
        let patterns = vec!["a.conf".to_string()];
        let filtered = config.filter_files(Some(&patterns));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].src, "a.conf");
    }

    #[test]
    fn filter_files_glob_match() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("hypr/hypr.conf", None), ("waybar/config", None)]),
        );
        let patterns = vec!["hypr/*".to_string()];
        let filtered = config.filter_files(Some(&patterns));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].src, "hypr/hypr.conf");
    }

    #[test]
    fn filter_files_no_match() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let patterns = vec!["nonexistent".to_string()];
        let filtered = config.filter_files(Some(&patterns));
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_files_multiple_patterns() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None), ("c.conf", None)]),
        );
        let patterns = vec!["a.conf".to_string(), "c.conf".to_string()];
        let filtered = config.filter_files(Some(&patterns));
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn matching_filesets_hit() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[[files]]
src = "hypr/hypr.conf"

[filesets.desktop]
patterns = ["hypr/*"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let matches = config.matching_filesets("hypr/hypr.conf");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn matching_filesets_multiple() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[[files]]
src = "hypr/hypr.conf"

[filesets.desktop]
patterns = ["hypr/*"]

[filesets.all_hypr]
patterns = ["hypr/*"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let matches = config.matching_filesets("hypr/hypr.conf");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn matching_filesets_miss() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[filesets.desktop]
patterns = ["hypr/*"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let matches = config.matching_filesets("waybar/config");
        assert!(matches.is_empty());
    }

    #[test]
    fn resolve_filesets_valid() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[filesets.desktop]
patterns = ["hypr/*", "waybar/*"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let patterns = config
            .resolve_filesets(&["desktop".to_string()])
            .unwrap();
        assert_eq!(patterns, vec!["hypr/*", "waybar/*"]);
    }

    #[test]
    fn resolve_filesets_unknown_errors() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let result = config.resolve_filesets(&["nonexistent".to_string()]);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("Unknown fileset"));
    }

    #[test]
    fn resolve_filesets_suggests_typo() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[filesets.desktop]
patterns = ["hypr/*"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let result = config.resolve_filesets(&["desktpp".to_string()]);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("Did you mean"), "got: {msg}");
    }

    #[test]
    fn suggest_files_close_match() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("hypr/hypr.conf", None)]),
        );
        let suggestions = config.suggest_files(&["hypr/hypr.conff".to_string()]);
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0], "hypr/hypr.conf");
    }

    #[test]
    fn suggest_files_no_match() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("hypr/hypr.conf", None)]),
        );
        let suggestions = config.suggest_files(&["zzzzzzz".to_string()]);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn bail_unmatched_none_ok() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        assert!(config.bail_unmatched(None).is_ok());
    }

    #[test]
    fn bail_unmatched_with_suggestion() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("hypr/hypr.conf", None)]),
        );
        let patterns = vec!["hypr/hypr.conff".to_string()];
        let result = config.bail_unmatched(Some(&patterns));
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("Did you mean"), "got: {msg}");
    }

    #[test]
    fn bail_unmatched_no_suggestion() {
        let fs = setup_fs();
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("hypr/hypr.conf", None)]),
        );
        let patterns = vec!["zzzzzzz".to_string()];
        let result = config.bail_unmatched(Some(&patterns));
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("No matching files"), "got: {msg}");
        assert!(!msg.contains("Did you mean"), "got: {msg}");
    }

    #[test]
    fn duplicate_src_both_returned() {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[[files]]
src = "a.conf"
target = "~/.config/a.conf"

[[files]]
src = "a.conf"
target = "~/.config/other/a.conf"
"#
        );
        let config = write_and_load_config(&fs, &toml);
        // filter_files(None) returns all entries, even duplicates
        let entries = config.filter_files(None);
        assert_eq!(entries.len(), 2);
        // Both should have the same src
        assert_eq!(entries[0].src, "a.conf");
        assert_eq!(entries[1].src, "a.conf");
        // But different targets
        assert_ne!(entries[0].target(), entries[1].target());
    }
}
