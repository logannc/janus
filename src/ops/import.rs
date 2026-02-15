//! Import existing config files into janus management.
//!
//! Takes a file or directory path, walks it (with configurable depth), and for
//! each file: prompts the user (Import/Ignore/Skip), copies it into the dotfiles
//! directory, adds a `[[files]]` entry to the config, and runs the full forward
//! pipeline (generate → stage → deploy).
//!
//! Uses fail-fast strategy since each file mutates config, state, and the filesystem.

use anyhow::{Context, Result};
use dialoguer::Select;
use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::config::Config;
use crate::paths::{collapse_tilde, expand_tilde};
use crate::state::{RecoveryInfo, State};

/// Import files from the given path into janus management.
///
/// If `import_all` is true, skips interactive prompts and imports everything.
/// Each imported file is immediately deployed (generate → stage → deploy).
pub fn run(
    config: &Config,
    config_path: &Path,
    path: &str,
    import_all: bool,
    max_depth: usize,
    dry_run: bool,
) -> Result<()> {
    let source_path = expand_tilde(path);
    let dotfiles_dir = config.dotfiles_dir();
    let mut state = State::load(&dotfiles_dir)?;

    if !source_path.exists() {
        anyhow::bail!("Path does not exist: {}", source_path.display());
    }

    let files: Vec<PathBuf> = if source_path.is_dir() {
        WalkDir::new(&source_path)
            .max_depth(max_depth)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect()
    } else {
        vec![source_path.clone()]
    };

    if files.is_empty() {
        info!("No files found to import");
        return Ok(());
    }

    info!("Found {} file(s) to consider", files.len());

    let managed_targets: HashSet<PathBuf> = config
        .files
        .iter()
        .map(|f| expand_tilde(&f.target()))
        .collect();

    for file_path in &files {
        let target_str = collapse_tilde(file_path);

        // Check if already managed
        if managed_targets.contains(file_path) {
            debug!("Already managed, skipping: {}", target_str);
            continue;
        }

        // Check if ignored
        if state.is_ignored(&target_str) {
            debug!("Already ignored, skipping: {}", target_str);
            continue;
        }

        if !import_all {
            let selection = Select::new()
                .with_prompt(format!("Import {}?", target_str))
                .items(&["Import", "Ignore", "Skip"])
                .default(0)
                .interact()
                .context("Failed to get user input")?;

            match selection {
                0 => {} // Import - continue below
                1 => {
                    // Ignore
                    state.add_ignored(target_str.clone(), "user_declined".to_string());
                    state.save_with_recovery(RecoveryInfo {
                        situation: vec![
                            format!("{target_str} was marked as ignored"),
                        ],
                        consequence: vec![
                            format!("{target_str} will be prompted again on next import"),
                        ],
                        instructions: vec![
                            format!("Add an [[ignored]] entry to the statefile with path = \"{target_str}\""),
                        ],
                    })?;
                    info!("Ignored {}", target_str);
                    continue;
                }
                _ => {
                    // Skip
                    debug!("Skipped {}", target_str);
                    continue;
                }
            }
        }

        import_file(
            file_path,
            &target_str,
            &dotfiles_dir,
            config_path,
            &mut state,
            dry_run,
        )?;
    }

    Ok(())
}

/// Import a single file: copy to dotfiles dir, add config entry, run pipeline.
fn import_file(
    file_path: &Path,
    target_str: &str,
    dotfiles_dir: &Path,
    config_path: &Path,
    state: &mut State,
    dry_run: bool,
) -> Result<()> {
    // Determine destination path in dotfiles dir
    let dest_relative = determine_dest_path(file_path)?;
    let dest_path = dotfiles_dir.join(&dest_relative);

    if dest_path.exists() {
        anyhow::bail!(
            "Destination already exists: {} (would overwrite existing source file)",
            dest_path.display()
        );
    }

    if dry_run {
        info!("[dry-run] Would import: {} -> {}", target_str, dest_relative);
        return Ok(());
    }

    // Copy file to dotfiles dir
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    std::fs::copy(file_path, &dest_path)
        .with_context(|| format!("Failed to copy file: {}", file_path.display()))?;

    // Preserve permissions
    let metadata = std::fs::metadata(file_path)
        .with_context(|| format!("Failed to read metadata: {}", file_path.display()))?;
    std::fs::set_permissions(
        &dest_path,
        std::fs::Permissions::from_mode(metadata.permissions().mode()),
    )
    .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

    // Append config entry using toml_edit
    append_config_entry(config_path, &dest_relative, target_str)?;

    // Generate, stage, and deploy
    let config = crate::config::Config::load(config_path)?;
    let file_patterns = vec![dest_relative.clone()];

    crate::ops::generate::run(&config, Some(&file_patterns), false)?;
    crate::ops::stage::run(&config, Some(&file_patterns), false)?;
    crate::ops::deploy::run(&config, Some(&file_patterns), true, false)?;

    state.add_deployed(dest_relative.clone(), target_str.to_string());
    state.save_with_recovery(RecoveryInfo {
        situation: vec![
            format!("{target_str} has been imported and deployed"),
        ],
        consequence: vec![
            format!("janus will not know {target_str} is deployed"),
            "The file is already in the dotfiles dir and config".to_string(),
        ],
        instructions: vec![
            format!("Add a [[deployed]] entry to the statefile with src = \"{dest_relative}\" and target = \"{target_str}\""),
            format!("Or re-run: janus deploy {dest_relative}"),
        ],
    })?;
    info!("Imported {}", target_str);
    Ok(())
}

/// Determine the relative destination path within the dotfiles directory.
///
/// Resolution order:
/// 1. Files under `~/.config/` → strip that prefix (e.g. `~/.config/hypr/hypr.conf` → `hypr/hypr.conf`)
/// 2. Files under `~/` → strip home + leading dot (e.g. `~/.bashrc` → `bashrc`)
/// 3. Files elsewhere → flatten parent with underscores (e.g. `/etc/systemd/system/foo.service` → `etc_systemd_system/foo.service`)
fn determine_dest_path(file_path: &Path) -> Result<String> {
    let config_dir = dirs::config_dir().unwrap_or_else(|| expand_tilde("~/.config"));

    // Files under ~/.config/ → strip that prefix, preserving subdirectory structure
    if let Ok(relative) = file_path.strip_prefix(&config_dir) {
        return Ok(relative.display().to_string());
    }

    // Files under ~/ → use relative path, stripping leading dot from hidden dirs
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = file_path.strip_prefix(&home) {
            let rel_str = relative.display().to_string();
            let stripped = rel_str.strip_prefix('.').unwrap_or(&rel_str);
            return Ok(stripped.to_string());
        }
    }

    // Fallback for files outside ~/ (e.g. /etc/systemd/system/foo.service):
    // flatten the parent directory into a single component with underscores
    let file_name = file_path
        .file_name()
        .context("File has no name")?
        .to_string_lossy()
        .to_string();

    let parent = file_path
        .parent()
        .context("File has no parent directory")?;

    // Strip leading / and replace path separators with underscores
    let parent_flat = parent
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "_");

    Ok(format!("{parent_flat}/{file_name}"))
}

/// Append a `[[files]]` entry to the config file using `toml_edit` to preserve formatting.
fn append_config_entry(config_path: &Path, src: &str, target: &str) -> Result<()> {
    let contents = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| "Failed to parse config for editing")?;

    // Get or create the [[files]] array
    let files = doc
        .entry("files")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));

    if let Some(array) = files.as_array_of_tables_mut() {
        let mut table = toml_edit::Table::new();
        table.insert("src", toml_edit::value(src));
        let default_target = format!("~/.config/{src}");
        if target != default_target {
            table.insert("target", toml_edit::value(target));
        }
        array.push(table);
    } else {
        warn!("Config 'files' is not an array of tables; cannot append entry");
        anyhow::bail!("Config 'files' field is malformed");
    }

    std::fs::write(config_path, doc.to_string())
        .with_context(|| format!("Failed to write config: {}", config_path.display()))?;

    debug!("Added config entry: src={}, target={}", src, target);
    Ok(())
}
