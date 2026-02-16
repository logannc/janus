//! Shared test helpers for setting up in-memory test environments.

use crate::config::Config;
use crate::platform::FakeFs;
use std::path::Path;

pub const HOME: &str = "/home/test";
pub const DOTFILES: &str = "/home/test/dotfiles";
pub const CONFIG_PATH: &str = "/home/test/.config/janus/config.toml";

/// Create a `FakeFs` seeded with the standard dotfiles directory structure
/// and an empty state file.
pub fn setup_fs() -> FakeFs {
    let fs = FakeFs::new(HOME);
    fs.add_dir(DOTFILES);
    fs.add_dir(format!("{DOTFILES}/.generated"));
    fs.add_dir(format!("{DOTFILES}/.staged"));
    fs.add_file(format!("{DOTFILES}/.janus_state.toml"), "");
    fs
}

/// Build a minimal config TOML string from file entries.
///
/// Each entry is `(src, optional_target)`. If `target` is `None`, the
/// `target` key is omitted (defaults to `~/.config/{src}`).
pub fn make_config_toml(files: &[(&str, Option<&str>)]) -> String {
    let mut toml = format!("dotfiles_dir = \"{DOTFILES}\"\nvars = [\"vars.toml\"]\n");
    for (src, target) in files {
        toml.push_str("\n[[files]]\n");
        toml.push_str(&format!("src = \"{src}\"\n"));
        if let Some(t) = target {
            toml.push_str(&format!("target = \"{t}\"\n"));
        }
    }
    toml
}

/// Write a config TOML string to the standard config path and load it.
pub fn write_and_load_config(fs: &FakeFs, toml_str: &str) -> Config {
    fs.add_file(CONFIG_PATH, toml_str);
    Config::load(Path::new(CONFIG_PATH), fs).expect("test config should parse")
}

/// Create a source file and its corresponding .generated and .staged copies.
pub fn setup_pipeline_file(fs: &FakeFs, src: &str, content: &str) {
    fs.add_file(format!("{DOTFILES}/{src}"), content);
    fs.add_file(format!("{DOTFILES}/.generated/{src}"), content);
    fs.add_file(format!("{DOTFILES}/.staged/{src}"), content);
}
