//! Compound command: run generate -> stage -> deploy in one shot.
//!
//! Bails between steps if any step fails — won't deploy if generation or
//! staging produced errors.

use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::platform::{Fs, SecretEngine};

/// Run the full forward pipeline: generate, stage, then deploy.
///
/// If any step fails, subsequent steps are skipped. The `force` and `dry_run`
/// flags are passed through to each step.
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    force: bool,
    dry_run: bool,
    fs: &impl Fs,
    engine: &impl SecretEngine,
) -> Result<()> {
    info!("Running generate...");
    crate::ops::generate::run(config, files, dry_run, fs, engine)?;

    info!("Running stage...");
    crate::ops::stage::run(config, files, dry_run, fs)?;

    info!("Running deploy...");
    crate::ops::deploy::run(config, files, force, dry_run, fs)?;

    Ok(())
}
