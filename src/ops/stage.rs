use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use tracing::{info, warn};

use crate::config::Config;

pub fn run(config: &Config, files: Option<&[String]>, dry_run: bool) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        info!("No files to stage");
        return Ok(());
    }

    let generated_dir = config.generated_dir();
    let staged_dir = config.staged_dir();
    let mut errors: Vec<(String, anyhow::Error)> = Vec::new();
    let mut succeeded = 0usize;

    for entry in &entries {
        match stage_file(entry, &generated_dir, &staged_dir, dry_run) {
            Ok(()) => succeeded += 1,
            Err(e) => {
                warn!("Failed to stage {}: {e:#}", entry.src);
                errors.push((entry.src.clone(), e));
            }
        }
    }

    if errors.is_empty() {
        info!("Staged {} file(s)", succeeded);
    } else {
        info!("Staged {} file(s) with {} failure(s)", succeeded, errors.len());
        let mut msg = format!("Failed to stage {} file(s):", errors.len());
        for (src, e) in &errors {
            msg.push_str(&format!("\n  {src}: {e:#}"));
        }
        anyhow::bail!(msg);
    }

    Ok(())
}

fn stage_file(
    entry: &crate::config::FileEntry,
    generated_dir: &std::path::Path,
    staged_dir: &std::path::Path,
    dry_run: bool,
) -> Result<()> {
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
        return Ok(());
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
    Ok(())
}
