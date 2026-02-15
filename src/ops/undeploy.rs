//! Remove deployed symlinks, optionally leaving a regular file at the target.
//!
//! By default, copies the staged file to the target location before removing
//! the symlink, so the application keeps a working config. With `--remove-file`,
//! simply deletes the symlink.
//!
//! Uses fail-fast strategy with state saved after each file, consistent with
//! deploy behavior.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::{RecoveryInfo, State};

/// Check if `target` is a symlink pointing to `expected_staged`.
fn is_janus_symlink(target: &Path, expected_staged: &Path) -> bool {
    if !target.is_symlink() {
        return false;
    }
    match std::fs::read_link(target) {
        Ok(link_dest) => link_dest == expected_staged,
        Err(_) => false,
    }
}

/// Undeploy a single file's symlink. Verifies it's a janus symlink pointing to
/// the expected staged path, then either removes the symlink or replaces it with
/// a regular file copy.
///
/// Updates `state` to mark the file as no longer deployed. Does NOT save state.
///
/// Returns `Ok(true)` if undeployed, `Ok(false)` if skipped (not a janus symlink).
pub fn undeploy_single(
    src: &str,
    staged_dir: &Path,
    target_path: &Path,
    remove_file: bool,
    state: &mut State,
) -> Result<bool> {
    let staged_path = staged_dir.join(src);

    if !is_janus_symlink(target_path, &staged_path) {
        warn!(
            "Target is not a janus symlink, skipping: {}",
            target_path.display()
        );
        return Ok(false);
    }

    if remove_file {
        std::fs::remove_file(target_path).with_context(|| {
            format!("Failed to remove symlink: {}", target_path.display())
        })?;
    } else {
        undeploy_with_copy(&staged_path, target_path)?;
    }

    state.remove_deployed(src);
    Ok(true)
}

/// Undeploy files by removing their symlinks.
///
/// Default behavior copies the staged file to the target so the application
/// keeps a working config. `remove_file = true` just deletes the symlink.
/// Skips files that aren't deployed or whose target isn't a janus symlink.
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    remove_file: bool,
    dry_run: bool,
) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
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

        let target_path = expand_tilde(&entry.target());

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

        if !undeploy_single(&entry.src, &staged_dir, &target_path, remove_file, &mut state)? {
            continue;
        }

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

/// Replace a symlink with a regular file copy, atomically.
///
/// Copies the staged file to a temp path, then renames over the symlink
/// so there's never a moment where the target is missing.
#[cfg(feature = "atomic-deploy")]
fn undeploy_with_copy(staged_path: &Path, target_path: &Path) -> Result<()> {
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

/// Replace a symlink with a regular file copy (non-atomic fallback).
///
/// Removes the symlink first, then copies. Brief window where the file is missing.
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
