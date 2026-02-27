use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};

pub mod app;
pub mod checkpoint;
pub mod config;
pub mod core;
pub mod logger;
pub mod loop_engine;

use self::app::{
    print_status, resume, run, validate_prd, PrdResumeOptions, PrdRunOptions, PrdRunOutput,
};
use crate::provider::ProviderSelection;

#[derive(Debug, Clone, Copy)]
pub struct PluginDispatchContext {
    pub default_provider: ProviderSelection,
}

#[derive(Debug, Parser)]
#[command(name = "prd-runner")]
struct PrdRunnerCli {
    #[command(subcommand)]
    command: PrdRunnerCommand,
}

#[derive(Debug, Subcommand)]
enum PrdRunnerCommand {
    /// Run PRD-driven coding loop from PRD.md
    Run(RunArgs),
    /// Resume PRD loop from last or specified checkpoint
    Resume(ResumeArgs),
    /// Show PRD checkpoint status
    Status(StatusArgs),
    /// Validate PRD.md format
    Validate,
    /// Initialize PRD.md template if not exists
    Init,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long)]
    provider: Option<String>,
    #[arg(long, default_value = "10m")]
    max_runtime: String,
    #[arg(long)]
    provider_timeout: Option<String>,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct ResumeArgs {
    #[arg(long)]
    provider: Option<String>,
    #[arg(long, default_value = "10m")]
    max_runtime: String,
    #[arg(long)]
    provider_timeout: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    checkpoint_id: Option<String>,
}

#[derive(Debug, Args)]
struct StatusArgs {
    #[arg(long)]
    run_id: Option<String>,
}

pub fn execute_from_tokens(
    workdir: &Path,
    tokens: &[String],
    context: PluginDispatchContext,
) -> Result<()> {
    let mut argv = vec!["prd-runner".to_string()];
    argv.extend(tokens.iter().cloned());
    let cli = PrdRunnerCli::try_parse_from(argv)
        .map_err(|err| anyhow::anyhow!("{}", err.render().ansi()))?;

    match cli.command {
        PrdRunnerCommand::Run(args) => {
            let options = PrdRunOptions {
                provider: resolve_provider_option(args.provider.as_deref(), context)?,
                max_runtime: args.max_runtime,
                provider_timeout: args.provider_timeout,
                dry_run: args.dry_run,
            };
            let output = run(workdir, options)?;
            print_run_output(output);
        }
        PrdRunnerCommand::Resume(args) => {
            let options = PrdResumeOptions {
                provider: resolve_provider_option(args.provider.as_deref(), context)?,
                max_runtime: args.max_runtime,
                provider_timeout: args.provider_timeout,
                dry_run: args.dry_run,
                run_id: args.run_id,
                checkpoint_id: args.checkpoint_id,
            };
            let output = resume(workdir, options)?;
            print_run_output(output);
        }
        PrdRunnerCommand::Status(args) => {
            print_status(workdir, args.run_id.as_deref())?;
        }
        PrdRunnerCommand::Validate => {
            validate_prd(workdir)?;
        }
        PrdRunnerCommand::Init => {
            init_prd_template(workdir)?;
        }
    }

    Ok(())
}

fn resolve_provider_option(
    input: Option<&str>,
    context: PluginDispatchContext,
) -> Result<ProviderSelection> {
    let Some(raw) = input else {
        return Ok(context.default_provider);
    };

    ProviderSelection::parse(raw)
        .with_context(|| format!("invalid provider '{}', expected auto|claude|opencode", raw))
}

fn print_run_output(output: PrdRunOutput) {
    println!("Run finished.");
    println!("- completed: {}", output.summary.completed);
    println!("- iterations: {}", output.summary.iterations);
    if let Some(reason) = output.summary.stop_reason {
        println!("- stop reason: {}", reason);
    }
    if let Some(path) = output.summary.last_checkpoint {
        println!("- last checkpoint: {}", path.display());
    }
    println!("- runtime files: {}", output.log_root.display());
}

fn init_prd_template(workdir: &Path) -> Result<()> {
    let prd_path = workdir.join("PRD.md");
    if prd_path.exists() {
        bail!("PRD.md already exists at {}", prd_path.display());
    }

    let created_date = Utc::now().format("%Y-%m-%d").to_string();
    let template = format!(
        r#"# PRD: Project Name
> 项目名称：project-name
> 创建日期：{}

## 1. 项目上下文

- **项目类型**: Rust CLI / Web / Service
- **目标**: 简述本项目目标

## 2. 需求列表

### REQ-001: 最小可运行版本

| 字段 | 值 |
|------|-----|
| **优先级** | high |
| **描述** | 搭建基础结构并可构建 |
| **验证命令** | `cargo test` |
| **通过条件** | 退出码 = 0 |

- [ ] 初始化项目结构
- [ ] 添加最小测试

## 3. 验收标准

| 标准 | 验证命令 | 通过条件 |
|------|----------|----------|
| 工程可构建 | `cargo build` | 退出码 = 0 |
| 全部测试通过 | `cargo test` | 退出码 = 0 |
"#,
        created_date
    );

    std::fs::write(&prd_path, template)
        .with_context(|| format!("failed to write {}", prd_path.display()))?;
    println!("Created {}", prd_path.display());
    Ok(())
}
