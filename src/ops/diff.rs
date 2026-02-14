use anyhow::{Context, Result};
use similar::{ChangeTag, TextDiff};
use tracing::info;

use crate::config::Config;

pub fn run(config: &Config, files: Option<&[String]>) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        info!("No files to diff");
        return Ok(());
    }

    let generated_dir = config.generated_dir();
    let staged_dir = config.staged_dir();
    let mut any_diff = false;

    for entry in &entries {
        let generated_path = generated_dir.join(&entry.src);
        let staged_path = staged_dir.join(&entry.src);

        if !generated_path.exists() {
            info!("{}: no generated file (run `janus generate` first)", entry.src);
            continue;
        }
        if !staged_path.exists() {
            info!("{}: no staged file (run `janus stage` first)", entry.src);
            continue;
        }

        let generated_content = std::fs::read_to_string(&generated_path)
            .with_context(|| format!("Failed to read generated file: {}", generated_path.display()))?;
        let staged_content = std::fs::read_to_string(&staged_path)
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
