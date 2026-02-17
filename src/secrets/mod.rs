//! Secret resolution for templates.
//!
//! Secrets behave like template variables but are resolved at generate-time
//! from external secret engines (e.g. 1Password CLI). Secret config files
//! are parsed eagerly, but actual secret resolution is deferred until needed
//! and cached so each unique reference is resolved at most once.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

use crate::platform::{Fs, SecretEngine};

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

/// Caching resolver that dispatches to a [`SecretEngine`] implementation.
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

    /// Resolve a secret entry, returning the cached value or fetching via the engine.
    pub fn resolve(&mut self, entry: &SecretEntry, engine: &impl SecretEngine) -> Result<String> {
        let cache_key = format!("{}:{}", entry.engine, entry.reference);
        if let Some(cached) = self.cache.get(&cache_key) {
            debug!("Secret cache hit: {}", entry.name);
            return Ok(cached.clone());
        }

        let value = engine
            .resolve(&entry.engine, &entry.reference)
            .with_context(|| format!("Failed to resolve secret '{}'", entry.name))?;

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
    fs: &impl Fs,
) -> Result<Vec<SecretEntry>> {
    let mut entries = Vec::new();
    for file in secret_files {
        let path = dotfiles_dir.join(file);
        if !fs.exists(&path) {
            debug!("Secrets file not found, skipping: {}", path.display());
            continue;
        }
        debug!("Loading secrets from {}", path.display());
        let contents = fs
            .read_to_string(&path)
            .with_context(|| format!("Failed to read secrets file: {}", path.display()))?;
        let secrets_file: SecretsFile = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse secrets file: {}", path.display()))?;
        entries.extend(secrets_file.secret);
    }
    Ok(entries)
}

/// Resolve all secret entries into a map of template variable name -> value.
///
/// Uses the shared resolver for caching across calls.
pub fn resolve_secrets(
    entries: &[SecretEntry],
    resolver: &mut SecretResolver,
    engine: &impl SecretEngine,
) -> Result<HashMap<String, toml::Value>> {
    let mut secrets = HashMap::new();
    for entry in entries {
        let value = resolver.resolve(entry, engine)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeSecretEngine;
    use crate::test_helpers::*;

    fn make_secrets_toml(entries: &[(&str, &str, &str)]) -> String {
        let mut s = String::new();
        for (name, engine, reference) in entries {
            s.push_str(&format!(
                "[[secret]]\nname = \"{name}\"\nengine = \"{engine}\"\nreference = \"{reference}\"\n\n"
            ));
        }
        s
    }

    #[test]
    fn parse_single_file() {
        let fs = setup_fs();
        let toml = make_secrets_toml(&[("db_pass", "1password", "op://Vault/db/pass")]);
        fs.add_file(format!("{DOTFILES}/secrets.toml"), toml);
        let entries =
            parse_secret_files(Path::new(DOTFILES), &["secrets.toml".to_string()], &fs).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "db_pass");
        assert_eq!(entries[0].engine, "1password");
    }

    #[test]
    fn parse_multiple_files() {
        let fs = setup_fs();
        fs.add_file(
            format!("{DOTFILES}/s1.toml"),
            make_secrets_toml(&[("a", "1password", "op://a")]),
        );
        fs.add_file(
            format!("{DOTFILES}/s2.toml"),
            make_secrets_toml(&[("b", "1password", "op://b")]),
        );
        let entries = parse_secret_files(
            Path::new(DOTFILES),
            &["s1.toml".to_string(), "s2.toml".to_string()],
            &fs,
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parse_missing_file_skipped() {
        let fs = setup_fs();
        let entries = parse_secret_files(
            Path::new(DOTFILES),
            &["nonexistent.toml".to_string()],
            &fs,
        )
        .unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_invalid_toml_errors() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/bad.toml"), "not valid {{{");
        let result =
            parse_secret_files(Path::new(DOTFILES), &["bad.toml".to_string()], &fs);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("parse") || msg.contains("TOML") || msg.contains("secret"), "got: {msg}");
    }

    #[test]
    fn resolver_fresh_call() {
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://a", "secret_value");
        let mut resolver = SecretResolver::new();
        let entry = SecretEntry {
            name: "test".to_string(),
            engine: "1password".to_string(),
            reference: "op://a".to_string(),
        };
        let result = resolver.resolve(&entry, &engine).unwrap();
        assert_eq!(result, "secret_value");
    }

    #[test]
    fn resolver_cached() {
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://a", "value1");
        let mut resolver = SecretResolver::new();
        let entry = SecretEntry {
            name: "test".to_string(),
            engine: "1password".to_string(),
            reference: "op://a".to_string(),
        };
        let v1 = resolver.resolve(&entry, &engine).unwrap();
        // Remove from engine — should still get cached value
        let engine2 = FakeSecretEngine::new();
        let v2 = resolver.resolve(&entry, &engine2).unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn resolver_different_refs() {
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://a", "val_a");
        engine.add_secret("1password", "op://b", "val_b");
        let mut resolver = SecretResolver::new();
        let entry_a = SecretEntry {
            name: "a".to_string(),
            engine: "1password".to_string(),
            reference: "op://a".to_string(),
        };
        let entry_b = SecretEntry {
            name: "b".to_string(),
            engine: "1password".to_string(),
            reference: "op://b".to_string(),
        };
        assert_eq!(resolver.resolve(&entry_a, &engine).unwrap(), "val_a");
        assert_eq!(resolver.resolve(&entry_b, &engine).unwrap(), "val_b");
    }

    #[test]
    fn resolve_secrets_builds_map() {
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://a", "secret_a");
        let mut resolver = SecretResolver::new();
        let entries = vec![SecretEntry {
            name: "my_secret".to_string(),
            engine: "1password".to_string(),
            reference: "op://a".to_string(),
        }];
        let map = resolve_secrets(&entries, &mut resolver, &engine).unwrap();
        assert_eq!(
            map.get("my_secret"),
            Some(&toml::Value::String("secret_a".to_string()))
        );
    }

    #[test]
    fn check_conflicts_none() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), toml::Value::String("val".to_string()));
        let mut secrets = HashMap::new();
        secrets.insert("pass".to_string(), toml::Value::String("val".to_string()));
        assert!(check_conflicts(&vars, &secrets).is_ok());
    }

    #[test]
    fn check_conflicts_single() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), toml::Value::String("val".to_string()));
        let mut secrets = HashMap::new();
        secrets.insert("name".to_string(), toml::Value::String("val".to_string()));
        let result = check_conflicts(&vars, &secrets);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("name"), "got: {msg}");
    }

    #[test]
    fn check_conflicts_multiple() {
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), toml::Value::String("v".to_string()));
        vars.insert("b".to_string(), toml::Value::String("v".to_string()));
        let mut secrets = HashMap::new();
        secrets.insert("a".to_string(), toml::Value::String("v".to_string()));
        secrets.insert("b".to_string(), toml::Value::String("v".to_string()));
        let result = check_conflicts(&vars, &secrets);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("a"), "got: {msg}");
        assert!(msg.contains("b"), "got: {msg}");
    }
}
