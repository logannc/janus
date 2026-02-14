mod cli;
mod config;
mod ops;
mod paths;
mod state;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command};
use config::Config;

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
                Command::Generate { files } => {
                    ops::generate::run(&config, &files, cli.dry_run)?;
                }
                Command::Stage { files } => {
                    ops::stage::run(&config, &files, cli.dry_run)?;
                }
                Command::Deploy { files, force } => {
                    ops::deploy::run(&config, &files, force, cli.dry_run)?;
                }
                Command::Diff { files } => {
                    ops::diff::run(&config, &files)?;
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
