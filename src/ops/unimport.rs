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
