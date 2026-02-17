//! Render templates and copy source files into `.generated/`.
//!
//! For files with `template = true`, renders the source through Tera with
//! merged global + per-file variables and secrets. For non-template files,
//! copies as-is. Preserves Unix file permissions on all output files.
//!
//! Uses error-collection strategy: processes all files and reports failures
//! at the end rather than bailing on the first error.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use tera::Tera;
use tracing::{debug, info, trace, warn};

use crate::config::{Config, FileEntry};
use crate::platform::{Fs, SecretEngine};
use crate::secrets::{self, SecretEntry, SecretResolver};

/// Load template variables from one or more TOML files in the dotfiles directory.
///
/// Later files override earlier ones. Missing files are silently skipped.
fn load_vars(
    dotfiles_dir: &Path,
    var_files: &[String],
    fs: &impl Fs,
) -> Result<HashMap<String, toml::Value>> {
    let mut vars = HashMap::new();
    for var_file in var_files {
        let path = dotfiles_dir.join(var_file);
        if !fs.exists(&path) {
            debug!("Vars file not found, skipping: {}", path.display());
            continue;
        }
        debug!("Loading vars from {}", path.display());
        let contents = fs
            .read_to_string(&path)
            .with_context(|| format!("Failed to read vars file: {}", path.display()))?;
        let table: HashMap<String, toml::Value> = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse vars file: {}", path.display()))?;
        vars.extend(table);
    }
    Ok(vars)
}

/// Convert a flat map of TOML values into a Tera template context.
fn vars_to_tera_context(vars: &HashMap<String, toml::Value>) -> Result<tera::Context> {
    let mut context = tera::Context::new();
    for (key, value) in vars {
        context.insert(key, value);
    }
    trace!("Tera context: {:?}", context);
    Ok(context)
}

/// Generate output files for the given file patterns (or all files).
///
/// Collects per-file errors and reports them at the end. Returns an error
/// if any file failed to generate.
pub fn run(
    config: &Config,
    files: Option<&[String]>,
    dry_run: bool,
    fs: &impl Fs,
    engine: &impl SecretEngine,
) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to generate");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir(fs);
    let generated_dir = config.generated_dir(fs);

    // Load global vars
    let global_vars = load_vars(&dotfiles_dir, &config.vars, fs)?;

    // Parse global secret entries (cheap TOML reads, no op calls yet)
    let global_secret_entries = secrets::parse_secret_files(&dotfiles_dir, &config.secrets, fs)?;

    // Shared resolver caches op read results across all files
    let mut resolver = SecretResolver::new();

    let mut errors: Vec<(String, anyhow::Error)> = Vec::new();
    let mut succeeded = 0usize;

    for entry in &entries {
        match generate_file(
            config,
            entry,
            &dotfiles_dir,
            &generated_dir,
            &global_vars,
            &global_secret_entries,
            &mut resolver,
            dry_run,
            fs,
            engine,
        ) {
            Ok(()) => succeeded += 1,
            Err(e) => {
                warn!("Failed to generate {}: {e:#}", entry.src);
                errors.push((entry.src.clone(), e));
            }
        }
    }

    if errors.is_empty() {
        info!("Generated {} file(s)", succeeded);
    } else {
        info!(
            "Generated {} file(s) with {} failure(s)",
            succeeded,
            errors.len()
        );
        let mut msg = format!("Failed to generate {} file(s):", errors.len());
        for (src, e) in &errors {
            msg.push_str(&format!("\n  {src}: {e:#}"));
        }
        anyhow::bail!(msg);
    }

    Ok(())
}

/// Generate a single file: render template or copy, then preserve permissions.
#[allow(clippy::too_many_arguments)]
fn generate_file(
    config: &Config,
    entry: &FileEntry,
    dotfiles_dir: &Path,
    generated_dir: &Path,
    global_vars: &HashMap<String, toml::Value>,
    global_secret_entries: &[SecretEntry],
    resolver: &mut SecretResolver,
    dry_run: bool,
    fs: &impl Fs,
    engine: &impl SecretEngine,
) -> Result<()> {
    let src_path = dotfiles_dir.join(&entry.src);
    let dest_path = generated_dir.join(&entry.src);

    if !fs.exists(&src_path) {
        anyhow::bail!("Source file not found: {}", src_path.display());
    }

    if dry_run {
        info!("[dry-run] Would generate: {}", entry.src);
        return Ok(());
    }

    // Ensure parent directory exists
    if let Some(parent) = dest_path.parent() {
        fs.create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if entry.template {
        // Look up matching filesets for this file
        let matching_filesets = config.matching_filesets(&entry.src);

        // Build vars: global -> fileset -> per-file (later wins)
        let mut vars = global_vars.clone();
        for fileset in &matching_filesets {
            if !fileset.vars.is_empty() {
                let fileset_vars = load_vars(dotfiles_dir, &fileset.vars, fs)?;
                vars.extend(fileset_vars);
            }
        }
        if !entry.vars.is_empty() {
            let local_vars = load_vars(dotfiles_dir, &entry.vars, fs)?;
            vars.extend(local_vars);
        }

        // Build secret entries: global -> fileset -> per-file
        let mut secret_entries: Vec<SecretEntry> = global_secret_entries.to_vec();
        for fileset in &matching_filesets {
            if !fileset.secrets.is_empty() {
                let fileset_secrets =
                    secrets::parse_secret_files(dotfiles_dir, &fileset.secrets, fs)?;
                secret_entries.extend(fileset_secrets);
            }
        }
        if !entry.secrets.is_empty() {
            let file_secrets = secrets::parse_secret_files(dotfiles_dir, &entry.secrets, fs)?;
            secret_entries.extend(file_secrets);
        }

        // Resolve secrets (lazy - only calls op read for uncached references)
        let resolved_secrets = if !secret_entries.is_empty() {
            secrets::resolve_secrets(&secret_entries, resolver, engine)?
        } else {
            HashMap::new()
        };

        // Check for var/secret name collisions
        if !resolved_secrets.is_empty() {
            secrets::check_conflicts(&vars, &resolved_secrets)?;
            vars.extend(resolved_secrets);
        }

        let context = vars_to_tera_context(&vars)?;
        let template_content = fs
            .read_to_string(&src_path)
            .with_context(|| format!("Failed to read template: {}", src_path.display()))?;

        let rendered = Tera::one_off(&template_content, &context, false)
            .with_context(|| format!("Failed to render template: {}", entry.src))?;

        fs.write(&dest_path, rendered.as_bytes())
            .with_context(|| format!("Failed to write generated file: {}", dest_path.display()))?;
    } else {
        // Copy as-is
        fs.copy(&src_path, &dest_path)
            .with_context(|| format!("Failed to copy file: {}", entry.src))?;
    }

    // Preserve file permissions
    let mode = fs
        .file_mode(&src_path)
        .with_context(|| format!("Failed to read metadata: {}", src_path.display()))?;
    fs.set_file_mode(&dest_path, mode)
        .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

    info!("Generated {}", entry.src);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::FakeSecretEngine;
    use crate::test_helpers::*;

    fn make_engine() -> FakeSecretEngine {
        FakeSecretEngine::new()
    }

    #[test]
    fn template_with_vars() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "name = \"world\"");
        fs.add_file(format!("{DOTFILES}/greet.conf"), "Hello {{ name }}!");
        let config = write_and_load_config(&fs, &make_config_toml(&[("greet.conf", None)]));
        run(&config, None, false, &fs, &make_engine()).unwrap();
        let content = fs
            .read_to_string(Path::new(&format!("{DOTFILES}/.generated/greet.conf")))
            .unwrap();
        assert_eq!(content, "Hello world!");
    }

    #[test]
    fn non_template_copy() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        let binary_content = b"\x00\x01\x02\x03";
        fs.add_file(format!("{DOTFILES}/data.bin"), binary_content.to_vec());
        let toml = format!(
            "dotfiles_dir = \"{DOTFILES}\"\nvars = [\"vars.toml\"]\n\n[[files]]\nsrc = \"data.bin\"\ntemplate = false\n"
        );
        let config = write_and_load_config(&fs, &toml);
        run(&config, None, false, &fs, &make_engine()).unwrap();
        let content = fs
            .read(Path::new(&format!("{DOTFILES}/.generated/data.bin")))
            .unwrap();
        assert_eq!(content, binary_content);
    }

    #[test]
    fn preserves_permissions() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file_with_mode(format!("{DOTFILES}/script.sh"), "#!/bin/bash", 0o755);
        let config = write_and_load_config(&fs, &make_config_toml(&[("script.sh", None)]));
        run(&config, None, false, &fs, &make_engine()).unwrap();
        let mode = fs
            .file_mode(Path::new(&format!("{DOTFILES}/.generated/script.sh")))
            .unwrap();
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn creates_parent_dirs() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/hypr/hypr.conf"), "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[("hypr/hypr.conf", None)]));
        run(&config, None, false, &fs, &make_engine()).unwrap();
        assert!(fs.is_dir(Path::new(&format!("{DOTFILES}/.generated/hypr"))));
    }

    #[test]
    fn missing_source_collected() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/good.conf"), "content");
        // "bad.conf" source doesn't exist
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("good.conf", None), ("bad.conf", None)]),
        );
        let result = run(&config, None, false, &fs, &make_engine());
        assert!(result.is_err());
        // good.conf should still have been generated
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/good.conf"))));
    }

    #[test]
    fn missing_vars_file_skipped() {
        let fs = setup_fs();
        // vars.toml doesn't exist but that's OK
        fs.add_file(format!("{DOTFILES}/a.conf"), "plain content");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        run(&config, None, false, &fs, &make_engine()).unwrap();
    }

    #[test]
    fn var_merge_order() {
        let fs = setup_fs();
        // Global var
        fs.add_file(format!("{DOTFILES}/vars.toml"), "val = \"global\"");
        // Fileset var
        fs.add_file(format!("{DOTFILES}/fs-vars.toml"), "val = \"fileset\"");
        // Per-file var
        fs.add_file(format!("{DOTFILES}/file-vars.toml"), "val = \"perfile\"");
        fs.add_file(format!("{DOTFILES}/test.conf"), "{{ val }}");

        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"
vars = ["vars.toml"]

[[files]]
src = "test.conf"
vars = ["file-vars.toml"]

[filesets.all]
patterns = ["test.conf"]
vars = ["fs-vars.toml"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        run(&config, None, false, &fs, &make_engine()).unwrap();
        let content = fs
            .read_to_string(Path::new(&format!("{DOTFILES}/.generated/test.conf")))
            .unwrap();
        assert_eq!(content, "perfile");
    }

    #[test]
    fn secret_injection() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(
            format!("{DOTFILES}/secrets.toml"),
            "[[secret]]\nname = \"db_pass\"\nengine = \"1password\"\nreference = \"op://db/pass\"\n",
        );
        fs.add_file(format!("{DOTFILES}/db.conf"), "password={{ db_pass }}");

        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"
vars = ["vars.toml"]
secrets = ["secrets.toml"]

[[files]]
src = "db.conf"
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://db/pass", "s3cret");
        run(&config, None, false, &fs, &engine).unwrap();
        let content = fs
            .read_to_string(Path::new(&format!("{DOTFILES}/.generated/db.conf")))
            .unwrap();
        assert_eq!(content, "password=s3cret");
    }

    #[test]
    fn secret_var_conflict() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "name = \"conflict\"");
        fs.add_file(
            format!("{DOTFILES}/secrets.toml"),
            "[[secret]]\nname = \"name\"\nengine = \"1password\"\nreference = \"op://x\"\n",
        );
        fs.add_file(format!("{DOTFILES}/test.conf"), "{{ name }}");

        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"
vars = ["vars.toml"]
secrets = ["secrets.toml"]

[[files]]
src = "test.conf"
"#
        );
        let config = write_and_load_config(&fs, &toml);
        let mut engine = FakeSecretEngine::new();
        engine.add_secret("1password", "op://x", "val");
        let result = run(&config, None, false, &fs, &engine);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("collision"), "got: {msg}");
    }

    #[test]
    fn all_files() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/a.conf"), "a");
        fs.add_file(format!("{DOTFILES}/b.conf"), "b");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None)]),
        );
        run(&config, None, false, &fs, &make_engine()).unwrap();
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/a.conf"))));
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/b.conf"))));
    }

    #[test]
    fn filtered_files() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/a.conf"), "a");
        fs.add_file(format!("{DOTFILES}/b.conf"), "b");
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[("a.conf", None), ("b.conf", None)]),
        );
        let patterns = vec!["a.conf".to_string()];
        run(&config, Some(&patterns), false, &fs, &make_engine()).unwrap();
        assert!(fs.exists(Path::new(&format!("{DOTFILES}/.generated/a.conf"))));
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.generated/b.conf"))));
    }

    #[test]
    fn error_collection() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/good.conf"), "ok");
        // missing1 and missing2 don't exist
        let config = write_and_load_config(
            &fs,
            &make_config_toml(&[
                ("good.conf", None),
                ("missing1.conf", None),
                ("missing2.conf", None),
            ]),
        );
        let result = run(&config, None, false, &fs, &make_engine());
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("2 file(s)"), "got: {msg}");
    }

    #[test]
    fn dry_run() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/a.conf"), "content");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        run(&config, None, true, &fs, &make_engine()).unwrap();
        assert!(!fs.exists(Path::new(&format!("{DOTFILES}/.generated/a.conf"))));
    }

    #[test]
    fn fileset_vars_inherited() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/fs-vars.toml"), "color = \"blue\"");
        fs.add_file(format!("{DOTFILES}/test.conf"), "{{ color }}");

        let toml = format!(
            r#"
dotfiles_dir = "{DOTFILES}"
vars = ["vars.toml"]

[[files]]
src = "test.conf"

[filesets.themed]
patterns = ["test.conf"]
vars = ["fs-vars.toml"]
"#
        );
        let config = write_and_load_config(&fs, &toml);
        run(&config, None, false, &fs, &make_engine()).unwrap();
        let content = fs
            .read_to_string(Path::new(&format!("{DOTFILES}/.generated/test.conf")))
            .unwrap();
        assert_eq!(content, "blue");
    }

    #[test]
    fn undefined_variable_errors() {
        let fs = setup_fs();
        fs.add_file(format!("{DOTFILES}/vars.toml"), "");
        fs.add_file(format!("{DOTFILES}/a.conf"), "Hello {{ undefined_name }}!");
        let config = write_and_load_config(&fs, &make_config_toml(&[("a.conf", None)]));
        let result = run(&config, None, false, &fs, &make_engine());
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("undefined_name")
                || msg.contains("not found")
                || msg.contains("Failed to render"),
            "got: {msg}"
        );
    }
}
