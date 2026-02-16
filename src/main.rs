//! Janus — a two-way dotfile manager.
//!
//! Provides a three-stage pipeline (generate → stage → deploy) for managing
//! dotfiles with template rendering, plus reverse operations (import, undeploy,
//! unimport) for bringing existing configs under management or removing them.

mod cli;
mod config;
mod ops;
mod paths;
mod platform;
mod secrets;
mod state;

use anyhow::{Result, bail};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command};
use config::Config;

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

    match cli.command {
        Command::Init { dotfiles_dir } => {
            ops::init::run(&dotfiles_dir, cli.dry_run)?;
        }
        command => {
            let config_path = cli.config.unwrap_or_else(Config::default_path);
            let config = Config::load(&config_path)?;

            match command {
                Command::Generate {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::generate::run(&config, files.as_deref(), cli.dry_run)?;
                }
                Command::Stage {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::stage::run(&config, files.as_deref(), cli.dry_run)?;
                }
                Command::Deploy {
                    files,
                    all,
                    force,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::deploy::run(&config, files.as_deref(), force, cli.dry_run)?;
                }
                Command::Diff {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::diff::run(&config, files.as_deref())?;
                }
                Command::Clean { generated, orphans } => {
                    ops::clean::run(&config, generated, orphans, cli.dry_run)?;
                }
                Command::Import {
                    path,
                    all,
                    max_depth,
                } => {
                    ops::import::run(&config, &config_path, &path, all, max_depth, cli.dry_run)?;
                }
                Command::Apply {
                    files,
                    all,
                    force,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::apply::run(&config, files.as_deref(), force, cli.dry_run)?;
                }
                Command::Undeploy {
                    files,
                    all,
                    remove_file,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::undeploy::run(&config, files.as_deref(), remove_file, cli.dry_run)?;
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
                    ops::unimport::run(&config, &config_path, &files, remove_file, cli.dry_run)?;
                }
                Command::Sync {
                    files,
                    all,
                    filesets,
                } => {
                    let files = resolve_file_selection(files, all, filesets, &config)?;
                    ops::sync::run(&config, files.as_deref(), cli.dry_run)?;
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
                    )?;
                }
                Command::Init { .. } => unreachable!(),
            }
        }
    }

    Ok(())
}
