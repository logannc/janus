//! Command-line interface definitions using `clap` derive macros.
//!
//! The [`Cli`] struct is the top-level parser, and [`Command`] enumerates all
//! available subcommands. Each variant's fields map directly to CLI arguments.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Top-level CLI arguments shared across all subcommands.
#[derive(Parser)]
#[command(name = "janus", about = "Two-way dotfile manager")]
pub struct Cli {
    /// Override config file location
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Increase verbosity (-v = DEBUG, -vv = TRACE)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Decrease verbosity (-q = WARN, -qq = ERROR, -qqq = OFF)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub quiet: u8,

    /// Preview actions without making changes
    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands for the janus CLI.
#[derive(Subcommand)]
pub enum Command {
    /// Initialize a dotfiles directory and config
    Init {
        /// Path for the dotfiles directory
        #[arg(long, default_value = "~/dotfiles")]
        dotfiles_dir: String,
    },

    /// Render templates into .generated/
    Generate {
        /// Files/globs to generate
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Copy generated files into .staged/
    Stage {
        /// Files/globs to stage
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Symlink staged files to their target locations
    Deploy {
        /// Files/globs to deploy
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Overwrite existing files without backup
        #[arg(long)]
        force: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Show diff between generated and staged files
    Diff {
        /// Files/globs to diff
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Remove generated files or clean up orphans
    Clean {
        /// Delete all generated files
        #[arg(long)]
        generated: bool,

        /// Remove orphan files from .generated/ and .staged/
        #[arg(long)]
        orphans: bool,
    },

    /// Import existing config files into management
    Import {
        /// Path to import (file or directory)
        path: String,

        /// Skip interactive prompts, import all files
        #[arg(long)]
        all: bool,

        /// Maximum directory traversal depth
        #[arg(long, default_value = "10")]
        max_depth: usize,
    },

    /// Run generate + stage + deploy in one shot
    Apply {
        /// Files/globs to apply
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Overwrite existing files without backup
        #[arg(long)]
        force: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Remove deployed symlinks
    Undeploy {
        /// Files/globs to undeploy
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Remove the symlink without leaving a copy of the file
        #[arg(long)]
        remove_file: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Fully reverse an import: undeploy, remove config entry, clean up source files
    Unimport {
        /// Source files to unimport (matched against src paths in config)
        files: Vec<String>,

        /// Remove the deployed symlink without leaving a copy of the file
        #[arg(long)]
        remove_file: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },

    /// Show pipeline status for managed files
    Status {
        /// Files/globs to check
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,

        /// Only show files with diffs
        #[arg(long)]
        only_diffs: bool,

        /// Only show deployed files
        #[arg(long)]
        deployed: bool,

        /// Only show undeployed files
        #[arg(long)]
        undeployed: bool,

        /// Filesets to operate on (comma-separated)
        #[arg(long, value_delimiter = ',')]
        filesets: Vec<String>,
    },
}
