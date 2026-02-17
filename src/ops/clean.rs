//! Delete generated files or remove orphaned files from `.generated/` and `.staged/`.
//!
//! Two modes:
//! - `--generated`: wipe everything in `.generated/` (files and empty dirs).
//! - `--orphans`: remove files in `.generated/` and `.staged/` that are no longer
//!   in the config. Staged orphans that are still actively deployed are preserved.
//!
//! Uses error-collection strategy: continues processing remaining files after
//! individual failures.

use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::platform::{Fs, WalkOptions};
use crate::state::State;

/// Result of a clean operation: count of removed files and any errors encountered.
struct CleanResult {
    count: usize,
    errors: Vec<(PathBuf, anyhow::Error)>,
}

/// Clean generated files, orphans, or both. Requires at least one flag.
pub fn run(
    config: &Config,
    generated: bool,
    orphans: bool,
    dry_run: bool,
    fs: &impl Fs,
) -> Result<()> {
    if !generated && !orphans {
        bail!("Specify --generated, --orphans, or both");
    }

    let mut errors: Vec<(PathBuf, anyhow::Error)> = Vec::new();

    if generated {
        let result = clean_generated(config, dry_run, fs)?;
        errors.extend(result.errors);
    }

    if orphans {
        let result = clean_orphans(config, dry_run, fs)?;
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
fn clean_generated(config: &Config, dry_run: bool, fs: &impl Fs) -> Result<CleanResult> {
    let generated_dir = config.generated_dir(fs);
    if !fs.exists(&generated_dir) {
        info!("No .generated/ directory to clean");
        return Ok(CleanResult {
            count: 0,
            errors: Vec::new(),
        });
    }

    let entries = fs.walk_dir(
        &generated_dir,
        &WalkOptions {
            min_depth: 1,
            contents_first: true,
            ..Default::default()
        },
    )?;

    let mut count = 0usize;
    let mut errors = Vec::new();

    for entry in &entries {
        if dry_run {
            info!("[dry-run] Would remove: {}", entry.path.display());
            count += entry.is_file as usize;
            continue;
        }

        if entry.is_file {
            match fs.remove_file(&entry.path) {
                Ok(()) => count += 1,
                Err(e) => {
                    warn!("Failed to remove: {}", entry.path.display());
                    errors.push((entry.path.clone(), e));
                }
            }
        } else if entry.is_dir && fs.remove_dir(&entry.path).is_err() {
            debug!("Keeping non-empty directory: {}", entry.path.display());
        }
    }

    info!("Cleaned {} generated file(s)", count);
    Ok(CleanResult { count, errors })
}

/// Remove orphan files from `.generated/` and `.staged/`.
///
/// A file is an orphan if its relative path doesn't match any configured `src`.
/// Staged orphans that are still deployed as symlinks are preserved to avoid
/// breaking live config files.
fn clean_orphans(config: &Config, dry_run: bool, fs: &impl Fs) -> Result<CleanResult> {
    let configured_srcs: HashSet<&str> = config.files.iter().map(|f| f.src.as_str()).collect();

    let gen_result = clean_orphans_in_dir(
        &config.generated_dir(fs),
        "generated",
        &configured_srcs,
        |_| true,
        dry_run,
        fs,
    )?;

    let dotfiles_dir = config.dotfiles_dir(fs);
    let state = State::load(&dotfiles_dir, fs)?;
    let staged_dir = config.staged_dir(fs);

    let deployed_srcs: HashSet<&str> = state
        .deployed
        .iter()
        .filter(|d| {
            let target_path = expand_tilde(&d.target, fs);
            is_symlink_to(&target_path, &staged_dir.join(&d.src), fs)
        })
        .map(|d| d.src.as_str())
        .collect();

    let staged_result = clean_orphans_in_dir(
        &staged_dir,
        "staged",
        &configured_srcs,
        |relative| !deployed_srcs.contains(relative),
        dry_run,
        fs,
    )?;

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
    Ok(CleanResult {
        count: total,
        errors,
    })
}

/// Walk a directory, remove files whose relative path isn't in `configured_srcs`
/// and for which `extra_check(relative_path)` returns true.
fn clean_orphans_in_dir(
    dir: &Path,
    label: &str,
    configured_srcs: &HashSet<&str>,
    extra_check: impl Fn(&str) -> bool,
    dry_run: bool,
    fs: &impl Fs,
) -> Result<CleanResult> {
    if !fs.exists(dir) {
        return Ok(CleanResult {
            count: 0,
            errors: Vec::new(),
        });
    }

    let entries = fs.walk_dir(
        dir,
        &WalkOptions {
            min_depth: 1,
            ..Default::default()
        },
    )?;

    let mut count = 0usize;
    let mut errors = Vec::new();
    let mut dirs_to_check: Vec<PathBuf> = Vec::new();

    for entry in &entries {
        if entry.is_dir {
            dirs_to_check.push(entry.path.clone());
            continue;
        }

        let relative = entry
            .path
            .strip_prefix(dir)
            .expect("entry is under dir")
            .to_string_lossy();

        if configured_srcs.contains(relative.as_ref()) {
            continue;
        }

        if !extra_check(&relative) {
            debug!("Keeping {} orphan (still deployed): {}", label, relative);
            continue;
        }

        if dry_run {
            info!("[dry-run] Would remove {} orphan: {}", label, relative);
        } else {
            match fs.remove_file(&entry.path) {
                Ok(()) => {
                    info!("Removed {} orphan: {}", label, relative);
                }
                Err(e) => {
                    warn!("Failed to remove {} orphan: {}", label, relative);
                    errors.push((entry.path.clone(), e));
                }
            }
        }
        count += 1;
    }

    if !dry_run {
        dirs_to_check.sort_by(|a, b| b.cmp(a));
        for dir_path in dirs_to_check {
            if fs.remove_dir(&dir_path).is_err() {
                debug!("Keeping non-empty directory: {}", dir_path.display());
            }
        }
    }

    Ok(CleanResult { count, errors })
}

/// Check if `path` is a symlink pointing to `expected_target`.
fn is_symlink_to(path: &Path, expected_target: &Path, fs: &impl Fs) -> bool {
    match fs.read_link(path) {
        Ok(dest) => dest == expected_target,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeFs;
    use crate::test_helpers::*;

    #[test]
    fn requires_flag() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let result = run(&config, false, false, false, &fs);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("--generated"), "got: {msg}");
    }

    #[test]
    fn clean_generated_removes_all() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "a");
        fs.add_file(format!("{DOTFILES}/.generated/b.conf"), "b");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None)]),
        );
        run(&config, true, false, false, &fs).unwrap();
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.generated/a.conf"))));
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.generated/b.conf"))));
    }

    #[test]
    fn clean_generated_missing_dir_noop() {
        let fs = FakeFs::new(HOME);
        fs.add_dir(DOTFILES);
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), "");
        // No .generated dir
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        run(&config, true, false, false, &fs).unwrap();
    }

    #[test]
    fn clean_generated_dry_run() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "a");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        run(&config, true, false, true, &fs).unwrap();
        // File should still exist
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/a.conf"))));
    }

    #[test]
    fn clean_orphans_removes_unconfigured() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/orphan.conf"), "orphan");
        fs.add_file(format!("{DOTFILES}/.generated/kept.conf"), "kept");
        let config = write_and_load_config(&fs, &make_config_toml(&[("kept.conf", None)]));
        run(&config, false, true, false, &fs).unwrap();
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.generated/orphan.conf"))));
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/kept.conf"))));
    }

    #[test]
    fn clean_orphans_preserves_configured() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "a");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "a");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        run(&config, false, true, false, &fs).unwrap();
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/a.conf"))));
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.staged/a.conf"))));
    }

    #[test]
    fn clean_orphans_preserves_deployed_staged() {
        let fs = setup_fs();
        let staged_path = format!("{DOTFILES}/.staged/orphan.conf");
        let target = format!("{HOME}/.config/orphan.conf");
        fs.add_file(&staged_path, "orphan");
        // Create a symlink at target pointing to staged
        fs.add_symlink(&target, &staged_path);
        // Mark as deployed in state
        let state_toml =
            "[[deployed]]\nsrc = \"orphan.conf\"\ntarget = \"~/.config/orphan.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        run(&config, false, true, false, &fs).unwrap();
        // Staged orphan that is still deployed should be preserved
        assert!(fs.exists(Path::new(&staged_path)));
    }

    #[test]
    fn clean_orphans_nested_dirs() {
        let fs = setup_fs();
        fs.add_file(
            format!("{DOTFILES}/.generated/deep/nested/orphan.conf"),
            "orphan",
        );
        fs.add_file(
            format!("{DOTFILES}/.staged/deep/nested/orphan.conf"),
            "orphan",
        );
        // Not in config → orphan
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        run(&config, false, true, false, &fs).unwrap();
        assert!(!fs.exists(Path::new(&format!(
            "{DOTFILES}/.generated/deep/nested/orphan.conf"
        ))));
        assert!(!fs.exists(Path::new(&format!(
            "{DOTFILES}/.staged/deep/nested/orphan.conf"
        ))));
    }

    #[test]
    fn clean_orphans_removes_undeployed_staged() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.staged/orphan.conf"), "orphan");
        // Not deployed
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        run(&config, false, true, false, &fs).unwrap();
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.staged/orphan.conf"))));
    }
}
