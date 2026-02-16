//! Real filesystem implementation delegating to `std::fs`, `std::os::unix::fs`,
//! `walkdir`, and `dirs`.
//!
//! Methods return bare errors without added context — callers add their own
//! `.with_context()` messages for domain-specific error descriptions.

use anyhow::Result;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::{DirEntry, Fs, WalkOptions};

/// Real filesystem — delegates every operation to the OS.
pub struct RealFs;

impl Fs for RealFs {
    fn read_to_string(&self, path: &Path) -> Result<String> {
        Ok(std::fs::read_to_string(path)?)
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(std::fs::read(path)?)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        Ok(std::fs::write(path, contents)?)
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        std::fs::copy(from, to)?;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        Ok(std::fs::remove_file(path)?)
    }

    fn remove_dir(&self, path: &Path) -> Result<()> {
        Ok(std::fs::remove_dir(path)?)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        Ok(std::fs::rename(from, to)?)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        Ok(std::fs::create_dir_all(path)?)
    }

    fn file_mode(&self, path: &Path) -> Result<u32> {
        Ok(std::fs::metadata(path)?.permissions().mode())
    }

    fn set_file_mode(&self, path: &Path, mode: u32) -> Result<()> {
        Ok(std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(mode),
        )?)
    }

    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        Ok(std::os::unix::fs::symlink(original, link)?)
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        Ok(std::fs::read_link(path)?)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_symlink(&self, path: &Path) -> bool {
        path.is_symlink()
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn walk_dir(&self, path: &Path, opts: &WalkOptions) -> Result<Vec<DirEntry>> {
        let mut walker = WalkDir::new(path).min_depth(opts.min_depth);
        if let Some(max_depth) = opts.max_depth {
            walker = walker.max_depth(max_depth);
        }
        walker = walker
            .follow_links(opts.follow_links)
            .contents_first(opts.contents_first);

        let entries = walker
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| {
                let ft = e.file_type();
                let entry_path = e.into_path();
                let is_symlink = entry_path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false);
                DirEntry {
                    path: entry_path,
                    is_file: ft.is_file(),
                    is_dir: ft.is_dir(),
                    is_symlink,
                }
            })
            .collect();

        Ok(entries)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    fn config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir()
    }
}
