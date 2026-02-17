//! Initialize a new dotfiles directory and janus config.
//!
//! Creates the directory structure (`dotfiles_dir/`, `.generated/`, `.staged/`),
//! a default `vars.toml`, an empty `.janus_state.toml`, and a config source at
//! `{dotfiles_dir}/janus/config.toml`. The config is then deployed through the
//! full pipeline (generate → stage → deploy) so janus manages its own config.

use anyhow::{Context, Result};
use tracing::info;

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::platform::{Fs, SecretEngine};

/// Scaffold the dotfiles directory, state file, and config file.
///
/// Creates the directory structure, writes the config source inside the
/// dotfiles directory (with a self-referencing `[[files]]` entry), then
/// runs the full pipeline (`apply`) to deploy it as a symlink.
pub fn run(dotfiles_dir: &str, dry_run: bool, fs: &impl Fs, engine: &impl SecretEngine) -> Result<()> {
    let dotfiles_path = expand_tilde(dotfiles_dir, fs);

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
            "[dry-run] Would create config source: {}",
            dotfiles_path.join("janus/config.toml").display()
        );
        info!("[dry-run] Would deploy config through pipeline");
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

    // Create config source inside dotfiles dir (self-managed)
    let config_src = dotfiles_path.join("janus/config.toml");
    if let Some(parent) = config_src.parent() {
        fs.create_dir_all(parent)
            .context("Failed to create janus config source directory")?;
    }
    if !fs.exists(&config_src) {
        let default_config = format!(
            "dotfiles_dir = \"{dotfiles_dir}\"\nvars = [\"vars.toml\"]\n\n[[files]]\nsrc = \"janus/config.toml\"\ntemplate = false\n"
        );
        fs.write(&config_src, default_config.as_bytes())
            .with_context(|| {
                format!(
                    "Failed to create config source: {}",
                    config_src.display()
                )
            })?;
        info!("Created config source at {}", config_src.display());
    }

    // Load config from source and deploy through the pipeline
    let config = Config::load(&config_src, fs)?;
    info!("Deploying config through pipeline...");
    crate::ops::apply::run(&config, None, false, dry_run, fs, engine)?;

    info!("Initialization complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{FakeFs, FakeSecretEngine};
    use crate::state::State;
    use std::path::Path;

    #[test]
    fn creates_all_dirs() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        assert!(fs.is_dir(Path::new("/home/test/dotfiles")));
        assert!(fs.is_dir(Path::new("/home/test/dotfiles/.generated")));
        assert!(fs.is_dir(Path::new("/home/test/dotfiles/.staged")));
        assert!(fs.is_dir(Path::new("/home/test/dotfiles/janus")));
    }

    #[test]
    fn creates_default_files() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        assert!(fs.exists(Path::new("/home/test/dotfiles/vars.toml")));
        assert!(fs.exists(Path::new("/home/test/dotfiles/.janus_state.toml")));
        // Config source exists in dotfiles dir
        assert!(fs.exists(Path::new("/home/test/dotfiles/janus/config.toml")));
        // Pipeline copies exist
        assert!(fs.exists(Path::new(
            "/home/test/dotfiles/.generated/janus/config.toml"
        )));
        assert!(fs.exists(Path::new(
            "/home/test/dotfiles/.staged/janus/config.toml"
        )));
        // Deployed symlink exists at target
        assert!(fs.exists(Path::new("/home/test/.config/janus/config.toml")));
    }

    #[test]
    fn idempotent() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        let source_content = fs
            .read_to_string(Path::new("/home/test/dotfiles/janus/config.toml"))
            .unwrap();
        // Run again
        run("~/dotfiles", false, &fs, &engine).unwrap();
        // Source should be unchanged
        let source_content2 = fs
            .read_to_string(Path::new("/home/test/dotfiles/janus/config.toml"))
            .unwrap();
        assert_eq!(source_content, source_content2);
        // Symlink should still be valid
        assert!(fs.is_symlink(Path::new("/home/test/.config/janus/config.toml")));
    }

    #[test]
    fn dry_run() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", true, &fs, &engine).unwrap();
        assert!(!fs.exists(Path::new("/home/test/dotfiles")));
        assert!(!fs.exists(Path::new("/home/test/dotfiles/janus/config.toml")));
        assert!(!fs.exists(Path::new("/home/test/.config/janus/config.toml")));
    }

    #[test]
    fn config_deployed_as_symlink() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        let target = Path::new("/home/test/.config/janus/config.toml");
        assert!(fs.is_symlink(target));
        let link_dest = fs.read_link(target).unwrap();
        assert_eq!(
            link_dest,
            std::path::PathBuf::from("/home/test/dotfiles/.staged/janus/config.toml")
        );
    }

    #[test]
    fn config_content_self_referencing() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        let content = fs
            .read_to_string(Path::new("/home/test/dotfiles/janus/config.toml"))
            .unwrap();
        assert!(content.contains("src = \"janus/config.toml\""));
        assert!(content.contains("template = false"));
    }

    #[test]
    fn state_records_deployment() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        let state = State::load(Path::new("/home/test/dotfiles"), &fs).unwrap();
        assert!(state.is_deployed("janus/config.toml"));
    }

    #[test]
    fn config_loadable_via_symlink() {
        let fs = FakeFs::new("/home/test");
        let engine = FakeSecretEngine::new();
        run("~/dotfiles", false, &fs, &engine).unwrap();
        // Load config via the deployed symlink path (as janus would normally do)
        let config = Config::load(Path::new("/home/test/.config/janus/config.toml"), &fs).unwrap();
        assert_eq!(config.dotfiles_dir, "~/dotfiles");
        assert_eq!(config.files.len(), 1);
        assert_eq!(config.files[0].src, "janus/config.toml");
    }
}
