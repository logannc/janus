//! Real secret engine implementation dispatching to external CLIs.

use anyhow::{bail, Context, Result};
use std::process::Command;

use super::SecretEngine;

/// Real secret engine — dispatches to external secret manager CLIs.
pub struct RealSecretEngine;

impl SecretEngine for RealSecretEngine {
    fn resolve(&self, engine: &str, reference: &str) -> Result<String> {
        match engine {
            "1password" => resolve_onepassword(reference),
            other => bail!("Unknown secret engine: {other}"),
        }
    }
}

/// Resolve a 1Password reference by calling `op read <reference>`.
fn resolve_onepassword(reference: &str) -> Result<String> {
    let output = Command::new("op")
        .arg("read")
        .arg(reference)
        .output()
        .context("Failed to run `op read`. Is 1Password CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`op read {reference}` failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        );
    }

    let value = String::from_utf8(output.stdout)
        .context("`op read` returned non-UTF-8 output")?
        .trim()
        .to_string();

    if value.is_empty() {
        bail!("`op read {reference}` returned empty output");
    }

    Ok(value)
}
