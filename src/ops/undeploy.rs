use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::{RecoveryInfo, State};

fn is_janus_symlink(target: &Path, expected_staged: &Path) -> bool {
    if !target.is_symlink() {
        return false;
    }
    match std::fs::read_link(target) {
        Ok(link_dest) => link_dest == expected_staged,
        Err(_) => false,
    }
}

pub fn run(
    config: &Config,
    files: Option<&[String]>,
    remove_file: bool,
    dry_run: bool,
) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        info!("No files to undeploy");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir();
    let staged_dir = config.staged_dir();
    let mut state = State::load(&dotfiles_dir)?;
    let mut count = 0usize;

    for entry in &entries {
        if !state.is_deployed(&entry.src) {
            info!("Not deployed, skipping: {}", entry.src);
            continue;
        }

        let staged_path = staged_dir.join(&entry.src);
        let target_path = expand_tilde(&entry.target());

        if !is_janus_symlink(&target_path, &staged_path) {
            warn!(
                "Target is not a janus symlink, skipping: {}",
                target_path.display()
            );
            continue;
        }

        if dry_run {
            if remove_file {
                info!(
                    "[dry-run] Would undeploy (remove file): {} -> {}",
                    entry.src,
                    target_path.display()
                );
            } else {
                info!(
                    "[dry-run] Would undeploy (leave copy): {} -> {}",
                    entry.src,
                    target_path.display()
                );
            }
            count += 1;
            continue;
        }

        if remove_file {
            // Just remove the symlink
            std::fs::remove_file(&target_path).with_context(|| {
                format!("Failed to remove symlink: {}", target_path.display())
            })?;
        } else {
            // Copy staged file to target, replacing the symlink
            undeploy_with_copy(&staged_path, &target_path)?;
        }

        state.remove_deployed(&entry.src);
        state.save_with_recovery(RecoveryInfo {
            situation: vec![format!(
                "{} has been undeployed from {}",
                entry.src,
                target_path.display()
            )],
            consequence: vec![format!(
                "janus will still think {} is deployed to {}",
                entry.src,
                target_path.display()
            )],
            instructions: vec![
                format!(
                    "Remove the [[deployed]] entry from the statefile with src = \"{}\"",
                    entry.src
                ),
                format!("Or re-run: janus undeploy {}", entry.src),
            ],
        })?;

        if remove_file {
            info!("Undeployed {} (file removed)", entry.src);
        } else {
            info!("Undeployed {} (copy left at target)", entry.src);
        }
        count += 1;
    }

    info!("Undeployed {} file(s)", count);
    Ok(())
}

#[cfg(feature = "atomic-deploy")]
fn undeploy_with_copy(staged_path: &Path, target_path: &Path) -> Result<()> {
    // Atomic: copy staged to temp, rename over symlink
    let temp_path = target_path.with_extension(".janus.tmp");
    if temp_path.exists() || temp_path.is_symlink() {
        std::fs::remove_file(&temp_path)
            .with_context(|| format!("Failed to remove stale temp file: {}", temp_path.display()))?;
    }

    std::fs::copy(staged_path, &temp_path).with_context(|| {
        format!(
            "Failed to copy staged file to temp: {}",
            temp_path.display()
        )
    })?;

    std::fs::rename(&temp_path, target_path).with_context(|| {
        let _ = std::fs::remove_file(&temp_path);
        format!(
            "Failed to atomically replace symlink: {}",
            target_path.display()
        )
    })?;

    Ok(())
}

#[cfg(not(feature = "atomic-deploy"))]
fn undeploy_with_copy(staged_path: &Path, target_path: &Path) -> Result<()> {
    std::fs::remove_file(target_path).with_context(|| {
        format!("Failed to remove symlink: {}", target_path.display())
    })?;

    std::fs::copy(staged_path, target_path).with_context(|| {
        format!(
            "Failed to copy staged file to target: {}",
            target_path.display()
        )
    })?;

    Ok(())
}
