//! Interactively merge staged changes back into source templates.
//!
//! When a deployed config is modified (by an application or user), the change
//! writes through the symlink to `.staged/`. This command diffs generated
//! (common ancestor) vs staged (current deployed content) and lets the user
//! choose per-hunk whether to apply the staged change back to the source.
//!
//! Uses error-collection strategy: processes all files and reports failures
//! at the end rather than bailing on the first error.

use anyhow::{Context, Result};
use similar::DiffOp;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use crate::config::{Config, FileEntry};
use crate::platform::{Fs, Prompter};

/// Run interactive sync for the given file patterns (or all files).
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    dry_run: bool,
    fs: &impl Fs,
    prompter: &impl Prompter,
) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to sync");
        return Ok(());
    }

    let mut errors: Vec<(String, anyhow::Error)> = Vec::new();
    let mut modified = 0usize;
    for entry in &entries {
        match sync_file(config, entry, dry_run, fs, prompter) {
            Ok(true) => modified += 1,
            Ok(false) => {}
            Err(e) => {
                warn!("Failed to sync {}: {e:#}", entry.src);
                errors.push((entry.src.clone(), e));
            }
        }
    }

    if modified > 0 {
        info!("Modified {} source file(s)", modified);
        println!("\nRun `janus generate` to re-render updated templates.");
    } else if errors.is_empty() {
        info!("No files needed syncing");
    }

    if !errors.is_empty() {
        info!("{} file(s) failed to sync", errors.len());
        let mut msg = format!("Failed to sync {} file(s):", errors.len());
        for (src, e) in &errors {
            msg.push_str(&format!("\n  {src}: {e:#}"));
        }
        anyhow::bail!(msg);
    }

    Ok(())
}

/// Check if a line contains Tera template syntax.
fn has_tera_syntax(line: &str) -> bool {
    line.contains("{{") || line.contains("{%") || line.contains("{#")
}

/// Split text into lines, keeping the newline character attached to each line.
///
/// This matches `similar`'s internal line splitting so DiffOp indices
/// correspond correctly to our line arrays.
fn split_lines_inclusive(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            lines.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Sync a single file. Returns `Ok(true)` if the source was modified.
fn sync_file(
    config: &Config,
    entry: &FileEntry,
    dry_run: bool,
    fs: &impl Fs,
    prompter: &impl Prompter,
) -> Result<bool> {
    let dotfiles_dir = config.dotfiles_dir(fs);
    let generated_dir = config.generated_dir(fs);
    let staged_dir = config.staged_dir(fs);

    let source_path = dotfiles_dir.join(&entry.src);
    let generated_path = generated_dir.join(&entry.src);
    let staged_path = staged_dir.join(&entry.src);

    // Read all three versions
    if !fs.exists(&generated_path) {
        anyhow::bail!("no generated file (run `janus generate` first)");
    }
    if !fs.exists(&staged_path) {
        anyhow::bail!("no staged file (run `janus stage` first)");
    }
    if !fs.exists(&source_path) {
        anyhow::bail!("source file not found: {}", source_path.display());
    }

    let source = fs
        .read_to_string(&source_path)
        .with_context(|| format!("Failed to read source: {}", source_path.display()))?;
    let generated = fs
        .read_to_string(&generated_path)
        .with_context(|| format!("Failed to read generated: {}", generated_path.display()))?;
    let staged = fs
        .read_to_string(&staged_path)
        .with_context(|| format!("Failed to read staged: {}", staged_path.display()))?;

    // No changes to sync
    if generated == staged {
        debug!(
            "{}: generated and staged are identical, skipping",
            entry.src
        );
        return Ok(false);
    }

    let source_lines = split_lines_inclusive(&source);
    let generated_lines = split_lines_inclusive(&generated);
    let staged_lines = split_lines_inclusive(&staged);

    // For template files, check for structural mismatch
    if entry.template && source_lines.len() != generated_lines.len() {
        warn!(
            "{}: source ({} lines) and generated ({} lines) have different line counts — \
             structural template directives prevent line-level sync, skipping",
            entry.src,
            source_lines.len(),
            generated_lines.len()
        );
        return Ok(false);
    }

    // Build set of line indices where source has template syntax
    let template_affected: HashSet<usize> = if entry.template {
        source_lines
            .iter()
            .enumerate()
            .filter(|(_, line)| has_tera_syntax(line))
            .map(|(i, _)| i)
            .collect()
    } else {
        HashSet::new()
    };

    // Diff generated vs staged
    let diff = similar::TextDiff::from_lines(&generated, &staged);
    let ops: Vec<DiffOp> = diff.ops().to_vec();

    // Count non-equal hunks for display numbering
    let total_hunks = ops
        .iter()
        .filter(|op| !matches!(op, DiffOp::Equal { .. }))
        .count();

    if total_hunks == 0 {
        return Ok(false);
    }

    println!(
        "\n=== {} ({} hunk{}) ===",
        entry.src,
        total_hunks,
        if total_hunks == 1 { "" } else { "s" }
    );

    // Build output from source lines, selectively applying staged changes
    let mut output_lines: Vec<&str> = Vec::new();
    let mut any_applied = false;
    let mut hunk_num = 0;

    for op in &ops {
        match *op {
            DiffOp::Equal { old_index, len, .. } => {
                // Copy source lines for equal regions (preserves template syntax)
                for source_line in source_lines.iter().skip(old_index).take(len) {
                    output_lines.push(source_line)
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                hunk_num += 1;
                let staged_range = &staged_lines[new_index..new_index + new_len];

                if dry_run {
                    print_insert_hunk(&entry.src, hunk_num, total_hunks, new_index, staged_range);
                    println!("  [dry-run] Would prompt: default Apply");
                } else {
                    print_insert_hunk(&entry.src, hunk_num, total_hunks, new_index, staged_range);

                    let selection =
                        prompter.select("Action", &["Apply", "Skip"], 0)?;

                    if selection == 0 {
                        for line in staged_range {
                            output_lines.push(line);
                        }
                        any_applied = true;
                    }
                    // Skip = don't add anything (lines didn't exist in source)
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                hunk_num += 1;
                let source_range: Vec<&str> = if entry.template {
                    (old_index..old_index + old_len)
                        .map(|i| source_lines[i])
                        .collect()
                } else {
                    (old_index..old_index + old_len)
                        .map(|i| source_lines[i])
                        .collect()
                };
                let gen_range: Vec<&str> = (old_index..old_index + old_len)
                    .map(|i| generated_lines[i])
                    .collect();

                let classification = classify_hunk(
                    &source_range,
                    &gen_range,
                    old_index,
                    old_len,
                    &template_affected,
                    entry.template,
                );

                let default_idx = if classification.is_safe { 0 } else { 1 };

                if dry_run {
                    print_delete_hunk(
                        &entry.src,
                        hunk_num,
                        total_hunks,
                        old_index,
                        old_len,
                        &source_range,
                        &classification,
                    );
                    println!(
                        "  [dry-run] Would prompt: default {}",
                        if classification.is_safe {
                            "Apply"
                        } else {
                            "Skip"
                        }
                    );
                    // Preserve source lines in dry-run
                    for line in &source_range {
                        output_lines.push(line);
                    }
                } else {
                    print_delete_hunk(
                        &entry.src,
                        hunk_num,
                        total_hunks,
                        old_index,
                        old_len,
                        &source_range,
                        &classification,
                    );

                    let selection = prompter.select(
                        "Action",
                        &["Apply (delete lines)", "Skip (keep source)"],
                        default_idx,
                    )?;

                    if selection == 0 {
                        // Apply = delete these lines (don't add them to output)
                        any_applied = true;
                    } else {
                        for line in &source_range {
                            output_lines.push(line);
                        }
                    }
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                hunk_num += 1;
                let source_range: Vec<&str> = if entry.template {
                    (old_index..old_index + old_len)
                        .map(|i| source_lines[i])
                        .collect()
                } else {
                    (old_index..old_index + old_len)
                        .map(|i| source_lines[i])
                        .collect()
                };
                let gen_range: Vec<&str> = (old_index..old_index + old_len)
                    .map(|i| generated_lines[i])
                    .collect();
                let staged_range = &staged_lines[new_index..new_index + new_len];

                let classification = classify_hunk(
                    &source_range,
                    &gen_range,
                    old_index,
                    old_len,
                    &template_affected,
                    entry.template,
                );

                let default_idx = if classification.is_safe { 0 } else { 1 };

                if dry_run {
                    print_replace_hunk(
                        &entry.src,
                        hunk_num,
                        total_hunks,
                        old_index,
                        old_len,
                        &source_range,
                        staged_range,
                        &classification,
                    );
                    println!(
                        "  [dry-run] Would prompt: default {}",
                        if classification.is_safe {
                            "Apply"
                        } else {
                            "Skip"
                        }
                    );
                    // Preserve source lines in dry-run
                    for line in &source_range {
                        output_lines.push(line);
                    }
                } else {
                    print_replace_hunk(
                        &entry.src,
                        hunk_num,
                        total_hunks,
                        old_index,
                        old_len,
                        &source_range,
                        staged_range,
                        &classification,
                    );

                    let selection = prompter.select(
                        "Action",
                        &["Apply (take staged)", "Skip (keep source)"],
                        default_idx,
                    )?;

                    if selection == 0 {
                        for line in staged_range {
                            output_lines.push(line);
                        }
                        any_applied = true;
                    } else {
                        for line in &source_range {
                            output_lines.push(line);
                        }
                    }
                }
            }
        }
    }

    if !any_applied {
        debug!("{}: no hunks applied", entry.src);
        return Ok(false);
    }

    if dry_run {
        return Ok(false);
    }

    // Write updated source
    let output: String = output_lines.concat();
    let mode = fs
        .file_mode(&source_path)
        .with_context(|| format!("Failed to read metadata: {}", source_path.display()))?;
    fs.write(&source_path, output.as_bytes())
        .with_context(|| format!("Failed to write source: {}", source_path.display()))?;
    fs.set_file_mode(&source_path, mode)
        .with_context(|| format!("Failed to set permissions: {}", source_path.display()))?;

    info!("Updated source: {}", entry.src);
    Ok(true)
}

struct HunkClassification {
    is_safe: bool,
    annotation: Option<&'static str>,
}

fn classify_hunk(
    source_range: &[&str],
    gen_range: &[&str],
    old_index: usize,
    old_len: usize,
    template_affected: &HashSet<usize>,
    is_template: bool,
) -> HunkClassification {
    let source_matches_generated = source_range == gen_range;

    let has_template =
        is_template && (old_index..old_index + old_len).any(|i| template_affected.contains(&i));

    if source_matches_generated && !has_template {
        HunkClassification {
            is_safe: true,
            annotation: None,
        }
    } else if has_template {
        HunkClassification {
            is_safe: false,
            annotation: Some(
                "(!) Template syntax \u{2014} applying would replace template expressions",
            ),
        }
    } else {
        HunkClassification {
            is_safe: false,
            annotation: Some(
                "(!) Source was independently edited \u{2014} applying would overwrite your changes",
            ),
        }
    }
}

fn print_insert_hunk(src: &str, hunk_num: usize, total: usize, new_index: usize, staged: &[&str]) {
    println!(
        "\n--- {}: hunk {}/{} (insert after line {}) ---",
        src, hunk_num, total, new_index
    );
    println!("\n  Staged (new lines):");
    for line in staged {
        print!("    \x1b[32m+{}\x1b[0m", line);
        if !line.ends_with('\n') {
            println!();
        }
    }
    println!();
}

fn print_delete_hunk(
    src: &str,
    hunk_num: usize,
    total: usize,
    old_index: usize,
    old_len: usize,
    source_range: &[&str],
    classification: &HunkClassification,
) {
    println!(
        "\n--- {}: hunk {}/{} (lines {}-{}) ---",
        src,
        hunk_num,
        total,
        old_index + 1,
        old_index + old_len
    );
    println!("\n  Source (would be deleted):");
    for line in source_range {
        print!("    \x1b[31m-{}\x1b[0m", line);
        if !line.ends_with('\n') {
            println!();
        }
    }
    if let Some(annotation) = classification.annotation {
        println!("\n  {}", annotation);
    }
    println!();
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakePrompter;
    use crate::test_helpers::*;
    use std::collections::HashSet;

    fn sync_setup(
        fs: &crate::platform::FakeFs,
        source: &str,
        generated: &str,
        staged: &str,
    ) -> Config {
        fs.add_file(format!("{DOTFILES}/a.conf"), source);
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), generated);
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), staged);
        write_and_load_config(fs, &make_config_toml(&[("a.conf", None)]))
    }

    #[test]
    fn no_diff_skips() {
        let fs = setup_fs();
        let config = sync_setup(&fs, "same\n", "same\n", "same\n");
        let prompter = FakePrompter::new(vec![]); // No prompts
        run(&config, None, false, &fs, &prompter).unwrap();
    }

    #[test]
    fn apply_replace_hunk() {
        let fs = setup_fs();
        let config = sync_setup(&fs, "old line\n", "old line\n", "new line\n");
        let prompter = FakePrompter::new(vec![0]); // Apply
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/a.conf")))
            .unwrap();
        assert_eq!(content, "new line\n");
    }

    #[test]
    fn skip_replace_hunk() {
        let fs = setup_fs();
        let config = sync_setup(&fs, "old line\n", "old line\n", "new line\n");
        let prompter = FakePrompter::new(vec![1]); // Skip
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/a.conf")))
            .unwrap();
        assert_eq!(content, "old line\n");
    }

    #[test]
    fn apply_insert_hunk() {
        let fs = setup_fs();
        let config = sync_setup(&fs, "line1\n", "line1\n", "line1\nnewline\n");
        let prompter = FakePrompter::new(vec![0]); // Apply
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/a.conf")))
            .unwrap();
        assert!(content.contains("newline"), "got: {content}");
    }

    #[test]
    fn apply_delete_hunk() {
        let fs = setup_fs();
        let config = sync_setup(&fs, "line1\nline2\n", "line1\nline2\n", "line1\n");
        let prompter = FakePrompter::new(vec![0]); // Apply (delete)
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/a.conf")))
            .unwrap();
        assert!(!content.contains("line2"), "got: {content}");
    }

    #[test]
    fn template_hunk_default_skip() {
        let fs = setup_fs();
        // Source has template syntax
        let source = "{{ var }}\n";
        let generated = "rendered\n";
        let staged = "changed\n";
        fs.add_file(format!("{DOTFILES}/t.conf"), source);
        fs.add_file(format!("{DOTFILES}/.generated/t.conf"), generated);
        fs.add_file(format!("{DOTFILES}/.staged/t.conf"), staged);
        let config = write_and_load_config(&fs, &make_config_toml(&[("t.conf", None)]));
        // Source has different line count from generated → structural mismatch → skip
        // Actually, both are 1 line, but source has template syntax
        // The hunk should have default=Skip (unsafe), so prompter response 1 = Skip
        let prompter = FakePrompter::new(vec![1]); // Skip
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/t.conf")))
            .unwrap();
        assert_eq!(content, source);
    }

    #[test]
    fn safe_hunk_default_apply() {
        let fs = setup_fs();
        // Non-template, source==generated → safe, default=Apply
        let toml = format!(
            "dotfiles_dir = \"{DOTFILES}\"\nvars = [\"vars.toml\"]\n\n[[files]]\nsrc = \"plain.conf\"\ntemplate = false\n"
        );
        fs.add_file(format!("{DOTFILES}/plain.conf"), "old\n");
        fs.add_file(format!("{DOTFILES}/.generated/plain.conf"), "old\n");
        fs.add_file(format!("{DOTFILES}/.staged/plain.conf"), "new\n");
        let config = write_and_load_config(&fs, &toml);
        let prompter = FakePrompter::new(vec![0]); // Apply (default for safe)
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/plain.conf")))
            .unwrap();
        assert_eq!(content, "new\n");
    }

    #[test]
    fn source_edited_unsafe() {
        let fs = setup_fs();
        // Non-template, source!=generated → unsafe
        let toml = format!(
            "dotfiles_dir = \"{DOTFILES}\"\nvars = [\"vars.toml\"]\n\n[[files]]\nsrc = \"edited.conf\"\ntemplate = false\n"
        );
        fs.add_file(format!("{DOTFILES}/edited.conf"), "independently edited\n");
        fs.add_file(format!("{DOTFILES}/.generated/edited.conf"), "original\n");
        fs.add_file(format!("{DOTFILES}/.staged/edited.conf"), "staged change\n");
        let config = write_and_load_config(&fs, &toml);
        // Default for unsafe is Skip (index 1)
        let prompter = FakePrompter::new(vec![1]); // Skip
        run(&config, None, false, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/edited.conf")))
            .unwrap();
        assert_eq!(content, "independently edited\n");
    }

    #[test]
    fn structural_mismatch_skips() {
        let fs = setup_fs();
        // Template file where source and generated have different line counts
        let source = "{{ if true }}\nline1\n{{ endif }}\n";
        let generated = "line1\n";
        let staged = "line1\nline2\n";
        fs.add_file(format!("{DOTFILES}/tmpl.conf"), source);
        fs.add_file(format!("{DOTFILES}/.generated/tmpl.conf"), generated);
        fs.add_file(format!("{DOTFILES}/.staged/tmpl.conf"), staged);
        let config = write_and_load_config(&fs, &make_config_toml(&[("tmpl.conf", None)]));
        let prompter = FakePrompter::new(vec![]); // No prompts expected
        run(&config, None, false, &fs, &prompter).unwrap();
        // Source should be unchanged
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/tmpl.conf")))
            .unwrap();
        assert_eq!(content, source);
    }

    #[test]
    fn preserves_permissions() {
        let fs = setup_fs();
        fs.add_file_with_mode(format!("{DOTFILES}/a.conf"), "old\n", 0o755);
        fs.add_file(format!("{DOTFILES}/.generated/a.conf"), "old\n");
        fs.add_file(format!("{DOTFILES}/.staged/a.conf"), "new\n");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let prompter = FakePrompter::new(vec![0]); // Apply
        run(&config, None, false, &fs, &prompter).unwrap();
        let mode = fs
            .file_mode(std::path::Path::new(&format!("{DOTFILES}/a.conf")))
            .unwrap();
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn dry_run() {
        let fs = setup_fs();
        let config = sync_setup(&fs, "old\n", "old\n", "new\n");
        let prompter = FakePrompter::new(vec![]); // No prompts in dry run
        run(&config, None, true, &fs, &prompter).unwrap();
        let content = fs
            .read_to_string(std::path::Path::new(&format!("{DOTFILES}/a.conf")))
            .unwrap();
        assert_eq!(content, "old\n"); // Unchanged
    }

    #[test]
    fn test_has_tera_syntax() {
        assert!(has_tera_syntax("{{ var }}"));
        assert!(has_tera_syntax("{% if true %}"));
        assert!(has_tera_syntax("{# comment #}"));
        assert!(!has_tera_syntax("no template here"));
        assert!(!has_tera_syntax("just { braces }"));
    }

    #[test]
    fn test_classify_hunk_safe() {
        let source = vec!["line\n"];
        let gen_lines = vec!["line\n"];
        let template_affected = HashSet::new();
        let result = classify_hunk(&source, &gen_lines, 0, 1, &template_affected, false);
        assert!(result.is_safe);
        assert!(result.annotation.is_none());
    }

    #[test]
    fn test_classify_hunk_template() {
        let source = vec!["{{ var }}\n"];
        let gen_lines = vec!["rendered\n"];
        let mut template_affected = HashSet::new();
        template_affected.insert(0);
        let result = classify_hunk(&source, &gen_lines, 0, 1, &template_affected, true);
        assert!(!result.is_safe);
        assert!(result.annotation.unwrap().contains("Template syntax"));
    }

    #[test]
    fn test_classify_hunk_edited() {
        let source = vec!["edited\n"];
        let gen_lines = vec!["original\n"];
        let template_affected = HashSet::new();
        let result = classify_hunk(&source, &gen_lines, 0, 1, &template_affected, false);
        assert!(!result.is_safe);
        assert!(result.annotation.unwrap().contains("independently edited"));
    }
}

#[allow(clippy::too_many_arguments)]
fn print_replace_hunk(
    src: &str,
    hunk_num: usize,
    total: usize,
    old_index: usize,
    old_len: usize,
    source_range: &[&str],
    staged: &[&str],
    classification: &HunkClassification,
) {
    let label = if classification.is_safe {
        "Current"
    } else {
        "Source"
    };
    println!(
        "\n--- {}: hunk {}/{} (lines {}-{}) ---",
        src,
        hunk_num,
        total,
        old_index + 1,
        old_index + old_len
    );
    println!("\n  {}:", label);
    for line in source_range {
        print!("    {}", line);
        if !line.ends_with('\n') {
            println!();
        }
    }
    println!("\n  Staged:");
    for line in staged {
        print!("    {}", line);
        if !line.ends_with('\n') {
            println!();
        }
    }
    if let Some(annotation) = classification.annotation {
        println!("\n  {}", annotation);
    }
    println!();
}
