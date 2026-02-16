//! Real filesystem implementation delegating to `std::fs`, `std::os::unix::fs`,
//! `walkdir`, and `dirs`.

use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::{DirEntry, Fs, WalkOptions};

/// Real filesystem — delegates every operation to the OS.
pub struct RealFs;

impl Fs for RealFs {
    fn read_to_string(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {}", path.display()))
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path).with_context(|| format!("Failed to read file: {}", path.display()))
    }

    fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        std::fs::write(path, contents)
            .with_context(|| format!("Failed to write file: {}", path.display()))
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        std::fs::copy(from, to).with_context(|| {
            format!(
                "Failed to copy {} -> {}",
                from.display(),
                to.display()
            )
        })?;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove file: {}", path.display()))
    }

    fn remove_dir(&self, path: &Path) -> Result<()> {
        std::fs::remove_dir(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        std::fs::rename(from, to).with_context(|| {
            format!(
                "Failed to rename {} -> {}",
                from.display(),
                to.display()
            )
        })
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(path)
            .with_context(|| format!("Failed to create directory: {}", path.display()))
    }

    fn file_mode(&self, path: &Path) -> Result<u32> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Failed to read metadata: {}", path.display()))?;
        Ok(metadata.permissions().mode())
    }

    fn set_file_mode(&self, path: &Path, mode: u32) -> Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("Failed to set permissions: {}", path.display()))
    }

    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        std::os::unix::fs::symlink(original, link).with_context(|| {
            format!(
                "Failed to create symlink: {} -> {}",
                link.display(),
                original.display()
            )
        })
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        std::fs::read_link(path)
            .with_context(|| format!("Failed to read symlink: {}", path.display()))
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
