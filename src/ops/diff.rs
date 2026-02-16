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

/// Display diffs between generated and staged versions of the given files.
///
/// Files with no diff are silently skipped. Missing generated or staged files
/// are reported but don't cause an error.
pub fn run(config: &Config, files: Option<&[String]>, fs: &impl Fs) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to diff");
        return Ok(());
    }

    let generated_dir = config.generated_dir(fs);
    let staged_dir = config.staged_dir(fs);
    let mut any_diff = false;

    for entry in &entries {
        let generated_path = generated_dir.join(&entry.src);
        let staged_path = staged_dir.join(&entry.src);

        if !fs.exists(&generated_path) {
            info!(
                "{}: no generated file (run `janus generate` first)",
                entry.src
            );
            continue;
        }
        if !fs.exists(&staged_path) {
            info!("{}: no staged file (run `janus stage` first)", entry.src);
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
            continue;
        }

        any_diff = true;
        println!("--- generated/{}", entry.src);
        println!("+++ staged/{}", entry.src);

        let diff = TextDiff::from_lines(&generated_content, &staged_content);
        for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
            println!("{}", hunk.header());
            for change in hunk.iter_changes() {
                let (prefix, color_start, color_end) = match change.tag() {
                    ChangeTag::Delete => ("-", "\x1b[31m", "\x1b[0m"),
                    ChangeTag::Insert => ("+", "\x1b[32m", "\x1b[0m"),
                    ChangeTag::Equal => (" ", "", ""),
                };
                print!("{color_start}{prefix}{change}{color_end}");
                if change.missing_newline() {
                    println!();
                }
            }
        }
        println!();
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
    fn identical_no_error() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "same");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "same");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None)]),
        );
        run(&config, None, &fs).unwrap();
    }

    #[test]
    fn different_files_succeeds() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "old");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "new");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None)]),
        );
        run(&config, None, &fs).unwrap();
    }

    #[test]
    fn missing_generated_no_error() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "staged");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None)]),
        );
        run(&config, None, &fs).unwrap();
    }

    #[test]
    fn missing_staged_no_error() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "generated");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None)]),
        );
        run(&config, None, &fs).unwrap();
    }
}
