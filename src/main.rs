use std::path::Path;

use anyhow::{bail, Context, Result};
use auto_code::config::{parser::parse_prd_file, AppConfig};
use auto_code::loop_engine::driver::{EngineRuntime, RunSummary};
use auto_code::runtime::signal::install_ctrlc_handler;
use chrono::Utc;
use clap::{Parser, ValueEnum};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "autocode")]
#[command(about = "Run coding loop in current project directory using PRD.md")]
struct Cli {
    /// Provider to run the coding loop with
    provider: ProviderKind,

    /// Maximum runtime for endless evolution loop (e.g. 4h, 30m, 1d)
    #[arg(long, default_value = "4h")]
    max_runtime: String,

    /// Simulate run without executing commands
    #[arg(long)]
    dry_run: bool,

    /// Timeout for each provider response (e.g. 120s, 2m)
    #[arg(long)]
    provider_timeout: Option<String>,

    /// Enable verbose logging
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderKind {
    Claude,
    Opencode,
}

impl ProviderKind {
    fn command(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Opencode => "opencode",
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose)?;
    install_ctrlc_handler()?;

    let workdir = std::env::current_dir().context("failed to resolve current directory")?;
    if !workdir.exists() {
        bail!("workdir does not exist: {}", workdir.display());
    }

    let prd_path = workdir.join("PRD.md");
    if !prd_path.exists() {
        bail!(
            "missing PRD.md in {}. put your PRD at project root and run ./autocode again",
            workdir.display()
        );
    }
    let prd = parse_prd_file(&prd_path)?;

    let mut config = AppConfig::default();
    config.convergence.max_runtime = cli.max_runtime.clone();
    if let Some(timeout) = &cli.provider_timeout {
        config.provider.response_timeout = timeout.clone();
    }

    let provider_timeout = config
        .provider
        .response_timeout_duration()
        .context("failed to parse provider response timeout")?;
    let log_root = workdir.join(".autocode");
    let log_dir = log_root.join("logs");
    let run_id = Utc::now().format("run_%Y%m%d_%H%M%S").to_string();
    let checkpoint_root = log_root.join("checkpoints").join(run_id);

    let runtime = EngineRuntime {
        prd,
        prd_path,
        config,
        workdir,
        log_dir,
        checkpoint_root,
        provider_command: cli.provider.command().to_string(),
        provider_timeout,
        dry_run: cli.dry_run,
    };

    info!(
        provider = %runtime.provider_command,
        workdir = %runtime.workdir.display(),
        max_runtime = %runtime.config.convergence.max_runtime,
        provider_timeout_s = provider_timeout.as_secs(),
        "autocode run starting"
    );

    let summary = runtime.run(None)?;
    print_run_summary(summary, &log_root);

    Ok(())
}

fn init_tracing(verbose: bool) -> Result<()> {
    let filter = if verbose {
        "auto_code=debug,info"
    } else {
        "auto_code=info,warn"
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init()
        .map_err(|err| anyhow::anyhow!("failed to initialize logger: {}", err))
}

fn print_run_summary(summary: RunSummary, log_root: &Path) {
    println!("Run finished.");
    println!("- completed: {}", summary.completed);
    println!("- iterations: {}", summary.iterations);
    if let Some(reason) = summary.stop_reason {
        println!("- stop reason: {}", reason);
    }
    if let Some(path) = summary.last_checkpoint {
        println!("- last checkpoint: {}", path.display());
    }
    println!("- runtime files: {}", log_root.display());
}
