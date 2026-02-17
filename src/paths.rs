//! Path utilities for tilde expansion and contraction.
//!
//! Use [`expand_tilde`] before any filesystem operation on user-provided paths.
//! Use [`collapse_tilde`] when displaying paths back to the user.

use std::path::{Path, PathBuf};

use crate::platform::Fs;

/// Expand `~` or `~/...` at the start of a path to the user's home directory.
///
/// Returns the path unchanged if it doesn't start with `~` or if the home
/// directory cannot be determined.
pub fn expand_tilde(path: &str, fs: &impl Fs) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = fs.home_dir() {
            return home.join(rest);
        }
    } else if path == "~"
        && let Some(home) = fs.home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

/// Collapse the user's home directory prefix back to `~/...` for display.
///
/// Returns the full path string if the home directory cannot be determined
/// or the path is not under it.
pub fn collapse_tilde(path: &Path, fs: &impl Fs) -> String {
    if let Some(home) = fs.home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeFs;
    use std::path::Path;

    #[test]
    fn expand_tilde_home_prefix() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(expand_tilde("~/foo", &fs), PathBuf::from("/home/test/foo"));
    }

    #[test]
    fn expand_tilde_bare_tilde() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(expand_tilde("~", &fs), PathBuf::from("/home/test"));
    }

    #[test]
    fn expand_tilde_no_tilde() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(expand_tilde("/etc/foo", &fs), PathBuf::from("/etc/foo"));
        assert_eq!(
            expand_tilde("relative/path", &fs),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn expand_tilde_tilde_not_at_start() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(expand_tilde("foo/~/bar", &fs), PathBuf::from("foo/~/bar"));
    }

    #[test]
    fn collapse_tilde_under_home() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(collapse_tilde(Path::new("/home/test/foo"), &fs), "~/foo");
    }

    #[test]
    fn collapse_tilde_not_under_home() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(collapse_tilde(Path::new("/etc/foo"), &fs), "/etc/foo");
    }

    #[test]
    fn collapse_tilde_exact_home() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(collapse_tilde(Path::new("/home/test"), &fs), "~/");
    }

    #[test]
    fn roundtrip() {
        let fs = FakeFs::new("/home/test");
        let original = "~/some/path";
        let expanded = expand_tilde(original, &fs);
        let collapsed = collapse_tilde(&expanded, &fs);
        assert_eq!(collapsed, "~/some/path");
    }
}
