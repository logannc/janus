use anyhow::{bail, Result};
use std::path::Path;
use tracing::info;

use crate::config::Config;
use crate::paths::expand_tilde;
use crate::state::State;

pub struct StatusFilters {
    pub only_diffs: bool,
    pub deployed: bool,
    pub undeployed: bool,
}

#[derive(PartialEq)]
enum DeployState {
    Deployed,
    Undeployed,
}

struct FileStatus {
    src: String,
    deploy_state: DeployState,
    detail: String,
}

pub fn run(config: &Config, files: Option<&[String]>, filters: StatusFilters) -> Result<()> {
    if filters.deployed && filters.undeployed {
        bail!("Cannot specify both --deployed and --undeployed");
    }

    let entries = config.filter_files(files);
    if entries.is_empty() {
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

        let is_deployed = state.is_deployed(src) && is_janus_symlink(&target_path, &staged_path);

        let deploy_state = if is_deployed {
            DeployState::Deployed
        } else {
            DeployState::Undeployed
        };

        let detail = compute_detail(
            &source_path,
            &generated_path,
            &staged_path,
            is_deployed,
        );

        let has_diff = detail.contains("diff") || detail.contains("missing") || detail.contains("not yet");

        // Apply filters
        if filters.deployed && deploy_state != DeployState::Deployed {
            continue;
        }
        if filters.undeployed && deploy_state != DeployState::Undeployed {
            continue;
        }
        if filters.only_diffs && !has_diff {
            continue;
        }

        statuses.push(FileStatus {
            src: src.clone(),
            deploy_state,
            detail,
        });
    }

    if statuses.is_empty() {
        info!("No files match the given filters");
        return Ok(());
    }

    // Find max src width for alignment
    let max_src_len = statuses.iter().map(|s| s.src.len()).max().unwrap_or(0);

    for status in &statuses {
        let state_str = match status.deploy_state {
            DeployState::Deployed => "deployed  ",
            DeployState::Undeployed => "undeployed",
        };

        println!(
            "  {:<width$}  {}  ({})",
            status.src,
            state_str,
            status.detail,
            width = max_src_len,
        );
    }

    Ok(())
}

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

fn files_match(a: &Path, b: &Path) -> bool {
    let Ok(content_a) = std::fs::read(a) else {
        return false;
    };
    let Ok(content_b) = std::fs::read(b) else {
        return false;
    };
    content_a == content_b
}

fn is_janus_symlink(target: &Path, expected_staged: &Path) -> bool {
    if !target.is_symlink() {
        return false;
    }
    match std::fs::read_link(target) {
        Ok(link_dest) => link_dest == expected_staged,
        Err(_) => false,
    }
}
