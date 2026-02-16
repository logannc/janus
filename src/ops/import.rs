//! Import existing config files into janus management.
//!
//! Takes a file or directory path, walks it (with configurable depth), and for
//! each file: prompts the user (Import/Ignore/Skip), copies it into the dotfiles
//! directory, adds a `[[files]]` entry to the config, and runs the full forward
//! pipeline (generate -> stage -> deploy).
//!
//! Uses fail-fast strategy since each file mutates config, state, and the filesystem.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::paths::{collapse_tilde, expand_tilde};
use crate::platform::{Fs, Prompter, SecretEngine, WalkOptions};
use crate::state::{RecoveryInfo, State};

/// Import files from the given path into janus management.
///
/// If `import_all` is true, skips interactive prompts and imports everything.
/// Each imported file is immediately deployed (generate -> stage -> deploy).
pub fn run(
    config: &Config,
    config_path: &Path,
    path: &str,
    import_all: bool,
    max_depth: usize,
    dry_run: bool,
    fs: &impl Fs,
    engine: &impl SecretEngine,
    prompter: &impl Prompter,
) -> Result<()> {
    let source_path = expand_tilde(path, fs);
    let dotfiles_dir = config.dotfiles_dir(fs);
    let mut state = State::load(&dotfiles_dir, fs)?;

    if !fs.exists(&source_path) {
        anyhow::bail!("Path does not exist: {}", source_path.display());
    }

    let files: Vec<PathBuf> = if fs.is_dir(&source_path) {
        fs.walk_dir(
            &source_path,
            &WalkOptions {
                max_depth: Some(max_depth),
                follow_links: true,
                ..Default::default()
            },
        )?
        .into_iter()
        .filter(|e| e.is_file)
        .map(|e| e.path)
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
        .map(|f| expand_tilde(&f.target(), fs))
        .collect();

    for file_path in &files {
        let target_str = collapse_tilde(file_path, fs);

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
            let selection = prompter.select(
                &format!("Import {}?", target_str),
                &["Import", "Ignore", "Skip"],
                0,
            )?;

            match selection {
                0 => {} // Import - continue below
                1 => {
                    // Ignore
                    state.add_ignored(target_str.clone(), "user_declined".to_string());
                    state.save_with_recovery(
                        RecoveryInfo {
                            situation: vec![format!("{target_str} was marked as ignored")],
                            consequence: vec![format!(
                                "{target_str} will be prompted again on next import"
                            )],
                            instructions: vec![format!(
                                "Add an [[ignored]] entry to the statefile with path = \"{target_str}\""
                            )],
                        },
                        fs,
                    )?;
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
            fs,
            engine,
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
    fs: &impl Fs,
    engine: &impl SecretEngine,
) -> Result<()> {
    // Determine destination path in dotfiles dir
    let dest_relative = determine_dest_path(file_path, fs)?;
    let dest_path = dotfiles_dir.join(&dest_relative);

    if fs.exists(&dest_path) {
        anyhow::bail!(
            "Destination already exists: {} (would overwrite existing source file)",
            dest_path.display()
        );
    }

    if dry_run {
        info!(
            "[dry-run] Would import: {} -> {}",
            target_str, dest_relative
        );
        return Ok(());
    }

    // Copy file to dotfiles dir
    if let Some(parent) = dest_path.parent() {
        fs.create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    fs.copy(file_path, &dest_path)
        .with_context(|| format!("Failed to copy file: {}", file_path.display()))?;

    // Preserve permissions
    let mode = fs
        .file_mode(file_path)
        .with_context(|| format!("Failed to read metadata: {}", file_path.display()))?;
    fs.set_file_mode(&dest_path, mode)
        .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

    // Append config entry using toml_edit
    append_config_entry(config_path, &dest_relative, target_str, fs)?;

    // Generate, stage, and deploy
    let config = crate::config::Config::load(config_path, fs)?;
    let file_patterns = vec![dest_relative.clone()];

    crate::ops::generate::run(&config, Some(&file_patterns), false, fs, engine)?;
    crate::ops::stage::run(&config, Some(&file_patterns), false, fs)?;
    crate::ops::deploy::run(&config, Some(&file_patterns), true, false, fs)?;

    state.add_deployed(dest_relative.clone(), target_str.to_string());
    state.save_with_recovery(
        RecoveryInfo {
            situation: vec![format!("{target_str} has been imported and deployed")],
            consequence: vec![
                format!("janus will not know {target_str} is deployed"),
                "The file is already in the dotfiles dir and config".to_string(),
            ],
            instructions: vec![
                format!(
                    "Add a [[deployed]] entry to the statefile with src = \"{dest_relative}\" and target = \"{target_str}\""
                ),
                format!("Or re-run: janus deploy {dest_relative}"),
            ],
        },
        fs,
    )?;
    info!("Imported {}", target_str);
    Ok(())
}

/// Determine the relative destination path within the dotfiles directory.
///
/// Resolution order:
/// 1. Files under `~/.config/` -> strip that prefix (e.g. `~/.config/hypr/hypr.conf` -> `hypr/hypr.conf`)
/// 2. Files under `~/` -> strip home + leading dot (e.g. `~/.bashrc` -> `bashrc`)
/// 3. Files elsewhere -> flatten parent with underscores (e.g. `/etc/systemd/system/foo.service` -> `etc_systemd_system/foo.service`)
fn determine_dest_path(file_path: &Path, fs: &impl Fs) -> Result<String> {
    let config_dir = fs
        .config_dir()
        .unwrap_or_else(|| expand_tilde("~/.config", fs));

    // Files under ~/.config/ -> strip that prefix, preserving subdirectory structure
    if let Ok(relative) = file_path.strip_prefix(&config_dir) {
        return Ok(relative.display().to_string());
    }

    // Files under ~/ -> use relative path, stripping leading dot from hidden dirs
    if let Some(home) = fs.home_dir()
        && let Ok(relative) = file_path.strip_prefix(&home)
    {
        let rel_str = relative.display().to_string();
        let stripped = rel_str.strip_prefix('.').unwrap_or(&rel_str);
        return Ok(stripped.to_string());
    }

    // Fallback for files outside ~/ (e.g. /etc/systemd/system/foo.service):
    // flatten the parent directory into a single component with underscores
    let file_name = file_path
        .file_name()
        .context("File has no name")?
        .to_string_lossy()
        .to_string();

    let parent = file_path.parent().context("File has no parent directory")?;

    // Strip leading / and replace path separators with underscores
    let parent_flat = parent
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "_");

    Ok(format!("{parent_flat}/{file_name}"))
}

/// Append a `[[files]]` entry to the config file using `toml_edit` to preserve formatting.
fn append_config_entry(
    config_path: &Path,
    src: &str,
    target: &str,
    fs: &impl Fs,
) -> Result<()> {
    let contents = fs
        .read_to_string(config_path)
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

    fs.write(config_path, doc.to_string().as_bytes())
        .with_context(|| format!("Failed to write config: {}", config_path.display()))?;

    debug!("Added config entry: src={}, target={}", src, target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{FakePrompter, FakeSecretEngine};
    use crate::state::State;
    use crate::test_helpers::*;

    fn make_engine() -> FakeSecretEngine {
        FakeSecretEngine::new()
    }

    #[test]
    fn dest_path_under_config() {
        let fs = crate::platform::FakeFs::new("/home/test");
        let result = determine_dest_path(Path::new("/home/test/.config/hypr/f"), &fs).unwrap();
        assert_eq!(result, "hypr/f");
    }

    #[test]
    fn dest_path_under_home() {
        let fs = crate::platform::FakeFs::new("/home/test");
        let result = determine_dest_path(Path::new("/home/test/.bashrc"), &fs).unwrap();
        assert_eq!(result, "bashrc");
    }

    #[test]
    fn dest_path_outside_home() {
        let fs = crate::platform::FakeFs::new("/home/test");
        let result =
            determine_dest_path(Path::new("/etc/systemd/system/foo.service"), &fs).unwrap();
        assert_eq!(result, "etc_systemd_system/foo.service");
    }

    #[test]
    fn import_single_file() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        // File to import
        fs.add_file("/home/test/.config/hypr/hypr.conf", "monitor=DP-1");
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![0]); // Import
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/hypr/hypr.conf",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
        // File should be copied to dotfiles dir
        assert!(fs.exists(Path::new(&format!(
            "{DOTFILES}/hypr/hypr.conf"
        ))));
        // Should be deployed
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(state.is_deployed("hypr/hypr.conf"));
    }

    #[test]
    fn skips_already_managed() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/a.conf", "content");
        // a.conf is already in config
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        );
        let prompter = FakePrompter::new(vec![]); // No prompts expected
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/a.conf",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
    }

    #[test]
    fn skips_ignored() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/ignored.conf", "content");
        let state_toml =
            "[[ignored]]\npath = \"~/.config/ignored.conf\"\nreason = \"user_declined\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![]); // No prompts
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/ignored.conf",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
    }

    #[test]
    fn user_ignores() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/new.conf", "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![1]); // Ignore
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/new.conf",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(state.is_ignored("~/.config/new.conf"));
    }

    #[test]
    fn user_skips() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/skip.conf", "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![2]); // Skip
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/skip.conf",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
        // Should not be ignored or imported
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(!state.is_ignored("~/.config/skip.conf"));
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/skip.conf"))));
    }

    #[test]
    fn import_all_no_prompt() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/auto.conf", "auto");
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![]); // No prompts expected
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/auto.conf",
            true, // import_all
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/auto.conf"))));
    }

    #[test]
    fn nonexistent_path_errors() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![]);
        let result = run(
            &config,
            Path::new(CONFIG_PATH),
            "/nonexistent/file",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        );
        assert!(result.is_err());
    }

    #[test]
    fn destination_exists_errors() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/dup.conf", "original");
        // Destination already exists in dotfiles dir
        fs.add_file(format!("{DOTFILES}/dup.conf"), "existing");
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![0]); // Import
        let result = run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/dup.conf",
            false,
            10,
            false,
            &fs,
            &make_engine(),
            &prompter,
        );
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("already exists"), "got: {msg}");
    }

    #[test]
    fn dry_run() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file("/home/test/.config/dry.conf", "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[]));
        let prompter = FakePrompter::new(vec![0]); // Import
        run(
            &config,
            Path::new(CONFIG_PATH),
            "~/.config/dry.conf",
            false,
            10,
            true, // dry_run
            &fs,
            &make_engine(),
            &prompter,
        )
        .unwrap();
        // Nothing should be written
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/dry.conf"))));
    }

    #[test]
    fn append_config_entry_default_target() {
        let fs = setup_fs();
        let toml = make_config_toml(&[]);
        fs.add_file(CONFIG_PATH, toml.as_str());
        append_config_entry(Path::new(CONFIG_PATH), "hypr/hypr.conf", "~/.config/hypr/hypr.conf", &fs)
            .unwrap();
        let content = fs.read_to_string(Path::new(CONFIG_PATH)).unwrap();
        // target should be omitted since it matches the default
        assert!(content.contains("src = \"hypr/hypr.conf\""));
        assert!(!content.contains("target = "));
    }

    #[test]
    fn append_config_entry_custom_target() {
        let fs = setup_fs();
        let toml = make_config_toml(&[]);
        fs.add_file(CONFIG_PATH, toml.as_str());
        append_config_entry(Path::new(CONFIG_PATH), "bashrc", "~/.bashrc", &fs).unwrap();
        let content = fs.read_to_string(Path::new(CONFIG_PATH)).unwrap();
        assert!(content.contains("src = \"bashrc\""));
        assert!(content.contains("target = \"~/.bashrc\""));
    }
}
