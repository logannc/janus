//! Janus — a two-way dotfile manager.
//!
//! Provides a three-stage pipeline (generate -> stage -> deploy) for managing
//! dotfiles with template rendering, plus reverse operations (import, undeploy,
//! unimport) for bringing existing configs under management or removing them.

mod cli;
mod config;
mod ops;
mod paths;
mod platform;
mod secrets;
mod state;
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod test_helpers;

use anyhow::{Result, bail};
use clap::{CommandFactory, Parser};
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command};
use config::Config;
use platform::{RealFs, RealPrompter, RealSecretEngine};

/// Resolve file selection from explicit files, `--all`, or `--filesets`.
///
/// Exactly one source must be provided. Returns `None` for "all files",
/// or `Some(patterns)` for explicit files or resolved filesets.
fn resolve_file_selection(
    files: Vec<String>,
    all: bool,
    filesets: Vec<String>,
    config: &Config,
) -> Result<Option<Vec<String>>> {
    let sources = [!files.is_empty(), all, !filesets.is_empty()]
        .iter()
        .filter(|&&b| b)
        .count();

    if sources > 1 {
        bail!("Cannot combine explicit files, --all, and --filesets");
    }
    if sources == 0 {
        bail!("Specify files to process, --all, or --filesets");
    }

    if all {
        return Ok(None);
    }
    if !filesets.is_empty() {
        return Ok(Some(config.resolve_filesets(&filesets)?));
    }
    Ok(Some(files))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    // verbosity is a signed level: positive = more verbose, negative = quieter
    let level = cli.verbose as i8 - cli.quiet as i8;
    let filter = match level {
        ..=-3 => "janus=off",
        -2 => "janus=error",
        -1 => "janus=warn",
        0 => "janus=info",
        1 => "janus=debug",
        2.. => "janus=trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .without_time()
        .init();

    let fs = RealFs;
    let engine = RealSecretEngine;
    let prompter = RealPrompter;

    match cli.command {
        Command::Init { dotfiles_dir } => {
            if cli.config.is_some() {
                bail!("--config cannot be used with init (init creates the config)");
            }
            ops::init::run(&dotfiles_dir, cli.dry_run, &fs, &engine)?;
        }
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "janus", &mut std::io::stdout());
        }
        command => {
            let config_path = cli.config.unwrap_or_else(|| Config::default_path(&fs));
            let config = Config::load(&config_path, &fs)?;

            match command {
                Command::Generate {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::generate::run(&config, files.as_deref(), cli.dry_run, &fs, &engine)?;
                }
                Command::Stage {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::stage::run(&config, files.as_deref(), cli.dry_run, &fs)?;
                }
                Command::Deploy {
                    files,
                    all,
                    force,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::deploy::run(&config, files.as_deref(), force, cli.dry_run, &fs)?;
                }
                Command::Diff {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::diff::run(&config, files.as_deref(), &fs)?;
                }
                Command::Clean { generated, orphans } => {
                    ops::clean::run(&config, generated, orphans, cli.dry_run, &fs)?;
                }
                Command::Import {
                    path,
                    all,
                    max_depth,
                } => {
                    ops::import::run(
                        &config,
                        &config_path,
                        &path,
                        all,
                        max_depth,
                        cli.dry_run,
                        &fs,
                        &engine,
                        &prompter,
                    )?;
                }
                Command::Apply {
                    files,
                    all,
                    force,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::apply::run(&config, files.as_deref(), force, cli.dry_run, &fs, &engine)?;
                }
                Command::Undeploy {
                    files,
                    all,
                    remove_file,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::undeploy::run(&config, files.as_deref(), remove_file, cli.dry_run, &fs)?;
                }
                Command::Unimport {
                    files,
                    remove_file,
                    filesets,
                } => {
                    let files = if !filesets.is_empty() {
                        if !files.is_empty() {
                            bail!("Cannot combine explicit files and --filesets");
                        }
                        config.resolve_filesets(&filesets)?
                    } else {
                        if files.is_empty() {
                            bail!("Specify files to unimport or use --filesets");
                        }
                        files
                    };
                    ops::unimport::run(
                        &config,
                        &config_path,
                        &files,
                        remove_file,
                        cli.dry_run,
                        &fs,
                    )?;
                }
                Command::Sync {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::sync::run(&config, files.as_deref(), cli.dry_run, &fs, &prompter)?;
                }
                Command::Status {
                    files,
                    all,
                    only_diffs,
                    deployed,
                    undeployed,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::status::run(
                        &config,
                        files.as_deref(),
                        ops::status::StatusFilters {
                            only_diffs,
                            deployed,
                            undeployed,
                        },
                        &fs,
                    )?;
                }
                Command::Init { .. } | Command::Completions { .. } => unreachable!(),
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    fn test_config() -> Config {
        let fs = setup_fs();
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"

[[files]]
src = "a.conf"

[[files]]
src = "hypr/hypr.conf"

[filesets.desktop]
patterns = ["hypr/*"]
"#
        );
        write_and_load_config(&fs, &toml)
    }

    #[test]
    fn all_returns_none() {
        let config = test_config();
        let result = resolve_file_selection(vec![], true, vec![], &config).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn explicit_files() {
        let config = test_config();
        let result =
            resolve_file_selection(vec!["a.conf".to_string()], false, vec![], &config).unwrap();
        assert_eq!(result, Some(vec!["a.conf".to_string()]));
    }

    #[test]
    fn filesets_resolved() {
        let config = test_config();
        let result =
            resolve_file_selection(vec![], false, vec!["desktop".to_string()], &config).unwrap();
        assert_eq!(result, Some(vec!["hypr/*".to_string()]));
    }

    #[test]
    fn no_source_errors() {
        let config = test_config();
        let result = resolve_file_selection(vec![], false, vec![], &config);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("Specify"), "got: {msg}");
    }

    #[test]
    fn multiple_sources_errors() {
        let config = test_config();
        let result = resolve_file_selection(vec!["a.conf".to_string()], true, vec![], &config);
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("Cannot combine"), "got: {msg}");
    }
}
