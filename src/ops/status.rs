//! Read-only overview of each managed file's state across the pipeline.
//!
//! For each configured file, checks whether the source, generated, staged,
//! and deployed versions exist and are in sync. Supports filtering by
//! deployment state and diff presence.

use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::State;

/// Filtering options for the status display.
pub struct StatusFilters {
    /// Only show files where source != generated or generated != staged.
    pub only_diffs: bool,
    /// Only show files that are currently deployed.
    pub deployed: bool,
    /// Only show files that are NOT currently deployed.
    pub undeployed: bool,
}

/// Computed status for a single managed file.
struct FileStatus {
    /// Relative source path (e.g. `hypr/hypr.conf`).
    src: String,
    /// Whether the file is currently deployed.
    deployed: bool,
    /// Human-readable detail string (e.g. "up to date", "source -> generated diff").
    detail: String,
    /// Number of changed lines between generated and staged (0 if identical or missing).
    changed_lines: usize,
}

/// Display pipeline status for the given files, applying optional filters.
///
/// `--deployed` and `--undeployed` are mutually exclusive. `--only-diffs`
/// can be combined with either.
pub fn run(config: &Config, files: Option<&[String]>, filters: StatusFilters) -> Result<()> {
    if filters.deployed && filters.undeployed {
        bail!("Cannot specify both --deployed and --undeployed");
    }

    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to check");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir();
    let generated_dir = config.generated_dir();
    let staged_dir = config.staged_dir();
    let state = State::load(&dotfiles_dir)?;

    let mut statuses: Vec<FileStatus> = Vec::new();

    for entry in &entries {
        let src = &entry.src;
        let source_path = dotfiles_dir.join(src);
        let generated_path = generated_dir.join(src);
        let staged_path = staged_dir.join(src);
        let target_path = expand_tilde(&entry.target());

        let deployed = state.is_deployed(src) && is_janus_symlink(&target_path, &staged_path);

        let detail = compute_detail(&source_path, &generated_path, &staged_path, deployed);

        let changed_lines = count_changed_lines(&generated_path, &staged_path);

        let has_diff =
            detail.contains("diff") || detail.contains("missing") || detail.contains("not yet");

        // Apply filters
        if filters.deployed && !deployed {
            continue;
        }
        if filters.undeployed && deployed {
            continue;
        }
        if filters.only_diffs && !has_diff {
            continue;
        }

        statuses.push(FileStatus {
            src: src.clone(),
            deployed,
            detail,
            changed_lines,
        });
    }

    if statuses.is_empty() {
        info!("No files match the given filters");
        return Ok(());
    }

    // Find max src width for alignment
    let max_src_len = statuses.iter().map(|s| s.src.len()).max().unwrap_or(0);

    for status in &statuses {
        let state_str = if status.deployed {
            "deployed  "
        } else {
            "undeployed"
        };

        println!(
            "  {:<width$}  {}  ({})",
            status.src,
            state_str,
            status.detail,
            width = max_src_len,
        );
    }

    // Fileset sync summary
    if !config.filesets.is_empty() {
        let summary = fileset_sync_summary(config, &statuses);
        if !summary.is_empty() {
            println!();
            println!("Filesets needing sync:");
            let max_name_len = summary
                .iter()
                .map(|(name, _, _)| name.len())
                .max()
                .unwrap_or(0);
            for (name, files_changed, total_lines) in &summary {
                println!(
                    "  {:<width$}  {} file(s) changed, {} line(s)",
                    name,
                    files_changed,
                    total_lines,
                    width = max_name_len,
                );
            }
        }
    }

    Ok(())
}

/// Compute a human-readable detail string describing the file's pipeline state.
///
/// Checks existence and content equality at each stage: source → generated → staged.
/// Returns descriptions like "up to date", "not yet generated", "source -> generated diff".
fn compute_detail(
    source_path: &Path,
    generated_path: &Path,
    staged_path: &Path,
    is_deployed: bool,
) -> String {
    if !source_path.exists() {
        return "source missing".to_string();
    }

    if !generated_path.exists() {
        return "not yet generated".to_string();
    }

    // Check source vs generated
    let source_matches_generated = files_match(source_path, generated_path);

    if !staged_path.exists() {
        if !source_matches_generated {
            return "source -> generated diff, not yet staged".to_string();
        }
        return "not yet staged".to_string();
    }

    // Check generated vs staged
    let generated_matches_staged = files_match(generated_path, staged_path);

    let mut parts = Vec::new();

    if !source_matches_generated {
        parts.push("source -> generated diff");
    }

    if !generated_matches_staged {
        parts.push("generated -> staged diff");
    }

    if parts.is_empty() {
        if is_deployed {
            "up to date".to_string()
        } else {
            "ready to deploy".to_string()
        }
    } else {
        parts.join(", ")
    }
}

/// Compare two files by content. Returns false if either file can't be read.
fn files_match(a: &Path, b: &Path) -> bool {
    let Ok(content_a) = std::fs::read(a) else {
        return false;
    };
    let Ok(content_b) = std::fs::read(b) else {
        return false;
    };
    content_a == content_b
}

/// Count the number of changed lines between generated and staged files.
///
/// Returns 0 if either file is missing or they are identical.
fn count_changed_lines(generated_path: &Path, staged_path: &Path) -> usize {
    let Ok(generated) = std::fs::read_to_string(generated_path) else {
        return 0;
    };
    let Ok(staged) = std::fs::read_to_string(staged_path) else {
        return 0;
    };
    if generated == staged {
        return 0;
    }
    let diff = similar::TextDiff::from_lines(&generated, &staged);
    diff.ops()
        .iter()
        .map(|op| match *op {
            similar::DiffOp::Equal { .. } => 0,
            similar::DiffOp::Delete { old_len, .. } => old_len,
            similar::DiffOp::Insert { new_len, .. } => new_len,
            similar::DiffOp::Replace {
                old_len, new_len, ..
            } => old_len + new_len,
        })
        .sum()
}

/// Build a sorted summary of filesets with changed files.
///
/// Returns `(fileset_name, files_changed, total_changed_lines)` sorted by
/// total changed lines descending.
fn fileset_sync_summary(config: &Config, statuses: &[FileStatus]) -> Vec<(String, usize, usize)> {
    let mut summary: HashMap<&str, (usize, usize)> = HashMap::new();

    for (name, fileset) in &config.filesets {
        for status in statuses {
            if status.changed_lines == 0 {
                continue;
            }
            let matches = fileset.patterns.iter().any(|pattern| {
                if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                    glob_pattern.matches(&status.src)
                } else {
                    status.src == *pattern
                }
            });
            if matches {
                let entry = summary.entry(name.as_str()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 += status.changed_lines;
            }
        }
    }

    let mut result: Vec<(String, usize, usize)> = summary
        .into_iter()
        .map(|(name, (files, lines))| (name.to_string(), files, lines))
        .collect();
    result.sort_by(|a, b| b.2.cmp(&a.2));
    result
}

/// Check if `target` is a symlink pointing to `expected_staged`.
fn is_janus_symlink(target: &Path, expected_staged: &Path) -> bool {
    if !target.is_symlink() {
        return false;
    }
    match std::fs::read_link(target) {
        Ok(link_dest) => link_dest == expected_staged,
        Err(_) => false,
    }
}
