use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::State;

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
        info!("Deployed {} -> {}", entry.src, target_path.display());
    }

    if !dry_run {
        state.save()?;
    }

    info!("Deployed {} file(s)", entries.len());
    Ok(())
}

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

fn backup_path_for(target_path: &Path) -> std::path::PathBuf {
    target_path.with_extension(format!(
        "{}.janus.bak",
        target_path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    ))
}

fn is_janus_symlink(target: &Path, expected_staged: &Path) -> bool {
    if !target.is_symlink() {
        return false;
    }
    match std::fs::read_link(target) {
        Ok(link_dest) => link_dest == expected_staged,
        Err(_) => false,
    }
}
