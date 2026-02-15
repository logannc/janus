//! 1Password CLI (`op read`) secret resolution.

use anyhow::{bail, Context, Result};
use std::process::Command;

/// Resolve a 1Password reference by calling `op read <reference>`.
///
/// Returns the trimmed secret value. Errors if `op` is not found,
/// exits non-zero, or returns empty/non-UTF-8 output.
pub fn resolve(reference: &str) -> Result<String> {
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
