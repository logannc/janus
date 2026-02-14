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
            info!("[dry-run] Would deploy: {} -> {}", entry.src, target_path.display());
            continue;
        }

        // Create parent directories
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        // Handle existing file at target
        if target_path.exists() || target_path.is_symlink() {
            if is_janus_symlink(&target_path, &staged_path) {
                // Already a janus symlink, just update
                std::fs::remove_file(&target_path)
                    .with_context(|| format!("Failed to remove existing symlink: {}", target_path.display()))?;
            } else if force {
                warn!("Overwriting existing file: {}", target_path.display());
                std::fs::remove_file(&target_path)
                    .with_context(|| format!("Failed to remove existing file: {}", target_path.display()))?;
            } else {
                // Backup existing file
                let backup_path = target_path.with_extension(
                    format!(
                        "{}.janus.bak",
                        target_path
                            .extension()
                            .map(|e| e.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ),
                );
                warn!(
                    "Backing up existing file: {} -> {}",
                    target_path.display(),
                    backup_path.display()
                );
                std::fs::rename(&target_path, &backup_path)
                    .with_context(|| format!("Failed to backup file: {}", target_path.display()))?;
            }
        }

        // Create symlink
        std::os::unix::fs::symlink(&staged_path, &target_path)
            .with_context(|| format!("Failed to create symlink: {} -> {}", target_path.display(), staged_path.display()))?;

        state.add_deployed(entry.src.clone(), entry.target());
        info!("Deployed {} -> {}", entry.src, target_path.display());
    }

    if !dry_run {
        state.save()?;
    }

    info!("Deployed {} file(s)", entries.len());
    Ok(())
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
