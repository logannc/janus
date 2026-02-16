//! Copy generated files from `.generated/` into `.staged/`.
//!
//! Staged files are the final versions that will be symlinked to their target
//! paths by `deploy`. This separation allows inspecting diffs between generated
//! and staged content before deploying.
//!
//! Uses error-collection strategy: processes all files and reports failures
//! at the end rather than bailing on the first error.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::config::Config;
use crate::platform::Fs;

/// Stage generated files for the given file patterns (or all files).
///
/// Collects per-file errors and reports them at the end. Returns an error
/// if any file failed to stage.
pub fn run(config: &Config, files: Option<&[String]>, dry_run: bool, fs: &impl Fs) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to stage");
        return Ok(());
    }

    let generated_dir = config.generated_dir(fs);
    let staged_dir = config.staged_dir(fs);
    let mut errors: Vec<(String, anyhow::Error)> = Vec::new();
    let mut succeeded = 0usize;

    for entry in &entries {
        match stage_file(entry, &generated_dir, &staged_dir, dry_run, fs) {
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
        info!(
            "Staged {} file(s) with {} failure(s)",
            succeeded,
            errors.len()
        );
        let mut msg = format!("Failed to stage {} file(s):", errors.len());
        for (src, e) in &errors {
            msg.push_str(&format!("\n  {src}: {e:#}"));
        }
        anyhow::bail!(msg);
    }

    Ok(())
}

/// Copy a single file from `.generated/` to `.staged/`, preserving permissions.
fn stage_file(
    entry: &crate::config::FileEntry,
    generated_dir: &Path,
    staged_dir: &Path,
    dry_run: bool,
    fs: &impl Fs,
) -> Result<()> {
    let src_path = generated_dir.join(&entry.src);
    let dest_path = staged_dir.join(&entry.src);

    if !fs.exists(&src_path) {
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
        fs.create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    fs.copy(&src_path, &dest_path)
        .with_context(|| format!("Failed to stage file: {}", entry.src))?;

    // Preserve permissions
    let mode = fs
        .file_mode(&src_path)
        .with_context(|| format!("Failed to read metadata: {}", src_path.display()))?;
    fs.set_file_mode(&dest_path, mode)
        .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

    info!("Staged {}", entry.src);
    Ok(())
}
