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
use crate::platform::Fs;
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
#[derive(Debug)]
pub struct FileStatus {
    /// Relative source path (e.g. `hypr/hypr.conf`).
    pub src: String,
    /// Whether the file is currently deployed.
    pub deployed: bool,
    /// Human-readable detail string (e.g. "up to date", "source -> generated diff").
    pub detail: String,
    /// Number of changed lines between generated and staged (0 if identical or missing).
    pub changed_lines: usize,
}

/// Result of computing pipeline status for all files.
#[derive(Debug)]
pub struct StatusResult {
    /// Per-file statuses after filtering.
    pub statuses: Vec<FileStatus>,
    /// Fileset sync summary: `(name, files_changed, total_changed_lines)`.
    pub fileset_summary: Vec<(String, usize, usize)>,
}

/// Compute pipeline status for the given files, applying optional filters.
///
/// Returns the structured result without printing. Use `run()` for the
/// CLI entry point that also displays output.
pub fn compute(
    config: &Config,
    files: Option<&[String]>,
    filters: &StatusFilters,
    fs: &impl Fs,
) -> Result<StatusResult> {
    if filters.deployed && filters.undeployed {
        bail!("Cannot specify both --deployed and --undeployed");
    }

    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        return Ok(StatusResult {
            statuses: Vec::new(),
            fileset_summary: Vec::new(),
        });
    }

    let dotfiles_dir = config.dotfiles_dir(fs);
    let generated_dir = config.generated_dir(fs);
    let staged_dir = config.staged_dir(fs);
    let state = State::load(&dotfiles_dir, fs)?;

    let mut statuses: Vec<FileStatus> = Vec::new();

    for entry in &entries {
        let src = &entry.src;
        let source_path = dotfiles_dir.join(src);
        let target_path = expand_tilde(&entry.target(), fs);

        let (deployed, detail, changed_lines) = if entry.direct {
            let deployed =
                state.is_deployed(src) && is_janus_symlink(&target_path, &source_path, fs);
            let detail = if !fs.exists(&source_path) {
                "source missing".to_string()
            } else if deployed {
                "deployed (direct)".to_string()
            } else {
                "ready to deploy (direct)".to_string()
            };
            (deployed, detail, 0)
        } else {
            let generated_path = generated_dir.join(src);
            let staged_path = staged_dir.join(src);
            let deployed =
                state.is_deployed(src) && is_janus_symlink(&target_path, &staged_path, fs);
            let detail =
                compute_detail(&source_path, &generated_path, &staged_path, deployed, fs);
            let changed_lines = count_changed_lines(&generated_path, &staged_path, fs);
            (deployed, detail, changed_lines)
        };

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

    let fileset_summary = if !config.filesets.is_empty() {
        fileset_sync_summary(config, &statuses)
    } else {
        Vec::new()
    };

    Ok(StatusResult {
        statuses,
        fileset_summary,
    })
}

/// Display pipeline status for the given files, applying optional filters.
///
/// `--deployed` and `--undeployed` are mutually exclusive. `--only-diffs`
/// can be combined with either.
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    filters: StatusFilters,
    fs: &impl Fs,
) -> Result<()> {
    let result = compute(config, files, &filters, fs)?;

    if result.statuses.is_empty() {
        info!("No files match the given filters");
        return Ok(());
    }

    // Find max src width for alignment
    let max_src_len = result
        .statuses
        .iter()
        .map(|s| s.src.len())
        .max()
        .unwrap_or(0);

    for status in &result.statuses {
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

    if !result.fileset_summary.is_empty() {
        println!();
        println!("Filesets needing sync:");
        let max_name_len = result
            .fileset_summary
            .iter()
            .map(|(name, _, _)| name.len())
            .max()
            .unwrap_or(0);
        for (name, files_changed, total_lines) in &result.fileset_summary {
            println!(
                "  {:<width$}  {} file(s) changed, {} line(s)",
                name,
                files_changed,
                total_lines,
                width = max_name_len,
            );
        }
    }

    Ok(())
}

/// Compute a human-readable detail string describing the file's pipeline state.
///
/// Checks existence and content equality at each stage: source -> generated -> staged.
/// Returns descriptions like "up to date", "not yet generated", "source -> generated diff".
fn compute_detail(
    source_path: &Path,
    generated_path: &Path,
    staged_path: &Path,
    is_deployed: bool,
    fs: &impl Fs,
) -> String {
    if !fs.exists(source_path) {
        return "source missing".to_string();
    }

    if !fs.exists(generated_path) {
        return "not yet generated".to_string();
    }

    // Check source vs generated
    let source_matches_generated = files_match(source_path, generated_path, fs);

    if !fs.exists(staged_path) {
        if !source_matches_generated {
            return "source -> generated diff, not yet staged".to_string();
        }
        return "not yet staged".to_string();
    }

    // Check generated vs staged
    let generated_matches_staged = files_match(generated_path, staged_path, fs);

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
fn files_match(a: &Path, b: &Path, fs: &impl Fs) -> bool {
    let Ok(content_a) = fs.read(a) else {
        return false;
    };
    let Ok(content_b) = fs.read(b) else {
        return false;
    };
    content_a == content_b
}

/// Count the number of changed lines between generated and staged files.
///
/// Returns 0 if either file is missing or they are identical.
fn count_changed_lines(generated_path: &Path, staged_path: &Path, fs: &impl Fs) -> usize {
    let Ok(generated) = fs.read_to_string(generated_path) else {
        return 0;
    };
    let Ok(staged) = fs.read_to_string(staged_path) else {
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

use super::is_janus_symlink;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    fn make_filters(only_diffs: bool, deployed: bool, undeployed: bool) -> StatusFilters {
        StatusFilters {
            only_diffs,
            deployed,
            undeployed,
        }
    }

    #[test]
    fn up_to_date() {
        let fs = setup_fs();
        setup_pipeline_file(&fs, "a.conf", "content");
        let staged = format!("{DOTFILES}/.staged/a.conf");
        fs.add_symlink("/home/test/.config/a.conf", &staged);
        let state_toml = "[[deployed]]\nsrc = \"a.conf\"\ntarget = \"~/.config/a.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        );
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].src, "a.conf");
        assert!(result.statuses[0].deployed);
        assert_eq!(result.statuses[0].detail, "up to date");
        assert_eq!(result.statuses[0].changed_lines, 0);
    }

    #[test]
    fn not_generated() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/a.conf"), "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].detail, "not yet generated");
        assert!(!result.statuses[0].deployed);
    }

    #[test]
    fn not_staged() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/a.conf"), "content");
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].detail, "not yet staged");
    }

    #[test]
    fn source_generated_diff() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/a.conf"), "source {{ var }}");
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "source rendered");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "source rendered");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert!(
            result.statuses[0]
                .detail
                .contains("source -> generated diff"),
            "got: {}",
            result.statuses[0].detail
        );
    }

    #[test]
    fn generated_staged_diff() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/a.conf"), "content");
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "content");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "modified in staged");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert!(
            result.statuses[0]
                .detail
                .contains("generated -> staged diff"),
            "got: {}",
            result.statuses[0].detail
        );
        assert!(result.statuses[0].changed_lines > 0);
    }

    #[test]
    fn ready_to_deploy() {
        let fs = setup_fs();
        setup_pipeline_file(&fs, "a.conf", "content");
        // Not deployed — all in sync but no symlink
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", Some("~/.config/a.conf"))]),
        );
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].detail, "ready to deploy");
        assert!(!result.statuses[0].deployed);
    }

    #[test]
    fn deployed_filter() {
        let fs = setup_fs();
        setup_pipeline_file(&fs, "deployed.conf", "content");
        setup_pipeline_file(&fs, "undeployed.conf", "content");
        let staged = format!("{DOTFILES}/.staged/deployed.conf");
        fs.add_symlink("/home/test/.config/deployed.conf", &staged);
        let state_toml =
            "[[deployed]]\nsrc = \"deployed.conf\"\ntarget = \"~/.config/deployed.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[
                ("deployed.conf", Some("~/.config/deployed.conf")),
                ("undeployed.conf", Some("~/.config/undeployed.conf")),
            ]),
        );
        let result = compute(&config, None, &make_filters(false, true, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].src, "deployed.conf");
        assert!(result.statuses[0].deployed);
    }

    #[test]
    fn undeployed_filter() {
        let fs = setup_fs();
        setup_pipeline_file(&fs, "deployed.conf", "content");
        setup_pipeline_file(&fs, "undeployed.conf", "content");
        let staged = format!("{DOTFILES}/.staged/deployed.conf");
        fs.add_symlink("/home/test/.config/deployed.conf", &staged);
        let state_toml =
            "[[deployed]]\nsrc = \"deployed.conf\"\ntarget = \"~/.config/deployed.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[
                ("deployed.conf", Some("~/.config/deployed.conf")),
                ("undeployed.conf", Some("~/.config/undeployed.conf")),
            ]),
        );
        let result = compute(&config, None, &make_filters(false, false, true), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].src, "undeployed.conf");
        assert!(!result.statuses[0].deployed);
    }

    #[test]
    fn both_filters_error() {
        let fs = setup_fs();
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let result = compute(&config, None, &make_filters(false, true, true), &fs);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("--deployed"), "got: {msg}");
        assert!(msg.contains("--undeployed"), "got: {msg}");
    }

    #[test]
    fn only_diffs_filter() {
        let fs = setup_fs();
        setup_pipeline_file(&fs, "a.conf", "same");
        let staged_a = format!("{DOTFILES}/.staged/a.conf");
        fs.add_symlink("/home/test/.config/a.conf", &staged_a);
        // b.conf has a diff (generated != staged)
        fs.add_file(format!("{DOTFILES}/b.conf"), "src");
        fs.add_file(format!("{DOTFILES}/.generated/b.conf"), "src");
        fs.add_file(format!("{DOTFILES}/.staged/b.conf"), "different");
        let state_toml = "[[deployed]]\nsrc = \"a.conf\"\ntarget = \"~/.config/a.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[
                ("a.conf", Some("~/.config/a.conf")),
                ("b.conf", Some("~/.config/b.conf")),
            ]),
        );
        let result = compute(&config, None, &make_filters(true, false, false), &fs).unwrap();
        // Only b.conf should appear (it has a diff)
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].src, "b.conf");
        assert!(
            result.statuses[0].detail.contains("diff"),
            "got: {}",
            result.statuses[0].detail
        );
    }

    #[test]
    fn source_missing() {
        let fs = setup_fs();
        // Source file doesn't exist, but config references it
        let config = write_and_load_config(&fs, &make_config_toml(&[("missing.conf", None)]));
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert_eq!(result.statuses[0].detail, "source missing");
    }

    #[test]
    fn fileset_sync_summary_computed() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/hypr/hypr.conf"), "src");
        fs.add_file(format!("{DOTFILES}/.generated/hypr/hypr.conf"), "src");
        fs.add_file(
            format!("{DOTFILES}/.staged/hypr/hypr.conf"),
            "modified\nlines\n",
        );
        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"
vars = ["vars.toml"]

[[files]]
src = "hypr/hypr.conf"

[filesets.desktop]
patterns = ["hypr/*"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.fileset_summary.len(), 1);
        assert_eq!(result.fileset_summary[0].0, "desktop");
        assert_eq!(result.fileset_summary[0].1, 1); // 1 file changed
        assert!(result.fileset_summary[0].2 > 0); // some lines changed
    }

    #[test]
    fn direct_file_deployed() {
        let fs = setup_fs();
        let source = format!("{DOTFILES}/direct.conf");
        fs.add_file(&source, "content");
        fs.add_symlink("/home/test/.config/direct.conf", &source);
        let state_toml =
            "[[deployed]]\nsrc = \"direct.conf\"\ntarget = \"~/.config/direct.conf\"\n";
        fs.add_file(format!("{DOTFILES}/.janus_state.toml"), state_toml);
        let toml = format!(
            "dotfiles_dir = \"{DOTFILES}\"\nvars = [\"vars.toml\"]\n\n[[files]]\nsrc = \"direct.conf\"\ntarget = \"~/.config/direct.conf\"\ndirect = true\ntemplate = false\n"
        );
        let config = write_and_load_config(&fs, &toml);
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert!(result.statuses[0].deployed);
        assert_eq!(result.statuses[0].detail, "deployed (direct)");
        assert_eq!(result.statuses[0].changed_lines, 0);
    }

    #[test]
    fn direct_file_ready() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/direct.conf"), "content");
        let toml = format!(
            "dotfiles_dir = \"{DOTFILES}\"\nvars = [\"vars.toml\"]\n\n[[files]]\nsrc = \"direct.conf\"\ntarget = \"~/.config/direct.conf\"\ndirect = true\ntemplate = false\n"
        );
        let config = write_and_load_config(&fs, &toml);
        let result = compute(&config, None, &make_filters(false, false, false), &fs).unwrap();
        assert_eq!(result.statuses.len(), 1);
        assert!(!result.statuses[0].deployed);
        assert_eq!(result.statuses[0].detail, "ready to deploy (direct)");
    }
}
