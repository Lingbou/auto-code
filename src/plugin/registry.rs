use std::path::Path;

use anyhow::{bail, Result};

use crate::plugin::prd_runner::{self, PluginDispatchContext};

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: &'static str,
    pub description: &'static str,
    pub aliases: &'static [&'static str],
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PluginRegistry;

impl PluginRegistry {
    pub fn new() -> Self {
        Self
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        vec![PluginInfo {
            id: "prd-runner",
            description: "PRD-driven autonomous coding loop (legacy engine preserved as plugin)",
            aliases: &["prd"],
        }]
    }

    pub fn execute(
        &self,
        workdir: &Path,
        plugin_id: &str,
        args: &[String],
        context: PluginDispatchContext,
    ) -> Result<()> {
        match plugin_id {
            "prd-runner" | "prd" => prd_runner::execute_from_tokens(workdir, args, context),
            _ => bail!("unknown plugin '{}'. run `autocode plugin list`", plugin_id),
        }
    }
}
