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
    let filter = match cli.verbose {
        0 => "janus=info",
        1 => "janus=debug",
        _ => "janus=trace",
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
                Command::Import {
                    path,
                    all,
                    max_depth,
                } => {
                    ops::import::run(&config, &config_path, &path, all, max_depth, cli.dry_run)?;
                }
                Command::Init { .. } => unreachable!(),
            }
        }
    }

    Ok(())
}
