use anyhow::{Context, Result};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tera::Tera;
use tracing::{debug, info, trace, warn};

use crate::config::{Config, FileEntry};

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
        let table: HashMap<String, toml::Value> =
            toml::from_str(&contents).with_context(|| format!("Failed to parse vars file: {}", path.display()))?;
        vars.extend(table);
    }
    Ok(vars)
}

fn vars_to_tera_context(vars: &HashMap<String, toml::Value>) -> Result<tera::Context> {
    let mut context = tera::Context::new();
    for (key, value) in vars {
        let json_value = toml_value_to_json(value);
        context.insert(key, &json_value);
    }
    trace!("Tera context: {:?}", context);
    Ok(context)
}

fn toml_value_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(i),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(table) => {
            let map = table
                .iter()
                .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

pub fn run(config: &Config, files: Option<&[String]>, dry_run: bool) -> Result<()> {
    let entries = config.filter_files(files);
    if entries.is_empty() {
        info!("No files to generate");
        return Ok(());
    }

    let dotfiles_dir = config.dotfiles_dir();
    let generated_dir = config.generated_dir();

    // Load global vars
    let global_vars = load_vars(&dotfiles_dir, &config.vars)?;

    let mut errors: Vec<(String, anyhow::Error)> = Vec::new();
    let mut succeeded = 0usize;

    for entry in &entries {
        match generate_file(config, entry, &dotfiles_dir, &generated_dir, &global_vars, dry_run) {
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
        info!("Generated {} file(s) with {} failure(s)", succeeded, errors.len());
        let mut msg = format!("Failed to generate {} file(s):", errors.len());
        for (src, e) in &errors {
            msg.push_str(&format!("\n  {src}: {e:#}"));
        }
        anyhow::bail!(msg);
    }

    Ok(())
}

fn generate_file(
    _config: &Config,
    entry: &FileEntry,
    dotfiles_dir: &Path,
    generated_dir: &Path,
    global_vars: &HashMap<String, toml::Value>,
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
        // Merge global vars with file-local vars (file-local wins)
        let mut vars = global_vars.clone();
        if !entry.vars.is_empty() {
            let local_vars = load_vars(dotfiles_dir, &entry.vars)?;
            vars.extend(local_vars);
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
    std::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(metadata.permissions().mode()))
        .with_context(|| format!("Failed to set permissions: {}", dest_path.display()))?;

    info!("Generated {}", entry.src);
    Ok(())
}
