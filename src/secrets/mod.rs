//! Secret resolution for templates.
//!
//! Secrets behave like template variables but are resolved at generate-time
//! from external secret engines (e.g. 1Password CLI). Secret config files
//! are parsed eagerly, but actual secret resolution (`op read`, etc.) is
//! deferred until needed and cached so each unique reference is resolved
//! at most once.

mod onepassword;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

/// A single secret entry from a secrets config file.
#[derive(Debug, Clone, Deserialize)]
pub struct SecretEntry {
    /// Template variable name this secret will be available as.
    pub name: String,
    /// Secret engine to use (e.g. "1password").
    pub engine: String,
    /// Engine-specific reference (e.g. "op://Private/foobar/password").
    pub reference: String,
}

/// Top-level structure of a secrets TOML file.
#[derive(Debug, Deserialize)]
struct SecretsFile {
    #[serde(default)]
    secret: Vec<SecretEntry>,
}

/// Caching resolver that dispatches to secret engines.
///
/// Caches resolved values by `"engine:reference"` so each unique
/// secret is fetched at most once per generate run.
pub struct SecretResolver {
    cache: HashMap<String, String>,
}

impl SecretResolver {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Resolve a secret entry, returning the cached value or fetching it.
    pub fn resolve(&mut self, entry: &SecretEntry) -> Result<String> {
        let cache_key = format!("{}:{}", entry.engine, entry.reference);
        if let Some(cached) = self.cache.get(&cache_key) {
            debug!("Secret cache hit: {}", entry.name);
            return Ok(cached.clone());
        }

        let value = match entry.engine.as_str() {
            "1password" => onepassword::resolve(&entry.reference)
                .with_context(|| format!("Failed to resolve secret '{}'", entry.name))?,
            other => bail!("Unknown secret engine: {other}"),
        };

        self.cache.insert(cache_key, value.clone());
        Ok(value)
    }
}

/// Parse secret entries from one or more TOML files in the dotfiles directory.
///
/// Missing files are silently skipped (consistent with `load_vars` behavior).
pub fn parse_secret_files(
    dotfiles_dir: &Path,
    secret_files: &[String],
) -> Result<Vec<SecretEntry>> {
    let mut entries = Vec::new();
    for file in secret_files {
        let path = dotfiles_dir.join(file);
        if !path.exists() {
            debug!("Secrets file not found, skipping: {}", path.display());
            continue;
        }
        debug!("Loading secrets from {}", path.display());
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read secrets file: {}", path.display()))?;
        let secrets_file: SecretsFile = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse secrets file: {}", path.display()))?;
        entries.extend(secrets_file.secret);
    }
    Ok(entries)
}

/// Resolve all secret entries into a map of template variable name → value.
///
/// Uses the shared resolver for caching across calls.
pub fn resolve_secrets(
    entries: &[SecretEntry],
    resolver: &mut SecretResolver,
) -> Result<HashMap<String, toml::Value>> {
    let mut secrets = HashMap::new();
    for entry in entries {
        let value = resolver.resolve(entry)?;
        secrets.insert(entry.name.clone(), toml::Value::String(value));
    }
    Ok(secrets)
}

/// Check that no variable name collides with a secret name.
///
/// Bails with a descriptive error listing all conflicts.
pub fn check_conflicts(
    vars: &HashMap<String, toml::Value>,
    secrets: &HashMap<String, toml::Value>,
) -> Result<()> {
    let conflicts: Vec<&String> = vars.keys().filter(|k| secrets.contains_key(*k)).collect();
    if conflicts.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = conflicts.iter().map(|s| s.as_str()).collect();
    bail!(
        "Variable/secret name collision: {}. \
         Each name must be unique across vars and secrets.",
        names.join(", ")
    );
}
