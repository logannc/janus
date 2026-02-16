//! In-memory filesystem fake for testing.
//!
//! Stores files, directories, and symlinks in a `HashMap` with interior
//! mutability via `RefCell`. Supports all `Fs` trait operations including
//! symlink resolution, permission tracking, and directory walking.
//!
//! Non-trait setup methods (`add_file`, `add_dir`, `add_symlink`) auto-create
//! parent directories for convenience in test setup.

use anyhow::{bail, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{DirEntry, Fs, WalkOptions};

/// A single entry in the fake filesystem.
#[derive(Clone, Debug)]
pub(crate) enum FakeEntry {
    File { content: Vec<u8>, mode: u32 },
    Symlink { target: PathBuf },
    Dir,
}

/// In-memory filesystem for testing — no real I/O.
///
/// Uses `RefCell` for interior mutability so trait methods taking `&self`
/// can still mutate the internal state.
pub struct FakeFs {
    entries: RefCell<HashMap<PathBuf, FakeEntry>>,
    home: PathBuf,
    config_dir: PathBuf,
    fail_writes: RefCell<bool>,
}

impl FakeFs {
    /// Create a new fake filesystem with the given home directory.
    ///
    /// Automatically creates the home directory and `~/.config`.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let config_dir = home.join(".config");
        let mut entries = HashMap::new();
        // Seed root, home, and config dirs
        entries.insert(PathBuf::from("/"), FakeEntry::Dir);
        entries.insert(home.clone(), FakeEntry::Dir);
        entries.insert(config_dir.clone(), FakeEntry::Dir);
        Self {
            entries: RefCell::new(entries),
            home,
            config_dir,
            fail_writes: RefCell::new(false),
        }
    }

    /// Toggle write failures. When enabled, all `Fs::write` calls bail.
    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.borrow_mut() = fail;
    }

    // -- Setup helpers (not part of the Fs trait) --

    /// Add a file with content and default permissions (0o644).
    /// Auto-creates parent directories. Returns the previous entry if one existed.
    pub fn add_file(
        &self,
        path: impl Into<PathBuf>,
        content: impl Into<Vec<u8>>,
    ) -> Option<FakeEntry> {
        let path = path.into();
        self.ensure_parents(&path);
        self.entries.borrow_mut().insert(
            path,
            FakeEntry::File {
                content: content.into(),
                mode: 0o644,
            },
        )
    }

    /// Add a file with content and explicit permissions.
    /// Auto-creates parent directories. Returns the previous entry if one existed.
    pub fn add_file_with_mode(
        &self,
        path: impl Into<PathBuf>,
        content: impl Into<Vec<u8>>,
        mode: u32,
    ) -> Option<FakeEntry> {
        let path = path.into();
        self.ensure_parents(&path);
        self.entries.borrow_mut().insert(
            path,
            FakeEntry::File {
                content: content.into(),
                mode,
            },
        )
    }

    /// Add a directory entry. Auto-creates parent directories.
    /// Returns the previous entry if one existed (no-op if already a dir).
    pub fn add_dir(&self, path: impl Into<PathBuf>) -> Option<FakeEntry> {
        let path = path.into();
        self.ensure_parents(&path);
        let mut entries = self.entries.borrow_mut();
        if entries.contains_key(&path) {
            entries.get(&path).cloned()
        } else {
            entries.insert(path, FakeEntry::Dir);
            None
        }
    }

    /// Add a symbolic link. Auto-creates parent directories for the link path.
    /// Returns the previous entry if one existed at the link path.
    pub fn add_symlink(
        &self,
        link: impl Into<PathBuf>,
        target: impl Into<PathBuf>,
    ) -> Option<FakeEntry> {
        let link = link.into();
        self.ensure_parents(&link);
        self.entries.borrow_mut().insert(
            link,
            FakeEntry::Symlink {
                target: target.into(),
            },
        )
    }

    /// Ensure all parent directories of `path` exist.
    fn ensure_parents(&self, path: &Path) {
        let mut entries = self.entries.borrow_mut();
        if let Some(parent) = path.parent() {
            let mut current = PathBuf::new();
            for component in parent.components() {
                current.push(component);
                entries.entry(current.clone()).or_insert(FakeEntry::Dir);
            }
        }
    }

    /// Resolve a path through symlinks (up to 32 hops to avoid infinite loops).
    fn resolve_path(&self, path: &Path) -> PathBuf {
        let entries = self.entries.borrow();
        let mut current = path.to_path_buf();
        for _ in 0..32 {
            match entries.get(&current) {
                Some(FakeEntry::Symlink { target }) => current = target.clone(),
                _ => break,
            }
        }
        current
    }
}

impl Fs for FakeFs {
    fn read_to_string(&self, path: &Path) -> Result<String> {
        let resolved = self.resolve_path(path);
        let entries = self.entries.borrow();
        match entries.get(&resolved) {
            Some(FakeEntry::File { content, .. }) => Ok(String::from_utf8(content.clone())?),
            Some(_) => bail!("not a file: {}", path.display()),
            None => bail!("file not found: {}", path.display()),
        }
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        let resolved = self.resolve_path(path);
        let entries = self.entries.borrow();
        match entries.get(&resolved) {
            Some(FakeEntry::File { content, .. }) => Ok(content.clone()),
            Some(_) => bail!("not a file: {}", path.display()),
            None => bail!("file not found: {}", path.display()),
        }
    }

    fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        if *self.fail_writes.borrow() {
            bail!("simulated write failure: {}", path.display());
        }
        let resolved = self.resolve_path(path);
        // Preserve existing mode if file already exists
        let mode = {
            let entries = self.entries.borrow();
            match entries.get(&resolved) {
                Some(FakeEntry::File { mode, .. }) => *mode,
                _ => 0o644,
            }
        };
        self.entries.borrow_mut().insert(
            resolved,
            FakeEntry::File {
                content: contents.to_vec(),
                mode,
            },
        );
        Ok(())
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        let resolved = self.resolve_path(from);
        let entry = { self.entries.borrow().get(&resolved).cloned() };
        match entry {
            Some(FakeEntry::File { content, mode }) => {
                self.entries
                    .borrow_mut()
                    .insert(to.to_path_buf(), FakeEntry::File { content, mode });
                Ok(())
            }
            _ => bail!("cannot copy non-file: {}", from.display()),
        }
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        let mut entries = self.entries.borrow_mut();
        match entries.get(path) {
            Some(FakeEntry::File { .. } | FakeEntry::Symlink { .. }) => {
                entries.remove(path);
                Ok(())
            }
            Some(FakeEntry::Dir) => bail!("is a directory: {}", path.display()),
            None => bail!("file not found: {}", path.display()),
        }
    }

    fn remove_dir(&self, path: &Path) -> Result<()> {
        let mut entries = self.entries.borrow_mut();
        match entries.get(path) {
            Some(FakeEntry::Dir) => {}
            _ => bail!("not a directory: {}", path.display()),
        }
        // Check if directory has children
        let has_children = entries
            .keys()
            .any(|k| k != path && k.starts_with(path) && k.parent() == Some(path));
        if has_children {
            bail!("directory not empty: {}", path.display());
        }
        entries.remove(path);
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let mut entries = self.entries.borrow_mut();
        match entries.remove(from) {
            Some(entry) => {
                entries.insert(to.to_path_buf(), entry);
                Ok(())
            }
            None => bail!("not found: {}", from.display()),
        }
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let mut entries = self.entries.borrow_mut();
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component);
            entries.entry(current.clone()).or_insert(FakeEntry::Dir);
        }
        Ok(())
    }

    fn file_mode(&self, path: &Path) -> Result<u32> {
        let resolved = self.resolve_path(path);
        let entries = self.entries.borrow();
        match entries.get(&resolved) {
            Some(FakeEntry::File { mode, .. }) => Ok(*mode),
            Some(FakeEntry::Dir) => Ok(0o755),
            _ => bail!("not found: {}", path.display()),
        }
    }

    fn set_file_mode(&self, path: &Path, mode: u32) -> Result<()> {
        let resolved = self.resolve_path(path);
        let mut entries = self.entries.borrow_mut();
        match entries.get_mut(&resolved) {
            Some(FakeEntry::File { mode: m, .. }) => {
                *m = mode;
                Ok(())
            }
            _ => bail!("not a file: {}", path.display()),
        }
    }

    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        self.entries.borrow_mut().insert(
            link.to_path_buf(),
            FakeEntry::Symlink {
                target: original.to_path_buf(),
            },
        );
        Ok(())
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        let entries = self.entries.borrow();
        match entries.get(path) {
            Some(FakeEntry::Symlink { target }) => Ok(target.clone()),
            _ => bail!("not a symlink: {}", path.display()),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        let resolved = self.resolve_path(path);
        self.entries.borrow().contains_key(&resolved)
    }

    fn is_symlink(&self, path: &Path) -> bool {
        matches!(
            self.entries.borrow().get(path),
            Some(FakeEntry::Symlink { .. })
        )
    }

    fn is_file(&self, path: &Path) -> bool {
        let resolved = self.resolve_path(path);
        matches!(
            self.entries.borrow().get(&resolved),
            Some(FakeEntry::File { .. })
        )
    }

    fn is_dir(&self, path: &Path) -> bool {
        let resolved = self.resolve_path(path);
        matches!(
            self.entries.borrow().get(&resolved),
            Some(FakeEntry::Dir)
        )
    }

    fn walk_dir(&self, path: &Path, opts: &WalkOptions) -> Result<Vec<DirEntry>> {
        let entries = self.entries.borrow();
        let root_components = path.components().count();

        let mut results: Vec<DirEntry> = entries
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .filter_map(|(p, entry)| {
                let depth = p.components().count().saturating_sub(root_components);

                if depth < opts.min_depth {
                    return None;
                }
                if let Some(max) = opts.max_depth {
                    if depth > max {
                        return None;
                    }
                }

                let (is_file, is_dir, is_symlink) = match entry {
                    FakeEntry::File { .. } => (true, false, false),
                    FakeEntry::Dir => (false, true, false),
                    FakeEntry::Symlink { target } => {
                        if opts.follow_links {
                            // Resolve symlink target type
                            match entries.get(target) {
                                Some(FakeEntry::File { .. }) => (true, false, true),
                                Some(FakeEntry::Dir) => (false, true, true),
                                _ => (false, false, true),
                            }
                        } else {
                            (false, false, true)
                        }
                    }
                };

                Some(DirEntry {
                    path: p.clone(),
                    is_file,
                    is_dir,
                    is_symlink,
                })
            })
            .collect();

        if opts.contents_first {
            // Deeper entries first, then alphabetical within same depth
            results.sort_by(|a, b| {
                let a_depth = a.path.components().count();
                let b_depth = b.path.components().count();
                b_depth.cmp(&a_depth).then(a.path.cmp(&b.path))
            });
        } else {
            results.sort_by(|a, b| a.path.cmp(&b.path));
        }

        Ok(results)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        Some(self.home.clone())
    }

    fn config_dir(&self) -> Option<PathBuf> {
        Some(self.config_dir.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_roundtrip() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/tmp/hello.txt", "hello world");
        assert_eq!(
            fs.read_to_string(Path::new("/tmp/hello.txt")).unwrap(),
            "hello world"
        );
        assert!(fs.exists(Path::new("/tmp/hello.txt")));
        assert!(fs.is_file(Path::new("/tmp/hello.txt")));
        assert!(!fs.is_dir(Path::new("/tmp/hello.txt")));
    }

    #[test]
    fn test_write_and_read() {
        let fs = FakeFs::new("/home/test");
        fs.create_dir_all(Path::new("/tmp")).unwrap();
        fs.write(Path::new("/tmp/out.txt"), b"written").unwrap();
        assert_eq!(
            fs.read_to_string(Path::new("/tmp/out.txt")).unwrap(),
            "written"
        );
    }

    #[test]
    fn test_symlink_resolution() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/real/file.txt", "content");
        fs.add_symlink("/link", "/real/file.txt");

        assert!(fs.is_symlink(Path::new("/link")));
        assert!(!fs.is_symlink(Path::new("/real/file.txt")));
        assert!(fs.exists(Path::new("/link")));
        assert_eq!(
            fs.read_to_string(Path::new("/link")).unwrap(),
            "content"
        );
        assert_eq!(
            fs.read_link(Path::new("/link")).unwrap(),
            PathBuf::from("/real/file.txt")
        );
    }

    #[test]
    fn test_broken_symlink() {
        let fs = FakeFs::new("/home/test");
        fs.add_symlink("/broken", "/nonexistent");

        assert!(fs.is_symlink(Path::new("/broken")));
        assert!(!fs.exists(Path::new("/broken"))); // broken symlink
    }

    #[test]
    fn test_file_permissions() {
        let fs = FakeFs::new("/home/test");
        fs.add_file_with_mode("/tmp/script.sh", "#!/bin/bash", 0o755);

        assert_eq!(fs.file_mode(Path::new("/tmp/script.sh")).unwrap(), 0o755);
        fs.set_file_mode(Path::new("/tmp/script.sh"), 0o644).unwrap();
        assert_eq!(fs.file_mode(Path::new("/tmp/script.sh")).unwrap(), 0o644);
    }

    #[test]
    fn test_remove_file() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/tmp/gone.txt", "bye");

        assert!(fs.exists(Path::new("/tmp/gone.txt")));
        fs.remove_file(Path::new("/tmp/gone.txt")).unwrap();
        assert!(!fs.exists(Path::new("/tmp/gone.txt")));
    }

    #[test]
    fn test_remove_dir_empty() {
        let fs = FakeFs::new("/home/test");
        fs.add_dir("/tmp/empty");

        fs.remove_dir(Path::new("/tmp/empty")).unwrap();
        assert!(!fs.exists(Path::new("/tmp/empty")));
    }

    #[test]
    fn test_remove_dir_nonempty_fails() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/tmp/dir/file.txt", "x");

        assert!(fs.remove_dir(Path::new("/tmp/dir")).is_err());
    }

    #[test]
    fn test_copy() {
        let fs = FakeFs::new("/home/test");
        fs.add_file_with_mode("/src/file.txt", "data", 0o755);
        fs.add_dir("/dst");

        fs.copy(Path::new("/src/file.txt"), Path::new("/dst/file.txt"))
            .unwrap();
        assert_eq!(
            fs.read_to_string(Path::new("/dst/file.txt")).unwrap(),
            "data"
        );
        assert_eq!(fs.file_mode(Path::new("/dst/file.txt")).unwrap(), 0o755);
    }

    #[test]
    fn test_rename() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/tmp/old.txt", "data");

        fs.rename(Path::new("/tmp/old.txt"), Path::new("/tmp/new.txt"))
            .unwrap();
        assert!(!fs.exists(Path::new("/tmp/old.txt")));
        assert_eq!(
            fs.read_to_string(Path::new("/tmp/new.txt")).unwrap(),
            "data"
        );
    }

    #[test]
    fn test_walk_dir() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/root/a.txt", "a");
        fs.add_file("/root/sub/b.txt", "b");
        fs.add_dir("/root/sub/deep");

        let entries = fs
            .walk_dir(Path::new("/root"), &WalkOptions::default())
            .unwrap();

        let paths: Vec<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        assert!(paths.contains(&Path::new("/root")));
        assert!(paths.contains(&Path::new("/root/a.txt")));
        assert!(paths.contains(&Path::new("/root/sub/b.txt")));
    }

    #[test]
    fn test_walk_dir_min_depth() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/root/a.txt", "a");

        let entries = fs
            .walk_dir(
                Path::new("/root"),
                &WalkOptions {
                    min_depth: 1,
                    ..Default::default()
                },
            )
            .unwrap();

        let paths: Vec<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        assert!(!paths.contains(&Path::new("/root"))); // root excluded
        assert!(paths.contains(&Path::new("/root/a.txt")));
    }

    #[test]
    fn test_walk_dir_max_depth() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/root/a.txt", "a");
        fs.add_file("/root/sub/deep/b.txt", "b");

        let entries = fs
            .walk_dir(
                Path::new("/root"),
                &WalkOptions {
                    max_depth: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();

        let paths: Vec<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        assert!(paths.contains(&Path::new("/root/a.txt")));
        assert!(!paths.contains(&Path::new("/root/sub/deep/b.txt"))); // too deep
    }

    #[test]
    fn test_walk_dir_contents_first() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/root/a.txt", "a");
        fs.add_dir("/root/sub");

        let entries = fs
            .walk_dir(
                Path::new("/root"),
                &WalkOptions {
                    contents_first: true,
                    ..Default::default()
                },
            )
            .unwrap();

        // Files (deeper) should come before directories (shallower)
        let file_idx = entries
            .iter()
            .position(|e| e.path == Path::new("/root/a.txt"))
            .unwrap();
        let root_idx = entries
            .iter()
            .position(|e| e.path == Path::new("/root"))
            .unwrap();
        assert!(file_idx < root_idx);
    }

    #[test]
    fn test_home_and_config_dir() {
        let fs = FakeFs::new("/home/test");
        assert_eq!(fs.home_dir(), Some(PathBuf::from("/home/test")));
        assert_eq!(fs.config_dir(), Some(PathBuf::from("/home/test/.config")));
    }

    #[test]
    fn test_write_preserves_mode() {
        let fs = FakeFs::new("/home/test");
        fs.add_file_with_mode("/tmp/script.sh", "old", 0o755);
        fs.write(Path::new("/tmp/script.sh"), b"new").unwrap();
        assert_eq!(fs.file_mode(Path::new("/tmp/script.sh")).unwrap(), 0o755);
    }

    #[test]
    fn test_auto_creates_parents() {
        let fs = FakeFs::new("/home/test");
        fs.add_file("/a/b/c/d.txt", "deep");

        assert!(fs.is_dir(Path::new("/a")));
        assert!(fs.is_dir(Path::new("/a/b")));
        assert!(fs.is_dir(Path::new("/a/b/c")));
        assert!(fs.is_file(Path::new("/a/b/c/d.txt")));
    }
}
