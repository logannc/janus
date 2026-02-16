//! Initialize a new dotfiles directory and janus config.
//!
//! Creates the directory structure (`dotfiles_dir/`, `.generated/`, `.staged/`),
//! a default `vars.toml`, an empty `.janus_state.toml`, and a config file at
//! the default XDG config path.

use anyhow::{Context, Result};
use tracing::info;

use crate::paths::expand_tilde;
use crate::platform::Fs;

/// Scaffold the dotfiles directory, state file, and config file.
///
/// Skips creating any file or directory that already exists.
pub fn run(dotfiles_dir: &str, dry_run: bool, fs: &impl Fs) -> Result<()> {
    let dotfiles_path = expand_tilde(dotfiles_dir, fs);
    let config_path = crate::config::Config::default_path(fs);

    info!(
        "Initializing dotfiles directory at {}",
        dotfiles_path.display()
    );

    if dry_run {
        info!(
            "[dry-run] Would create directory: {}",
            dotfiles_path.display()
        );
        info!(
            "[dry-run] Would create directory: {}",
            dotfiles_path.join(".generated").display()
        );
        info!(
            "[dry-run] Would create directory: {}",
            dotfiles_path.join(".staged").display()
        );
        info!(
            "[dry-run] Would create state file: {}",
            dotfiles_path.join(".janus_state.toml").display()
        );
        info!(
            "[dry-run] Would create config file: {}",
            config_path.display()
        );
        return Ok(());
    }

    // Create directories
    fs.create_dir_all(&dotfiles_path).with_context(|| {
        format!(
            "Failed to create dotfiles directory: {}",
            dotfiles_path.display()
        )
    })?;
    fs.create_dir_all(&dotfiles_path.join(".generated"))
        .context("Failed to create .generated directory")?;
    fs.create_dir_all(&dotfiles_path.join(".staged"))
        .context("Failed to create .staged directory")?;

    // Create default vars.toml
    let vars_path = dotfiles_path.join("vars.toml");
    if !fs.exists(&vars_path) {
        fs.write(&vars_path, b"# Template variables\n")
            .context("Failed to create vars.toml")?;
        info!("Created {}", vars_path.display());
    }

    // Create state file
    let state_path = dotfiles_path.join(".janus_state.toml");
    if !fs.exists(&state_path) {
        fs.write(&state_path, b"")
            .context("Failed to create state file")?;
        info!("Created {}", state_path.display());
    }

    // Create default config
    if let Some(parent) = config_path.parent() {
        fs.create_dir_all(parent)
            .context("Failed to create config directory")?;
    }
    if !fs.exists(&config_path) {
        let default_config = format!("dotfiles_dir = \"{dotfiles_dir}\"\nvars = [\"vars.toml\"]\n");
        fs.write(&config_path, default_config.as_bytes())
            .with_context(|| format!("Failed to create config file: {}", config_path.display()))?;
        info!("Created config at {}", config_path.display());
    }

    info!("Initialization complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeFs;
    use std::path::Path;

    #[test]
    fn creates_all_dirs() {
        let fs = FakeFs::new("/home/test");
        run("~/dotfiles", false, &fs).unwrap();
        assert!(fs.is_dir(Path::new("/home/test/dotfiles")));
        assert!(fs.is_dir(Path::new("/home/test/dotfiles/.generated")));
        assert!(fs.is_dir(Path::new("/home/test/dotfiles/.staged")));
    }

    #[test]
    fn creates_default_files() {
        let fs = FakeFs::new("/home/test");
        run("~/dotfiles", false, &fs).unwrap();
        assert!(fs.exists(Path::new("/home/test/dotfiles/vars.toml")));
        assert!(fs.exists(Path::new("/home/test/dotfiles/.janus_state.toml")));
        assert!(fs.exists(Path::new("/home/test/.config/janus/config.toml")));
    }

    #[test]
    fn idempotent() {
        let fs = FakeFs::new("/home/test");
        run("~/dotfiles", false, &fs).unwrap();
        // Read the config to check its content
        let content1 = fs
            .read_to_string(Path::new("/home/test/.config/janus/config.toml"))
            .unwrap();
        // Run again
        run("~/dotfiles", false, &fs).unwrap();
        let content2 = fs
            .read_to_string(Path::new("/home/test/.config/janus/config.toml"))
            .unwrap();
        // Content should be identical (not overwritten)
        assert_eq!(content1, content2);
    }

    #[test]
    fn dry_run() {
        let fs = FakeFs::new("/home/test");
        run("~/dotfiles", true, &fs).unwrap();
        assert!(!fs.exists(Path::new("/home/test/dotfiles")));
        assert!(!fs.exists(Path::new("/home/test/.config/janus/config.toml")));
    }
}
