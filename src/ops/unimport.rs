//! Full reversal of import: undeploy, remove config entry, and clean up all
//! copies of a file (source, generated, staged).
//!
//! By default, leaves a regular file at the target path (safety by default).
//! With `--remove-file`, the target is deleted entirely.
//!
//! Intentionally has no `--all` flag — unimporting everything is too destructive.
//! Requires an explicit file list.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::platform::Fs;
use crate::state::State;

/// Unimport files: undeploy, remove config entry, delete source/generated/staged copies.
///
/// For each matched file:
/// 1. Undeploy if currently deployed (respects `remove_file` flag)
/// 2. Remove the `[[files]]` config entry via `toml_edit`
/// 3. Delete source, generated, and staged files
/// 4. Remove any corresponding ignored entry from state
/// 5. Save state
pub fn run(
    config: &Config,
    config_path: &Path,
    files: &[String],
    remove_file: bool,
    dry_run: bool,
    fs: &impl Fs,
) -> Result<()> {
    if files.is_empty() {
        anyhow::bail!("Specify files to unimport");
    }

    let dotfiles_dir = config.dotfiles_dir(fs);
    let generated_dir = config.generated_dir(fs);
    let staged_dir = config.staged_dir(fs);
    let mut state = State::load(&dotfiles_dir, fs)?;

    let entries = config.filter_files(Some(files));
    if entries.is_empty() {
        config.bail_unmatched(Some(files))?;
    }

    for entry in &entries {
        let src = &entry.src;
        let target_path = expand_tilde(&entry.target(), fs);

        if dry_run {
            info!("[dry-run] Would unimport: {}", src);
            continue;
        }

        // 1. Undeploy if currently deployed
        if state.is_deployed(src) {
            super::undeploy::undeploy_single(
                src,
                &staged_dir,
                &target_path,
                remove_file,
                &mut state,
                fs,
            )?;
        }

        // 2. Remove config entry
        remove_config_entry(config_path, src, fs)?;

        // 3. Remove source file from dotfiles dir
        let source_path = dotfiles_dir.join(src);
        if fs.exists(&source_path) {
            fs.remove_file(&source_path).with_context(|| {
                format!("Failed to remove source file: {}", source_path.display())
            })?;
            // Clean up empty parent directories
            remove_empty_parents(&source_path, &dotfiles_dir, fs);
            debug!("Removed source: {}", source_path.display());
        }

        // 4. Remove generated file
        let generated_path = generated_dir.join(src);
        if fs.exists(&generated_path) {
            fs.remove_file(&generated_path).with_context(|| {
                format!(
                    "Failed to remove generated file: {}",
                    generated_path.display()
                )
            })?;
            remove_empty_parents(&generated_path, &generated_dir, fs);
            debug!("Removed generated: {}", generated_path.display());
        }

        // 5. Remove staged file
        let staged_path = staged_dir.join(src);
        if fs.exists(&staged_path) {
            fs.remove_file(&staged_path).with_context(|| {
                format!("Failed to remove staged file: {}", staged_path.display())
            })?;
            remove_empty_parents(&staged_path, &staged_dir, fs);
            debug!("Removed staged: {}", staged_path.display());
        }

        state
            .save(fs)
            .with_context(|| format!("Failed to save state after unimporting {}", src))?;

        info!("Unimported {}", src);
    }

    Ok(())
}

/// Remove the `[[files]]` entry matching `src` from the config file.
///
/// Uses `toml_edit` to preserve formatting and comments in the config.
/// Warns (but doesn't error) if no matching entry is found.
fn remove_config_entry(config_path: &Path, src: &str, fs: &impl Fs) -> Result<()> {
    let contents = fs
        .read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| "Failed to parse config for editing")?;

    if let Some(files) = doc.get_mut("files")
        && let Some(array) = files.as_array_of_tables_mut()
    {
        // Find and remove the entry matching src
        let mut index_to_remove = None;
        for (i, table) in array.iter().enumerate() {
            if let Some(entry_src) = table.get("src").and_then(|v| v.as_str())
                && entry_src == src
            {
                index_to_remove = Some(i);
                break;
            }
        }

        if let Some(idx) = index_to_remove {
            array.remove(idx);
        } else {
            warn!("Config entry not found for src: {}", src);
        }
    }

    fs.write(config_path, doc.to_string().as_bytes())
        .with_context(|| format!("Failed to write config: {}", config_path.display()))?;

    debug!("Removed config entry: src={}", src);
    Ok(())
}

/// Remove empty parent directories up to (but not including) the stop directory.
fn remove_empty_parents(path: &Path, stop_at: &Path, fs: &impl Fs) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == stop_at {
            break;
        }
        if fs.remove_dir(dir).is_err() {
            break; // Not empty or other error, stop
        }
        current = dir.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::test_helpers::*;

    fn setup_managed_file(fs: &crate::platform::FakeFs) -> Config {
        // Source file
        fs.add_file(format!("{DOTFILES}/a.conf"), "source");
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "generated");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "staged");
        // Deploy it
        let staged = format!("{DOTFILES}/.staged/a.conf");
        fs.add_symlink("/home/test/.config/a.conf", &staged);
        let state_toml =
            "[[deployed]]\nsrc = \"a.conf\"\ntarget = \"~/.config/a.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        write_and_load_config(
            fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        )
    }

    #[test]
    fn full_reversal() {
        let fs = setup_fs();
        let config = setup_managed_file(&fs);
        let files = vec!["a.conf".to_string()];
        run(&config, Path::new(CONFIG_PATH), &files, false, false, &fs).unwrap();
        // Source, generated, staged should be removed
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/a.conf"))));
        assert!(!fs.exists(Path::new(&format!(
            "{DOTFILES}/.generated/a.conf"
        ))));
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.staged/a.conf"))));
        // Target should have a copy (not removed by default)
        assert!(fs.is_file(Path::new("/home/test/.config/a.conf")));
        assert!(!fs.is_symlink(Path::new("/home/test/.config/a.conf")));
        // State should be updated
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(!state.is_deployed("a.conf"));
    }

    #[test]
    fn not_deployed() {
        let fs = setup_fs();
        // Source file but NOT deployed
        fs.add_file(format!("{DOTFILES}/a.conf"), "source");
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "generated");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "staged");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        );
        let files = vec!["a.conf".to_string()];
        run(&config, Path::new(CONFIG_PATH), &files, false, false, &fs).unwrap();
        // Files should still be cleaned up
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/a.conf"))));
    }

    #[test]
    fn empty_files_errors() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let result = run(&config, Path::new(CONFIG_PATH), &[], false, false, &fs);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("No files") || msg.contains("Specify"), "got: {msg}");
    }

    #[test]
    fn dry_run() {
        let fs = setup_fs();
        let config = setup_managed_file(&fs);
        let files = vec!["a.conf".to_string()];
        run(&config, Path::new(CONFIG_PATH), &files, false, true, &fs).unwrap();
        // Nothing should be removed
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/a.conf"))));
        assert!(fs.is_symlink(Path::new("/home/test/.config/a.conf")));
    }

    #[test]
    fn removes_empty_parent_dirs() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/deep/nested/a.conf"), "source");
        fs.add_file(format!("{DOTFILES}/.generated/deep/nested/a.conf"), "gen");
        fs.add_file(format!("{DOTFILES}/.staged/deep/nested/a.conf"), "staged");
        let staged = format!("{DOTFILES}/.staged/deep/nested/a.conf");
        fs.add_symlink("/home/test/.config/deep/nested/a.conf", &staged);
        let state_toml = "[[deployed]]\nsrc = \"deep/nested/a.conf\"\ntarget = \"~/.config/deep/nested/a.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[(
                "deep/nested/a.conf",
                Some("~/.config/deep/nested/a.conf"),
            )]),
        );
        let files = vec!["deep/nested/a.conf".to_string()];
        run(&config, Path::new(CONFIG_PATH), &files, false, false, &fs).unwrap();
        // Parent dirs should be removed since they're empty
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/deep/nested"))));
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/deep"))));
    }

    #[test]
    fn removes_config_entry() {
        let fs = setup_fs();
        let config = setup_managed_file(&fs);
        let files = vec!["a.conf".to_string()];
        run(&config, Path::new(CONFIG_PATH), &files, false, false, &fs).unwrap();
        // Config should no longer contain the entry
        let content = fs.read_to_string(Path::new(CONFIG_PATH)).unwrap();
        assert!(!content.contains("a.conf"), "config still contains a.conf: {content}");
    }

    #[test]
    fn remove_config_entry_missing_warns() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/a.conf"), "source");
        let toml = make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]);
        fs.add_file(CONFIG_PATH, toml.as_str());
        // Removing a non-existent entry should warn but not error
        super::remove_config_entry(Path::new(CONFIG_PATH), "nonexistent.conf", &fs).unwrap();
    }
}
