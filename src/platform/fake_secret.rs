//! Fake secret engine for testing.
//!
//! Pre-loaded with secret values via `add_secret()`. Calls to `resolve()`
//! return the matching value or bail if no secret was registered.

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::SecretEngine;

/// In-memory secret engine — returns pre-configured values without external calls.
pub struct FakeSecretEngine {
    /// Map of `(engine, reference)` -> resolved value.
    secrets: HashMap<(String, String), String>,
}

impl FakeSecretEngine {
    pub fn new() -> Self {
        Self {
            secrets: HashMap::new(),
        }
    }

    /// Register a secret that `resolve()` will return.
    /// Returns the previous value if one was already registered for this key.
    pub fn add_secret(&mut self, engine: &str, reference: &str, value: &str) -> Option<String> {
        self.secrets.insert(
            (engine.to_string(), reference.to_string()),
            value.to_string(),
        )
    }
}

impl SecretEngine for FakeSecretEngine {
    fn resolve(&self, engine: &str, reference: &str) -> Result<String> {
        let key = (engine.to_string(), reference.to_string());
        match self.secrets.get(&key) {
            Some(value) => Ok(value.clone()),
            None => bail!(
                "FakeSecretEngine: no secret for engine={}, reference={}",
                engine,
                reference
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_registered_secret() {
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://Vault/Item/Field", "s3cret");

        assert_eq!(
            engine
                .resolve("1password", "op://Vault/Item/Field")
                .unwrap(),
            "s3cret"
        );
    }

    #[test]
    fn test_resolve_missing_secret_fails() {
        let engine = FakeSecretEngine::new();
        assert!(engine.resolve("1password", "op://missing").is_err());
    }

    #[test]
    fn test_resolve_wrong_engine_fails() {
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://Vault/Item", "value");

        assert!(engine.resolve("bitwarden", "op://Vault/Item").is_err());
    }
}
