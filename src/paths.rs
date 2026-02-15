//! Path utilities for tilde expansion and contraction.
//!
//! Use [`expand_tilde`] before any filesystem operation on user-provided paths.
//! Use [`collapse_tilde`] when displaying paths back to the user.

use std::path::{Path, PathBuf};

/// Expand `~` or `~/...` at the start of a path to the user's home directory.
///
/// Returns the path unchanged if it doesn't start with `~` or if the home
/// directory cannot be determined.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir() {
            return home;
        }
    PathBuf::from(path)
}

/// Collapse the user's home directory prefix back to `~/...` for display.
///
/// Returns the full path string if the home directory cannot be determined
/// or the path is not under it.
pub fn collapse_tilde(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    path.display().to_string()
}
