use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    },

    /// Copy generated files into .staged/
    Stage {
        /// Files/globs to stage
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,
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
    },

    /// Show diff between generated and staged files
    Diff {
        /// Files/globs to diff
        files: Vec<String>,

        /// Process all configured files
        #[arg(long)]
        all: bool,
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
}
