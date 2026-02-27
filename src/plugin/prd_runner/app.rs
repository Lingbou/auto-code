use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;

use crate::plugin::prd_runner::config::parser::parse_prd_file;
use crate::plugin::prd_runner::config::AppConfig;
use crate::plugin::prd_runner::loop_engine::driver::{EngineRuntime, RunSummary};
use crate::plugin::prd_runner::loop_engine::state::EngineState;
use crate::provider::{resolve_provider, ProviderSelection};

#[derive(Debug, Clone)]
pub struct PrdRunOptions {
    pub provider: ProviderSelection,
    pub max_runtime: String,
    pub provider_timeout: Option<String>,
    pub dry_run: bool,
}

impl Default for PrdRunOptions {
    fn default() -> Self {
        Self {
            provider: ProviderSelection::Auto,
            max_runtime: "10m".to_string(),
            provider_timeout: None,
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PrdResumeOptions {
    pub provider: ProviderSelection,
    pub max_runtime: String,
    pub provider_timeout: Option<String>,
    pub dry_run: bool,
    pub run_id: Option<String>,
    pub checkpoint_id: Option<String>,
}

impl Default for PrdResumeOptions {
    fn default() -> Self {
        Self {
            provider: ProviderSelection::Auto,
            max_runtime: "10m".to_string(),
            provider_timeout: None,
            dry_run: false,
            run_id: None,
            checkpoint_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PrdRunOutput {
    pub summary: RunSummary,
    pub log_root: PathBuf,
}

pub fn run(workdir: &Path, options: PrdRunOptions) -> Result<PrdRunOutput> {
    let prd_path = workdir.join("PRD.md");
    if !prd_path.exists() {
        bail!(
            "missing PRD.md in {}. put your PRD at project root and rerun",
            workdir.display()
        );
    }

    let prd = parse_prd_file(&prd_path)?;
    let mut config = AppConfig::default();
    config.convergence.max_runtime = options.max_runtime;
    if let Some(timeout) = options.provider_timeout {
        config.provider.response_timeout = timeout;
    }

    let provider_timeout = config
        .provider
        .response_timeout_duration()
        .context("failed to parse provider response timeout")?;
    let provider_kind = resolve_provider(options.provider)?;
    let provider_command = provider_kind.command().to_string();
    let log_root = workdir.join(".autocode");
    let log_dir = log_root.join("logs");
    let run_id = Utc::now().format("run_%Y%m%d_%H%M%S").to_string();
    let checkpoint_root = log_root.join("checkpoints").join(run_id);

    let runtime = EngineRuntime {
        prd,
        prd_path,
        config,
        workdir: workdir.to_path_buf(),
        log_dir,
        checkpoint_root,
        provider_command,
        provider_timeout,
        dry_run: options.dry_run,
    };

    let summary = runtime.run(None)?;
    Ok(PrdRunOutput { summary, log_root })
}

pub fn resume(workdir: &Path, options: PrdResumeOptions) -> Result<PrdRunOutput> {
    let prd_path = workdir.join("PRD.md");
    if !prd_path.exists() {
        bail!(
            "missing PRD.md in {}. put your PRD at project root and rerun",
            workdir.display()
        );
    }

    let prd = parse_prd_file(&prd_path)?;
    let mut config = AppConfig::default();
    config.convergence.max_runtime = options.max_runtime;
    if let Some(timeout) = options.provider_timeout {
        config.provider.response_timeout = timeout;
    }

    let provider_timeout = config
        .provider
        .response_timeout_duration()
        .context("failed to parse provider response timeout")?;
    let provider_kind = resolve_provider(options.provider)?;
    let provider_command = provider_kind.command().to_string();

    let checkpoint_root_all = workdir.join(".autocode").join("checkpoints");
    let run_dir = resolve_run_dir(&checkpoint_root_all, options.run_id.as_deref())?;
    let state_path = resolve_checkpoint_state_path(&run_dir, options.checkpoint_id.as_deref())?;
    let state_content = std::fs::read_to_string(&state_path)
        .with_context(|| format!("failed to read {}", state_path.display()))?;
    let resume_state: EngineState = serde_json::from_str(&state_content)
        .with_context(|| format!("failed to parse checkpoint state {}", state_path.display()))?;

    let log_root = workdir.join(".autocode");
    let log_dir = log_root.join("logs");
    let runtime = EngineRuntime {
        prd,
        prd_path,
        config,
        workdir: workdir.to_path_buf(),
        log_dir,
        checkpoint_root: run_dir,
        provider_command,
        provider_timeout,
        dry_run: options.dry_run,
    };

    let summary = runtime.run(Some(resume_state))?;
    Ok(PrdRunOutput { summary, log_root })
}

pub fn validate_prd(workdir: &Path) -> Result<()> {
    let prd_path = workdir.join("PRD.md");
    let prd = parse_prd_file(&prd_path)?;
    println!("PRD is valid.");
    println!(
        "- project: {}",
        prd.project_name.unwrap_or_else(|| "unknown".to_string())
    );
    println!("- requirements: {}", prd.requirements.len());
    println!("- acceptance criteria: {}", prd.acceptance_criteria.len());
    Ok(())
}

pub fn print_status(workdir: &Path, run_id: Option<&str>) -> Result<()> {
    let checkpoint_root_all = workdir.join(".autocode").join("checkpoints");
    if !checkpoint_root_all.exists() {
        println!("No checkpoints found.");
        println!("- expected path: {}", checkpoint_root_all.display());
        return Ok(());
    }

    let run_dir = resolve_run_dir(&checkpoint_root_all, run_id)?;
    let run_name = run_dir
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let checkpoints = list_checkpoint_dirs(&run_dir)?;
    println!("PRD runner status:");
    println!("- run: {}", run_name);
    println!("- checkpoint root: {}", run_dir.display());
    println!("- checkpoint count: {}", checkpoints.len());
    if let Some(last) = checkpoints.last() {
        println!("- latest checkpoint: {}", last.display());
    }

    Ok(())
}

fn resolve_run_dir(root: &Path, run_id: Option<&str>) -> Result<PathBuf> {
    if let Some(run_id) = run_id {
        let path = root.join(run_id);
        if path.exists() {
            return Ok(path);
        }
        bail!("run id not found: {}", run_id);
    }

    let mut runs = std::fs::read_dir(root)
        .with_context(|| format!("failed to read checkpoint root {}", root.display()))?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            Some(path)
        })
        .collect::<Vec<_>>();

    runs.sort();
    runs.pop()
        .ok_or_else(|| anyhow!("no run directories found in {}", root.display()))
}

fn resolve_checkpoint_state_path(run_dir: &Path, checkpoint_id: Option<&str>) -> Result<PathBuf> {
    if let Some(checkpoint_id) = checkpoint_id {
        let state = run_dir.join(checkpoint_id).join("state.json");
        if state.exists() {
            return Ok(state);
        }
        bail!(
            "checkpoint '{}' not found in {}",
            checkpoint_id,
            run_dir.display()
        );
    }

    let checkpoints = list_checkpoint_dirs(run_dir)?;
    let last = checkpoints
        .last()
        .ok_or_else(|| anyhow!("no checkpoints found in {}", run_dir.display()))?;

    Ok(last.join("state.json"))
}

fn list_checkpoint_dirs(run_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut checkpoints = std::fs::read_dir(run_dir)
        .with_context(|| format!("failed to read run dir {}", run_dir.display()))?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy();
            if path.is_dir() && name.starts_with("checkpoint_") {
                return Some(path);
            }
            None
        })
        .collect::<Vec<_>>();

    checkpoints.sort_by_key(|path| checkpoint_order_key(path));
    Ok(checkpoints)
}

fn checkpoint_order_key(path: &Path) -> (u32, String) {
    let name = path
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let num = parse_checkpoint_index(&name).unwrap_or(0);
    (num, name)
}

fn parse_checkpoint_index(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("checkpoint_")?;
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok()
}
