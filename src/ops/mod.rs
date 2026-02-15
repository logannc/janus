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
pub mod undeploy;
pub mod unimport;
