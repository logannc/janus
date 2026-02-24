//! Abstracted external dependencies for testability.
//!
//! Three traits cover all side effects: [`Fs`] for filesystem operations,
//! [`SecretEngine`] for resolving secrets from external managers, and
//! [`Prompter`] for interactive user prompts.
//!
//! Production code uses the real implementations ([`RealFs`], [`RealSecretEngine`],
//! [`RealPrompter`]). Tests substitute fakes via generics — no trait objects needed.

mod real_fs;
mod real_locker;
mod real_prompt;
mod real_secret;

pub use real_fs::RealFs;
pub use real_locker::RealLocker;
pub use real_prompt::RealPrompter;
pub use real_secret::RealSecretEngine;

#[cfg(test)]
mod fake_fs;
#[cfg(test)]
mod fake_locker;
#[cfg(test)]
mod fake_prompt;
#[cfg(test)]
mod fake_secret;

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::fake_fs::FakeEntry;
#[cfg(test)]
#[allow(unused_imports)]
pub use self::fake_fs::FakeFs;
#[cfg(test)]
#[allow(unused_imports)]
pub use self::fake_locker::FakeLocker;
#[cfg(test)]
#[allow(unused_imports)]
pub use self::fake_prompt::FakePrompter;
#[cfg(test)]
#[allow(unused_imports)]
pub use self::fake_secret::FakeSecretEngine;

use anyhow::Result;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

/// Options for directory traversal via [`Fs::walk_dir`].
#[derive(Debug, Clone, Default)]
pub struct WalkOptions {
    /// Maximum depth to recurse. `None` means unlimited.
    pub max_depth: Option<usize>,
    /// Minimum depth before yielding entries (0 = include the root itself).
    pub min_depth: usize,
    /// Whether to follow symbolic links.
    pub follow_links: bool,
    /// If true, yield directory contents before the directory itself.
    pub contents_first: bool,
}

/// A single entry returned by [`Fs::walk_dir`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Full path to this entry.
    pub path: PathBuf,
    /// Whether this is a regular file (follows symlinks per `WalkOptions::follow_links`).
    pub is_file: bool,
    /// Whether this is a directory (follows symlinks per `WalkOptions::follow_links`).
    pub is_dir: bool,
    /// Whether the path itself is a symbolic link (always raw lstat, regardless of follow_links).
    #[allow(dead_code)]
    pub is_symlink: bool,
}

/// Abstraction over all filesystem operations, directory traversal, and
/// system path queries (home dir, config dir).
///
/// Every method that touches the filesystem goes through this trait.
pub trait Fs {
    // -- Reading --

    /// Read the entire contents of a file as a UTF-8 string.
    fn read_to_string(&self, path: &Path) -> Result<String>;

    /// Read the entire contents of a file as raw bytes.
    fn read(&self, path: &Path) -> Result<Vec<u8>>;

    // -- Writing --

    /// Write `contents` to a file, creating it or truncating if it exists.
    fn write(&self, path: &Path, contents: &[u8]) -> Result<()>;

    /// Copy a file from `from` to `to`, overwriting `to` if it exists.
    fn copy(&self, from: &Path, to: &Path) -> Result<()>;

    // -- File/directory removal --

    /// Remove a single file (or symlink).
    fn remove_file(&self, path: &Path) -> Result<()>;

    /// Remove an empty directory.
    fn remove_dir(&self, path: &Path) -> Result<()>;

    /// Atomically rename `from` to `to`.
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;

    // -- Directory creation --

    /// Create a directory and all missing parents.
    fn create_dir_all(&self, path: &Path) -> Result<()>;

    // -- Permissions --

    /// Get the Unix file mode (permission bits) of a file.
    fn file_mode(&self, path: &Path) -> Result<u32>;

    /// Set the Unix file mode (permission bits) of a file.
    fn set_file_mode(&self, path: &Path, mode: u32) -> Result<()>;

    // -- Symlinks --

    /// Create a symbolic link at `link` pointing to `original`.
    fn symlink(&self, original: &Path, link: &Path) -> Result<()>;

    /// Read the target of a symbolic link.
    fn read_link(&self, path: &Path) -> Result<PathBuf>;

    // -- Path queries --

    /// Check if a path exists (follows symlinks; broken symlinks return false).
    fn exists(&self, path: &Path) -> bool;

    /// Check if a path is a symbolic link (raw lstat, does not follow).
    fn is_symlink(&self, path: &Path) -> bool;

    /// Check if a path is a regular file (follows symlinks).
    #[allow(dead_code)]
    fn is_file(&self, path: &Path) -> bool;

    /// Check if a path is a directory (follows symlinks).
    fn is_dir(&self, path: &Path) -> bool;

    // -- Directory traversal --

    /// Walk a directory tree, returning entries matching the given options.
    fn walk_dir(&self, path: &Path, opts: &WalkOptions) -> Result<Vec<DirEntry>>;

    // -- System paths --

    /// Return the user's home directory, if it can be determined.
    fn home_dir(&self) -> Option<PathBuf>;

    /// Return the user's config directory (e.g. `~/.config`), if it can be determined.
    fn config_dir(&self) -> Option<PathBuf>;
}

// ---------------------------------------------------------------------------
// Secret engine
// ---------------------------------------------------------------------------

/// Abstraction over external secret resolution (e.g. 1Password CLI).
///
/// Given an engine name and a reference string, resolves the secret value.
pub trait SecretEngine {
    /// Resolve a secret by engine name (e.g. `"1password"`) and reference
    /// (e.g. `"op://Vault/Item/Field"`).
    ///
    /// Returns the secret value as a string.
    fn resolve(&self, engine: &str, reference: &str) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Prompter
// ---------------------------------------------------------------------------

/// Abstraction over interactive user prompts.
///
/// In production, delegates to `dialoguer`. In tests, returns predetermined answers.
pub trait Prompter {
    /// Present a selection prompt and return the index of the chosen item.
    ///
    /// `prompt` is the question text, `items` are the choices, and `default`
    /// is the pre-selected index.
    fn select(&self, prompt: &str, items: &[&str], default: usize) -> Result<usize>;
}

// ---------------------------------------------------------------------------
// Process lock
// ---------------------------------------------------------------------------

/// Abstraction over process-level file locking.
///
/// In production, delegates to `fslock`. In tests, uses in-memory state.
pub trait Locker {
    /// Attempt to acquire the lock without blocking.
    /// Returns `true` if the lock was acquired, `false` if held by another process.
    fn try_lock(&mut self) -> Result<bool>;

    /// Release the lock.
    #[allow(dead_code)]
    fn unlock(&mut self) -> Result<()>;

    /// Read the PID of the process currently holding the lock, if available.
    fn read_lock_owner(&self) -> Result<Option<u32>>;

    /// Path to the lock file.
    fn lock_path(&self) -> &Path;
}
