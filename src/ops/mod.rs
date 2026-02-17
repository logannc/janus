//! Operations that implement janus commands.
//!
//! Each submodule corresponds to a CLI subcommand and exposes a `run()` function.
//! The forward pipeline is `generate` → `stage` → `deploy` (or `apply` as a
//! compound shortcut). Reverse operations are `undeploy`, `unimport`, and `clean`.

pub mod apply;
pub mod clean;
pub mod deploy;
pub mod diff;
pub mod generate;
pub mod import;
pub mod init;
pub mod stage;
pub mod status;
pub mod sync;
pub mod undeploy;
pub mod unimport;

use std::path::Path;

use crate::platform::Fs;

/// Check if `target` is a symlink pointing to `expected_staged`.
pub(crate) fn is_janus_symlink(target: &Path, expected_staged: &Path, fs: &impl Fs) -> bool {
    if !fs.is_symlink(target) {
        return false;
    }
    match fs.read_link(target) {
        Ok(link_dest) => link_dest == expected_staged,
        Err(_) => false,
    }
}
