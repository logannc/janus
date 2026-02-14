use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use tracing::info;

use crate::config::Config;

pub fn run(config: &Config, files: &[String], dry_run: bool) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        info!("No files to stage");
        return Ok(());
    }

    let generated_dir = config.generated_dir();
    let staged_dir = config.staged_dir();

    for entry in &entries {
        let src_path = generated_dir.join(&entry.src);
        let dest_path = staged_dir.join(&entry.src);

        if !src_path.exists() {
            anyhow::bail!(
                "Generated file not found: {} (run `janus generate` first)",
                src_path.display()
            );
        }

        if dry_run {
            info!("[dry-run] Would stage: {}", entry.src);
            continue;
        }

        // Ensure parent directory exists
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        std::fs::copy(&src_path, &dest_path)
            .with_context(|| format!("Failed to stage file: {}", entry.src))?;

        // Preserve permissions
        let metadata = std::fs::metadata(&src_path)
            .with_context(|| format!("Failed to read metadata: {}", src_path.display()))?;
        std::fs::set_permissions(
            &dest_path,
            std::fs::Permissions::from_mode(metadata.permissions().mode()),
        )
        .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

        info!("Staged {}", entry.src);
    }

    info!("Staged {} file(s)", entries.len());
    Ok(())
}
