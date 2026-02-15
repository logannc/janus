//! Delete generated files or remove orphaned files from `.generated/` and `.staged/`.
//!
//! Two modes:
//! - `--generated`: wipe everything in `.generated/` (files and empty dirs).
//! - `--orphans`: remove files in `.generated/` and `.staged/` that are no longer
//!   in the config. Staged orphans that are still actively deployed are preserved.
//!
//! Uses error-collection strategy: continues processing remaining files after
//! individual failures.

use anyhow::{bail, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::State;

/// Result of a clean operation: count of removed files and any errors encountered.
struct CleanResult {
    count: usize,
    errors: Vec<(PathBuf, anyhow::Error)>,
}

/// Clean generated files, orphans, or both. Requires at least one flag.
pub fn run(config: &Config, generated: bool, orphans: bool, dry_run: bool) -> Result<()> {
    if !generated && !orphans {
        bail!("Specify --generated, --orphans, or both");
    }

    let mut errors: Vec<(PathBuf, anyhow::Error)> = Vec::new();

    if generated {
        let result = clean_generated(config, dry_run);
        errors.extend(result.errors);
    }

    if orphans {
        let result = clean_orphans(config, dry_run)?;
        errors.extend(result.errors);
    }

    if !errors.is_empty() {
        let mut msg = format!("Failed to clean {} file(s):", errors.len());
        for (path, e) in &errors {
            msg.push_str(&format!("\n  {}: {e:#}", path.display()));
        }
        bail!(msg);
    }

    Ok(())
}

/// Delete everything in .generated/
fn clean_generated(config: &Config, dry_run: bool) -> CleanResult {
    let generated_dir = config.generated_dir();
    if !generated_dir.exists() {
        info!("No .generated/ directory to clean");
        return CleanResult { count: 0, errors: Vec::new() };
    }

    let mut count = 0usize;
    let mut errors = Vec::new();

    for entry in WalkDir::new(&generated_dir)
        .min_depth(1)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if dry_run {
            info!("[dry-run] Would remove: {}", path.display());
            count += entry.file_type().is_file() as usize;
            continue;
        }

        if entry.file_type().is_file() {
            match std::fs::remove_file(path) {
                Ok(()) => count += 1,
                Err(e) => {
                    warn!("Failed to remove: {}", path.display());
                    errors.push((path.to_path_buf(), e.into()));
                }
            }
        } else if entry.file_type().is_dir()
            && std::fs::remove_dir(path).is_err() {
                debug!("Keeping non-empty directory: {}", path.display());
            }
    }

    info!("Cleaned {} generated file(s)", count);
    CleanResult { count, errors }
}

/// Remove orphan files from `.generated/` and `.staged/`.
///
/// A file is an orphan if its relative path doesn't match any configured `src`.
/// Staged orphans that are still deployed as symlinks are preserved to avoid
/// breaking live config files.
fn clean_orphans(config: &Config, dry_run: bool) -> Result<CleanResult> {
    let configured_srcs: HashSet<&str> = config.files.iter().map(|f| f.src.as_str()).collect();

    let gen_result = clean_orphans_in_dir(
        &config.generated_dir(),
        "generated",
        &configured_srcs,
        |_| true,
        dry_run,
    );

    let dotfiles_dir = config.dotfiles_dir();
    let state = State::load(&dotfiles_dir)?;
    let staged_dir = config.staged_dir();

    let deployed_srcs: HashSet<&str> = state
        .deployed
        .iter()
        .filter(|d| {
            let target_path = expand_tilde(&d.target);
            is_symlink_to(&target_path, &staged_dir.join(&d.src))
        })
        .map(|d| d.src.as_str())
        .collect();

    let staged_result = clean_orphans_in_dir(
        &staged_dir,
        "staged",
        &configured_srcs,
        |relative| !deployed_srcs.contains(relative),
        dry_run,
    );

    let total = gen_result.count + staged_result.count;
    if total == 0 {
        info!("No orphans found");
    } else {
        info!(
            "Cleaned {} orphan(s) ({} generated, {} staged)",
            total, gen_result.count, staged_result.count,
        );
    }

    let mut errors = gen_result.errors;
    errors.extend(staged_result.errors);
    Ok(CleanResult { count: total, errors })
}

/// Walk a directory, remove files whose relative path isn't in `configured_srcs`
/// and for which `extra_check(relative_path)` returns true.
fn clean_orphans_in_dir(
    dir: &Path,
    label: &str,
    configured_srcs: &HashSet<&str>,
    extra_check: impl Fn(&str) -> bool,
    dry_run: bool,
) -> CleanResult {
    if !dir.exists() {
        return CleanResult { count: 0, errors: Vec::new() };
    }

    let mut count = 0usize;
    let mut errors = Vec::new();
    let mut dirs_to_check: Vec<PathBuf> = Vec::new();

    for entry in WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir() {
            dirs_to_check.push(entry.into_path());
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(dir)
            .expect("entry is under dir")
            .to_string_lossy();

        if configured_srcs.contains(relative.as_ref()) {
            continue;
        }

        if !extra_check(&relative) {
            debug!(
                "Keeping {} orphan (still deployed): {}",
                label, relative
            );
            continue;
        }

        if dry_run {
            info!("[dry-run] Would remove {} orphan: {}", label, relative);
        } else {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => {
                    info!("Removed {} orphan: {}", label, relative);
                }
                Err(e) => {
                    warn!("Failed to remove {} orphan: {}", label, relative);
                    errors.push((entry.path().to_path_buf(), e.into()));
                }
            }
        }
        count += 1;
    }

    if !dry_run {
        dirs_to_check.sort_by(|a, b| b.cmp(a));
        for dir_path in dirs_to_check {
            if std::fs::remove_dir(&dir_path).is_err() {
                debug!("Keeping non-empty directory: {}", dir_path.display());
            }
        }
    }

    CleanResult { count, errors }
}

/// Check if `path` is a symlink pointing to `expected_target`.
fn is_symlink_to(path: &Path, expected_target: &Path) -> bool {
    match std::fs::read_link(path) {
        Ok(dest) => dest == expected_target,
        Err(_) => false,
    }
}
