mod cli;
mod config;
mod ops;
mod paths;
mod state;

use anyhow::{bail, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command};
use config::Config;

/// Validate that either explicit files or `--all` was provided.
/// Returns `None` to mean "all files".
fn require_files_or_all(files: Vec<String>, all: bool) -> Result<Option<Vec<String>>> {
    if all && !files.is_empty() {
        bail!("Cannot specify both --all and explicit files");
    }
    if !all && files.is_empty() {
        bail!("Specify files to process, or use --all");
    }
    if all { Ok(None) } else { Ok(Some(files)) }
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
                Command::Generate { files, all } => {
                    let files = require_files_or_all(files, all)?;
                    ops::generate::run(&config, files.as_deref(), cli.dry_run)?;
                }
                Command::Stage { files, all } => {
                    let files = require_files_or_all(files, all)?;
                    ops::stage::run(&config, files.as_deref(), cli.dry_run)?;
                }
                Command::Deploy { files, all, force } => {
                    let files = require_files_or_all(files, all)?;
                    ops::deploy::run(&config, files.as_deref(), force, cli.dry_run)?;
                }
                Command::Diff { files, all } => {
                    let files = require_files_or_all(files, all)?;
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
                Command::Apply { files, all, force } => {
                    let files = require_files_or_all(files, all)?;
                    ops::apply::run(&config, files.as_deref(), force, cli.dry_run)?;
                }
                Command::Undeploy {
                    files,
                    all,
                    remove_file,
                } => {
                    let files = require_files_or_all(files, all)?;
                    ops::undeploy::run(&config, files.as_deref(), remove_file, cli.dry_run)?;
                }
                Command::Unimport {
                    files,
                    remove_file,
                } => {
                    if files.is_empty() {
                        anyhow::bail!("Specify files to unimport");
                    }
                    ops::unimport::run(
                        &config,
                        &config_path,
                        &files,
                        remove_file,
                        cli.dry_run,
                    )?;
                }
                Command::Status {
                    files,
                    all,
                    only_diffs,
                    deployed,
                    undeployed,
                } => {
                    let files = require_files_or_all(files, all)?;
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
