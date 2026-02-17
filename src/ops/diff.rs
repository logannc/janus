//! Show unified diffs between `.generated/` and `.staged/` files.
//!
//! This is a read-only operation that helps inspect what changed between
//! the last generation and the last staging. Uses the `similar` crate for
//! diff computation with colored terminal output.

use anyhow::{Context, Result};
use similar::{ChangeTag, TextDiff};
use tracing::info;

use crate::config::Config;
use crate::platform::Fs;

/// Computed diff result for a single file.
pub struct FileDiff {
    /// Relative source path.
    pub src: String,
    /// What happened: `Identical`, `MissingGenerated`, `MissingStaged`, or `Changed`.
    pub kind: DiffKind,
}

/// Classification of a file's diff result.
#[derive(Debug)]
pub enum DiffKind {
    /// Generated and staged are identical.
    Identical,
    /// No generated file exists.
    MissingGenerated,
    /// No staged file exists.
    MissingStaged,
    /// Files differ; contains the unified diff text.
    Changed(String),
}

/// Compute diffs between generated and staged versions of the given files.
///
/// Returns structured results without printing.
pub fn compute(config: &Config, files: Option<&[String]>, fs: &impl Fs) -> Result<Vec<FileDiff>> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        return Ok(Vec::new());
    }

    let generated_dir = config.generated_dir(fs);
    let staged_dir = config.staged_dir(fs);
    let mut results = Vec::new();

    for entry in &entries {
        let generated_path = generated_dir.join(&entry.src);
        let staged_path = staged_dir.join(&entry.src);

        if !fs.exists(&generated_path) {
            results.push(FileDiff {
                src: entry.src.clone(),
                kind: DiffKind::MissingGenerated,
            });
            continue;
        }
        if !fs.exists(&staged_path) {
            results.push(FileDiff {
                src: entry.src.clone(),
                kind: DiffKind::MissingStaged,
            });
            continue;
        }

        let generated_content = fs.read_to_string(&generated_path).with_context(|| {
            format!(
                "Failed to read generated file: {}",
                generated_path.display()
            )
        })?;
        let staged_content = fs
            .read_to_string(&staged_path)
            .with_context(|| format!("Failed to read staged file: {}", staged_path.display()))?;

        if generated_content == staged_content {
            results.push(FileDiff {
                src: entry.src.clone(),
                kind: DiffKind::Identical,
            });
            continue;
        }

        let diff = TextDiff::from_lines(&generated_content, &staged_content);
        let mut diff_text = format!("--- generated/{}\n+++ staged/{}\n", entry.src, entry.src);
        for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
            diff_text.push_str(&format!("{}", hunk.header()));
            for change in hunk.iter_changes() {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                diff_text.push_str(&format!("{prefix}{change}"));
                if change.missing_newline() {
                    diff_text.push('\n');
                }
            }
        }

        results.push(FileDiff {
            src: entry.src.clone(),
            kind: DiffKind::Changed(diff_text),
        });
    }

    Ok(results)
}

/// Display diffs between generated and staged versions of the given files.
///
/// Files with no diff are silently skipped. Missing generated or staged files
/// are reported but don't cause an error.
pub fn run(config: &Config, files: Option<&[String]>, fs: &impl Fs) -> Result<()> {
    let results = compute(config, files, fs)?;

    let mut any_diff = false;
    for result in &results {
        match &result.kind {
            DiffKind::MissingGenerated => {
                info!(
                    "{}: no generated file (run `janus generate` first)",
                    result.src
                );
            }
            DiffKind::MissingStaged => {
                info!("{}: no staged file (run `janus stage` first)", result.src);
            }
            DiffKind::Identical => {}
            DiffKind::Changed(diff_text) => {
                any_diff = true;
                // Re-print with colors for terminal output
                println!("--- generated/{}", result.src);
                println!("+++ staged/{}", result.src);
                for line in diff_text.lines().skip(2) {
                    let (color_start, color_end) = if line.starts_with('-') {
                        ("\x1b[31m", "\x1b[0m")
                    } else if line.starts_with('+') {
                        ("\x1b[32m", "\x1b[0m")
                    } else {
                        ("", "")
                    };
                    println!("{color_start}{line}{color_end}");
                }
                println!();
            }
        }
    }

    if !any_diff {
        info!("No differences found");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[test]
    fn identical_returns_identical() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "same");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "same");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let results = compute(&config, None, &fs).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].src, "a.conf");
        assert!(matches!(results[0].kind, DiffKind::Identical));
    }

    #[test]
    fn different_files_returns_changed() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "old\n");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "new\n");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let results = compute(&config, None, &fs).unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].kind {
            DiffKind::Changed(text) => {
                assert!(
                    text.contains("-old"),
                    "diff should show removed line, got: {text}"
                );
                assert!(
                    text.contains("+new"),
                    "diff should show added line, got: {text}"
                );
            }
            other => panic!("expected Changed, got: {other:?}"),
        }
    }

    #[test]
    fn missing_generated_detected() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "staged");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let results = compute(&config, None, &fs).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].kind, DiffKind::MissingGenerated));
    }

    #[test]
    fn missing_staged_detected() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "generated");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let results = compute(&config, None, &fs).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].kind, DiffKind::MissingStaged));
    }

    #[test]
    fn multiple_files_each_classified() {
        let fs = setup_fs();
        // a.conf: identical
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "same");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "same");
        // b.conf: changed
        fs.add_file(format!("{DOTFILES}/.generated/b.conf"), "old\n");
        fs.add_file(format!("{DOTFILES}/.staged/b.conf"), "new\n");
        // c.conf: missing generated
        fs.add_file(format!("{DOTFILES}/.staged/c.conf"), "staged");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None), ("c.conf", None)]),
        );
        let results = compute(&config, None, &fs).unwrap();
        assert_eq!(results.len(), 3);
        assert!(matches!(results[0].kind, DiffKind::Identical));
        assert!(matches!(results[1].kind, DiffKind::Changed(_)));
        assert!(matches!(results[2].kind, DiffKind::MissingGenerated));
    }
}
