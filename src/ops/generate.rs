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
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tera::Tera;
use tracing::{debug, info, trace, warn};

use crate::config::{Config, FileEntry};
use crate::secrets::{self, SecretEntry, SecretResolver};

/// Load template variables from one or more TOML files in the dotfiles directory.
///
/// Later files override earlier ones. Missing files are silently skipped.
fn load_vars(dotfiles_dir: &Path, var_files: &[String]) -> Result<HashMap<String, toml::Value>> {
    let mut vars = HashMap::new();
    for var_file in var_files {
        let path = dotfiles_dir.join(var_file);
        if !path.exists() {
            debug!("Vars file not found, skipping: {}", path.display());
            continue;
        }
        debug!("Loading vars from {}", path.display());
        let contents = std::fs::read_to_string(&path)
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
pub fn run(config: &Config, files: Option<&[String]>, dry_run: bool) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        config.bail_unmatched(files)?;
        info!("No files to generate");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir();
    let generated_dir = config.generated_dir();

    // Load global vars
    let global_vars = load_vars(&dotfiles_dir, &config.vars)?;

    // Parse global secret entries (cheap TOML reads, no op calls yet)
    let global_secret_entries = secrets::parse_secret_files(&dotfiles_dir, &config.secrets)?;

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
) -> Result<()> {
    let src_path = dotfiles_dir.join(&entry.src);
    let dest_path = generated_dir.join(&entry.src);

    if !src_path.exists() {
        anyhow::bail!("Source file not found: {}", src_path.display());
    }

    if dry_run {
        info!("[dry-run] Would generate: {}", entry.src);
        return Ok(());
    }

    // Ensure parent directory exists
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if entry.template {
        // Look up matching filesets for this file
        let matching_filesets = config.matching_filesets(&entry.src);

        // Build vars: global → fileset → per-file (later wins)
        let mut vars = global_vars.clone();
        for fileset in &matching_filesets {
            if !fileset.vars.is_empty() {
                let fileset_vars = load_vars(dotfiles_dir, &fileset.vars)?;
                vars.extend(fileset_vars);
            }
        }
        if !entry.vars.is_empty() {
            let local_vars = load_vars(dotfiles_dir, &entry.vars)?;
            vars.extend(local_vars);
        }

        // Build secret entries: global → fileset → per-file
        let mut secret_entries: Vec<SecretEntry> = global_secret_entries.to_vec();
        for fileset in &matching_filesets {
            if !fileset.secrets.is_empty() {
                let fileset_secrets = secrets::parse_secret_files(dotfiles_dir, &fileset.secrets)?;
                secret_entries.extend(fileset_secrets);
            }
        }
        if !entry.secrets.is_empty() {
            let file_secrets = secrets::parse_secret_files(dotfiles_dir, &entry.secrets)?;
            secret_entries.extend(file_secrets);
        }

        // Resolve secrets (lazy - only calls op read for uncached references)
        let resolved_secrets = if !secret_entries.is_empty() {
            secrets::resolve_secrets(&secret_entries, resolver)?
        } else {
            HashMap::new()
        };

        // Check for var/secret name collisions
        if !resolved_secrets.is_empty() {
            secrets::check_conflicts(&vars, &resolved_secrets)?;
            vars.extend(resolved_secrets);
        }

        let context = vars_to_tera_context(&vars)?;
        let template_content = std::fs::read_to_string(&src_path)
            .with_context(|| format!("Failed to read template: {}", src_path.display()))?;

        let rendered = Tera::one_off(&template_content, &context, false)
            .with_context(|| format!("Failed to render template: {}", entry.src))?;

        std::fs::write(&dest_path, rendered)
            .with_context(|| format!("Failed to write generated file: {}", dest_path.display()))?;
    } else {
        // Copy as-is
        std::fs::copy(&src_path, &dest_path)
            .with_context(|| format!("Failed to copy file: {}", entry.src))?;
    }

    // Preserve file permissions
    let metadata = std::fs::metadata(&src_path)
        .with_context(|| format!("Failed to read metadata: {}", src_path.display()))?;
    std::fs::set_permissions(
        &dest_path,
        std::fs::Permissions::from_mode(metadata.permissions().mode()),
    )
    .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

    info!("Generated {}", entry.src);
    Ok(())
}
