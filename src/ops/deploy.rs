//! Create symlinks from target paths to staged files.
//!
//! Each target path becomes a symlink pointing to the corresponding file in
//! `.staged/`. Existing files are backed up unless `--force` is set. Uses
//! fail-fast strategy with state saved after each file.
//!
//! The `atomic-deploy` feature (default) creates a temp symlink then atomically
//! renames it over the target, avoiding any window where the file doesn't exist.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::{RecoveryInfo, State};

/// Deploy staged files as symlinks to their target paths.
///
/// Bails on the first error. Saves state after each successful deployment
/// with recovery info in case the save itself fails.
pub fn run(config: &Config, files: Option<&[String]>, force: bool, dry_run: bool) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        info!("No files to deploy");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir();
    let staged_dir = config.staged_dir();
    let mut state = State::load(&dotfiles_dir)?;

    for entry in &entries {
        let staged_path = staged_dir.join(&entry.src);
        let target_path = expand_tilde(&entry.target());

        if !staged_path.exists() {
            anyhow::bail!(
                "Staged file not found: {} (run `janus stage` first)",
                staged_path.display()
            );
        }

        if dry_run {
            info!(
                "[dry-run] Would deploy: {} -> {}",
                entry.src,
                target_path.display()
            );
            continue;
        }

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        deploy_symlink(&staged_path, &target_path, force)?;

        state.add_deployed(entry.src.clone(), entry.target());
        state.save_with_recovery(RecoveryInfo {
            situation: vec![
                format!("{} has been deployed to {}", entry.src, target_path.display()),
            ],
            consequence: vec![
                format!("janus will not know {} is deployed to {}", entry.src, target_path.display()),
            ],
            instructions: vec![
                format!("Add a [[deployed]] entry to the statefile with src = \"{}\" and target = \"{}\"",
                    entry.src, entry.target()),
                format!("Or re-run: janus deploy {}", entry.src),
            ],
        })?;
        info!("Deployed {} -> {}", entry.src, target_path.display());
    }

    info!("Deployed {} file(s)", entries.len());
    Ok(())
}

/// Create a symlink from `target_path` → `staged_path` using atomic rename.
///
/// Creates a temporary symlink (`.janus.tmp`) then renames it over the target
/// so there's never a moment where the file is missing.
#[cfg(feature = "atomic-deploy")]
fn deploy_symlink(staged_path: &Path, target_path: &Path, force: bool) -> Result<()> {
    let exists = target_path.exists() || target_path.is_symlink();

    // Backup if needed (copy, so the original stays in place until the atomic swap)
    if exists && !force && !is_janus_symlink(target_path, staged_path) {
        let backup_path = backup_path_for(target_path);
        warn!(
            "Backing up existing file: {} -> {}",
            target_path.display(),
            backup_path.display()
        );
        std::fs::copy(target_path, &backup_path)
            .with_context(|| format!("Failed to backup file: {}", target_path.display()))?;
    } else if exists && force && !is_janus_symlink(target_path, staged_path) {
        warn!("Overwriting existing file: {}", target_path.display());
    }

    // Create a temp symlink in the same directory, then atomically rename over the target
    let temp_path = target_path.with_extension(".janus.tmp");
    // Clean up any stale temp symlink
    if temp_path.exists() || temp_path.is_symlink() {
        std::fs::remove_file(&temp_path)
            .with_context(|| format!("Failed to remove stale temp symlink: {}", temp_path.display()))?;
    }

    std::os::unix::fs::symlink(staged_path, &temp_path)
        .with_context(|| format!("Failed to create temp symlink: {}", temp_path.display()))?;

    std::fs::rename(&temp_path, target_path).with_context(|| {
        // Clean up temp symlink on failure
        let _ = std::fs::remove_file(&temp_path);
        format!(
            "Failed to atomically replace: {}",
            target_path.display()
        )
    })?;

    Ok(())
}

/// Create a symlink from `target_path` → `staged_path` using remove-then-create.
///
/// Non-atomic fallback: removes the existing file first, then creates the symlink.
#[cfg(not(feature = "atomic-deploy"))]
fn deploy_symlink(staged_path: &Path, target_path: &Path, force: bool) -> Result<()> {
    if target_path.exists() || target_path.is_symlink() {
        if is_janus_symlink(target_path, staged_path) {
            std::fs::remove_file(target_path)
                .with_context(|| format!("Failed to remove existing symlink: {}", target_path.display()))?;
        } else if force {
            warn!("Overwriting existing file: {}", target_path.display());
            std::fs::remove_file(target_path)
                .with_context(|| format!("Failed to remove existing file: {}", target_path.display()))?;
        } else {
            let backup_path = backup_path_for(target_path);
            warn!(
                "Backing up existing file: {} -> {}",
                target_path.display(),
                backup_path.display()
            );
            std::fs::rename(target_path, &backup_path)
                .with_context(|| format!("Failed to backup file: {}", target_path.display()))?;
        }
    }

    std::os::unix::fs::symlink(staged_path, target_path)
        .with_context(|| format!("Failed to create symlink: {} -> {}", target_path.display(), staged_path.display()))?;

    Ok(())
}

/// Compute the backup path for a file (e.g. `config.toml` → `config.toml.janus.bak`).
fn backup_path_for(target_path: &Path) -> std::path::PathBuf {
    target_path.with_extension(format!(
        "{}.janus.bak",
        target_path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    ))
}

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
