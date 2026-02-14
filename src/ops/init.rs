use anyhow::{Context, Result};
use tracing::info;

use crate::paths::expand_tilde;

pub fn run(dotfiles_dir: &str, dry_run: bool) -> Result<()> {
    let dotfiles_path = expand_tilde(dotfiles_dir);
    let config_path = crate::config::Config::default_path();

    info!("Initializing dotfiles directory at {}", dotfiles_path.display());

    if dry_run {
        info!("[dry-run] Would create directory: {}", dotfiles_path.display());
        info!("[dry-run] Would create directory: {}", dotfiles_path.join(".generated").display());
        info!("[dry-run] Would create directory: {}", dotfiles_path.join(".staged").display());
        info!("[dry-run] Would create state file: {}", dotfiles_path.join(".janus_state.toml").display());
        info!("[dry-run] Would create config file: {}", config_path.display());
        return Ok(());
    }

    // Create directories
    std::fs::create_dir_all(&dotfiles_path)
        .with_context(|| format!("Failed to create dotfiles directory: {}", dotfiles_path.display()))?;
    std::fs::create_dir_all(dotfiles_path.join(".generated"))
        .context("Failed to create .generated directory")?;
    std::fs::create_dir_all(dotfiles_path.join(".staged"))
        .context("Failed to create .staged directory")?;

    // Create default vars.toml
    let vars_path = dotfiles_path.join("vars.toml");
    if !vars_path.exists() {
        std::fs::write(&vars_path, "# Template variables\n")
            .context("Failed to create vars.toml")?;
        info!("Created {}", vars_path.display());
    }

    // Create state file
    let state_path = dotfiles_path.join(".janus_state.toml");
    if !state_path.exists() {
        std::fs::write(&state_path, "")
            .context("Failed to create state file")?;
        info!("Created {}", state_path.display());
    }

    // Create default config
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .context("Failed to create config directory")?;
    }
    if !config_path.exists() {
        let default_config = format!(
            "dotfiles_dir = \"{dotfiles_dir}\"\nvars = [\"vars.toml\"]\n"
        );
        std::fs::write(&config_path, default_config)
            .with_context(|| format!("Failed to create config file: {}", config_path.display()))?;
        info!("Created config at {}", config_path.display());
    }

    info!("Initialization complete");
    Ok(())
}
