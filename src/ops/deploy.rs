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
use crate::platform::Fs;
use crate::state::{RecoveryInfo, State};

/// Deploy staged files as symlinks to their target paths.
///
/// Bails on the first error. Saves state after each successful deployment
/// with recovery info in case the save itself fails.
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    force: bool,
    dry_run: bool,
    fs: &impl Fs,
) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to deploy");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir(fs);
    let staged_dir = config.staged_dir(fs);
    let mut state = State::load(&dotfiles_dir, fs)?;

    for entry in &entries {
        let staged_path = staged_dir.join(&entry.src);
        let target_path = expand_tilde(&entry.target(), fs);

        if !fs.exists(&staged_path) {
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
            fs.create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        deploy_symlink(&staged_path, &target_path, force, fs)?;

        state.add_deployed(entry.src.clone(), entry.target());
        state.save_with_recovery(
            RecoveryInfo {
                situation: vec![format!(
                    "{} has been deployed to {}",
                    entry.src,
                    target_path.display()
                )],
                consequence: vec![format!(
                    "janus will not know {} is deployed to {}",
                    entry.src,
                    target_path.display()
                )],
                instructions: vec![
                    format!(
                        "Add a [[deployed]] entry to the statefile with src = \"{}\" and target = \"{}\"",
                        entry.src,
                        entry.target()
                    ),
                    format!("Or re-run: janus deploy {}", entry.src),
                ],
            },
            fs,
        )?;
        info!("Deployed {} -> {}", entry.src, target_path.display());
    }

    info!("Deployed {} file(s)", entries.len());
    Ok(())
}

/// Create a symlink from `target_path` -> `staged_path` using atomic rename.
///
/// Creates a temporary symlink (`.janus.tmp`) then renames it over the target
/// so there's never a moment where the file is missing.
#[cfg(feature = "atomic-deploy")]
fn deploy_symlink(staged_path: &Path, target_path: &Path, force: bool, fs: &impl Fs) -> Result<()> {
    let exists = fs.exists(target_path) || fs.is_symlink(target_path);

    // Backup if needed (copy, so the original stays in place until the atomic swap)
    if exists && !force && !is_janus_symlink(target_path, staged_path, fs) {
        let backup_path = backup_path_for(target_path);
        warn!(
            "Backing up existing file: {} -> {}",
            target_path.display(),
            backup_path.display()
        );
        fs.copy(target_path, &backup_path)
            .with_context(|| format!("Failed to backup file: {}", target_path.display()))?;
    } else if exists && force && !is_janus_symlink(target_path, staged_path, fs) {
        warn!("Overwriting existing file: {}", target_path.display());
    }

    // Create a temp symlink in the same directory, then atomically rename over the target
    let temp_path = target_path.with_extension(".janus.tmp");
    // Clean up any stale temp symlink
    if fs.exists(&temp_path) || fs.is_symlink(&temp_path) {
        fs.remove_file(&temp_path).with_context(|| {
            format!(
                "Failed to remove stale temp symlink: {}",
                temp_path.display()
            )
        })?;
    }

    fs.symlink(staged_path, &temp_path)
        .with_context(|| format!("Failed to create temp symlink: {}", temp_path.display()))?;

    fs.rename(&temp_path, target_path).with_context(|| {
        // Clean up temp symlink on failure
        let _ = fs.remove_file(&temp_path);
        format!("Failed to atomically replace: {}", target_path.display())
    })?;

    Ok(())
}

/// Create a symlink from `target_path` -> `staged_path` using remove-then-create.
///
/// Non-atomic fallback: removes the existing file first, then creates the symlink.
#[cfg(not(feature = "atomic-deploy"))]
fn deploy_symlink(staged_path: &Path, target_path: &Path, force: bool, fs: &impl Fs) -> Result<()> {
    if fs.exists(target_path) || fs.is_symlink(target_path) {
        if is_janus_symlink(target_path, staged_path, fs) {
            fs.remove_file(target_path).with_context(|| {
                format!(
                    "Failed to remove existing symlink: {}",
                    target_path.display()
                )
            })?;
        } else if force {
            warn!("Overwriting existing file: {}", target_path.display());
            fs.remove_file(target_path).with_context(|| {
                format!("Failed to remove existing file: {}", target_path.display())
            })?;
        } else {
            let backup_path = backup_path_for(target_path);
            warn!(
                "Backing up existing file: {} -> {}",
                target_path.display(),
                backup_path.display()
            );
            fs.rename(target_path, &backup_path)
                .with_context(|| format!("Failed to backup file: {}", target_path.display()))?;
        }
    }

    fs.symlink(staged_path, target_path).with_context(|| {
        format!(
            "Failed to create symlink: {} -> {}",
            target_path.display(),
            staged_path.display()
        )
    })?;

    Ok(())
}

/// Compute the backup path for a file (e.g. `config.toml` -> `config.toml.janus.bak`).
fn backup_path_for(target_path: &Path) -> std::path::PathBuf {
    target_path.with_extension(format!(
        "{}.janus.bak",
        target_path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    ))
}

use super::is_janus_symlink;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::test_helpers::*;
    use std::path::PathBuf;

    fn deploy_setup(fs: &crate::platform::FakeFs) -> Config {
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "staged content");
        write_and_load_config(
            fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        )
    }

    #[test]
    fn creates_symlink() {
        let fs = setup_fs();
        let config = deploy_setup(&fs);
        run(&config, None, false, false, &fs).unwrap();
        let target = Path::new("/home/test/.config/a.conf");
        assert!(fs.is_symlink(target));
        let link_dest = fs.read_link(target).unwrap();
        assert_eq!(
            link_dest,
            PathBuf::from(format!("{DOTFILES}/.staged/a.conf"))
        );
    }

    #[test]
    fn updates_state() {
        let fs = setup_fs();
        let config = deploy_setup(&fs);
        run(&config, None, false, false, &fs).unwrap();
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(state.is_deployed("a.conf"));
    }

    #[test]
    fn state_saved_per_file() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "a");
        fs.add_file(format!("{DOTFILES}/.staged/b.conf"), "b");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[
                ("a.conf", Some("~/.config/a.conf")),
                ("b.conf", Some("~/.config/b.conf")),
            ]),
        );
        run(&config, None, false, false, &fs).unwrap();
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(state.is_deployed("a.conf"));
        assert!(state.is_deployed("b.conf"));
    }

    #[test]
    fn creates_parent_dirs() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.staged/deep/nested.conf"), "content");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("deep/nested.conf", Some("~/.config/deep/nested.conf"))]),
        );
        run(&config, None, false, false, &fs).unwrap();
        assert!(fs.is_dir(Path::new("/home/test/.config/deep")));
    }

    #[test]
    fn missing_staged_bails() {
        let fs = setup_fs();
        // No staged file
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("missing.conf", Some("~/.config/missing.conf"))]),
        );
        let result = run(&config, None, false, false, &fs);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("Staged file not found") || msg.contains("missing.conf"),
            "got: {msg}"
        );
    }

    #[test]
    fn backup_existing_file() {
        let fs = setup_fs();
        // Put a regular file at the target
        fs.add_file("/home/test/.config/a.conf", "existing content");
        let config = deploy_setup(&fs);
        run(&config, None, false, false, &fs).unwrap();
        // Should have created backup
        assert!(fs.exists(Path::new("/home/test/.config/a.conf.janus.bak")));
    }

    #[test]
    fn force_overwrites() {
        let fs = setup_fs();
        fs.add_file("/home/test/.config/a.conf", "existing content");
        let config = deploy_setup(&fs);
        run(&config, None, true, false, &fs).unwrap();
        // No backup with force
        assert!(!fs.exists(Path::new("/home/test/.config/a.conf.janus.bak")));
        // But symlink should exist
        assert!(fs.is_symlink(Path::new("/home/test/.config/a.conf")));
    }

    #[test]
    fn redeploy_no_backup() {
        let fs = setup_fs();
        let config = deploy_setup(&fs);
        // Deploy once
        run(&config, None, false, false, &fs).unwrap();
        // Deploy again — existing janus symlink should be replaced without backup
        run(&config, None, false, false, &fs).unwrap();
        assert!(!fs.exists(Path::new("/home/test/.config/a.conf.janus.bak")));
        assert!(fs.is_symlink(Path::new("/home/test/.config/a.conf")));
    }

    #[test]
    fn dry_run() {
        let fs = setup_fs();
        let config = deploy_setup(&fs);
        run(&config, None, false, true, &fs).unwrap();
        assert!(!fs.exists(Path::new("/home/test/.config/a.conf")));
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(!state.is_deployed("a.conf"));
    }

    #[test]
    fn backup_nested_path() {
        let fs = setup_fs();
        // Put a regular file at a nested target
        fs.add_file("/home/test/.config/deep/nested.conf", "existing");
        fs.add_file(format!("{DOTFILES}/.staged/deep/nested.conf"), "staged");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("deep/nested.conf", Some("~/.config/deep/nested.conf"))]),
        );
        run(&config, None, false, false, &fs).unwrap();
        // Backup should exist at nested path
        assert!(fs.exists(Path::new("/home/test/.config/deep/nested.conf.janus.bak")));
        assert!(fs.is_symlink(Path::new("/home/test/.config/deep/nested.conf")));
    }

    #[test]
    fn is_janus_symlink_true() {
        let fs = setup_fs();
        let staged = PathBuf::from(format!("{DOTFILES}/.staged/a.conf"));
        let target = Path::new("/home/test/.config/a.conf");
        fs.add_file(&staged, "content");
        fs.add_symlink(target, &staged);
        assert!(is_janus_symlink(target, &staged, &fs));
    }

    #[test]
    fn is_janus_symlink_false() {
        let fs = setup_fs();
        let staged = PathBuf::from(format!("{DOTFILES}/.staged/a.conf"));
        let target = Path::new("/home/test/.config/a.conf");
        // Regular file, not a symlink
        fs.add_file(target, "content");
        assert!(!is_janus_symlink(target, &staged, &fs));
        // Wrong target
        fs.add_symlink("/home/test/.config/b.conf", "/wrong/path");
        assert!(!is_janus_symlink(
            Path::new("/home/test/.config/b.conf"),
            &staged,
            &fs
        ));
    }
}
