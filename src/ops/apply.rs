//! Compound command: run generate -> stage -> deploy in one shot.
//!
//! Bails between steps if any step fails — won't deploy if generation or
//! staging produced errors.

use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::platform::{Fs, SecretEngine};

/// Run the full forward pipeline: generate, stage, then deploy.
///
/// If any step fails, subsequent steps are skipped. The `force` and `dry_run`
/// flags are passed through to each step.
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    force: bool,
    dry_run: bool,
    fs: &impl Fs,
    engine: &impl SecretEngine,
) -> Result<()> {
    info!("Running generate...");
    crate::ops::generate::run(config, files, dry_run, fs, engine)?;

    info!("Running stage...");
    crate::ops::stage::run(config, files, dry_run, fs)?;

    info!("Running deploy...");
    crate::ops::deploy::run(config, files, force, dry_run, fs)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeSecretEngine;
    use crate::state::State;
    use crate::test_helpers::*;
    use std::path::Path;

    #[test]
    fn full_pipeline() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/a.conf"), "content");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        );
        let engine = FakeSecretEngine::new();
        run(&config, None, false, false, &fs, &engine).unwrap();
        // Should have generated, staged, and deployed
        assert!(fs.exists(Path::new(&format!(
            "{DOTFILES}/.generated/a.conf"
        ))));
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.staged/a.conf"))));
        assert!(fs.is_symlink(Path::new("/home/test/.config/a.conf")));
        let state = State::load(Path::new(DOTFILES), &fs).unwrap();
        assert!(state.is_deployed("a.conf"));
    }

    #[test]
    fn generate_failure_stops() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        // Bad template syntax
        fs.add_file(format!("{DOTFILES}/bad.conf"), "{{ undefined_var }");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("bad.conf", Some("~/.config/bad.conf"))]),
        );
        let engine = FakeSecretEngine::new();
        let result = run(&config, None, false, false, &fs, &engine);
        assert!(result.is_err());
        // Should not have deployed
        assert!(!fs.exists(Path::new("/home/test/.config/bad.conf")));
    }

    #[test]
    fn dry_run() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/a.conf"), "content");
        // Pre-populate generated+staged so each dry_run step can verify what
        // *would* happen without the pipeline failing on missing intermediates.
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "content");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "content");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        );
        let engine = FakeSecretEngine::new();
        run(&config, None, false, true, &fs, &engine).unwrap();
        // No symlink should be created
        assert!(!fs.is_symlink(Path::new("/home/test/.config/a.conf")));
    }
}
