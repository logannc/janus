use anyhow::Result;
use tracing::info;

use crate::config::Config;

pub fn run(config: &Config, files: Option<&[String]>, force: bool, dry_run: bool) -> Result<()> {
    info!("Running generate...");
    crate::ops::generate::run(config, files, dry_run)?;

    info!("Running stage...");
    crate::ops::stage::run(config, files, dry_run)?;

    info!("Running deploy...");
    crate::ops::deploy::run(config, files, force, dry_run)?;

    Ok(())
}
