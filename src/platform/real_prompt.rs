//! Real prompter implementation using `dialoguer`.

use anyhow::{Context, Result};
use dialoguer::Select;

use super::Prompter;

/// Real prompter — delegates to `dialoguer::Select` for interactive terminal prompts.
pub struct RealPrompter;

impl Prompter for RealPrompter {
    fn select(&self, prompt: &str, items: &[&str], default: usize) -> Result<usize> {
        Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(default)
            .interact()
            .context("Prompt interaction failed")
    }
}
