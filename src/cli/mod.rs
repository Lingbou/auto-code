mod args;
mod interactive;
mod session_store;
mod tui;
mod ui;
mod utils;

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::CommandFactory;
use serde_json::json;
use tracing::info;

use crate::cli::args::{
    parse_cli, Cli, Command, PluginArgs, ProviderArg, RunArgs, SessionArgs, SessionCommand,
};
use crate::cli::session_store::{OpenSessionOptions, SessionStore};
use crate::plugin::prd_runner::PluginDispatchContext;
use crate::plugin::registry::PluginRegistry;
use crate::provider::{provider_available, ProviderKind, ProviderSelection};
use crate::runtime::signal::install_ctrlc_handler;

pub fn run() -> Result<()> {
    let cli = parse_cli();
    init_tracing(cli.verbose)?;
    install_ctrlc_handler()?;

    let workdir = std::env::current_dir().context("failed to resolve current directory")?;
    let session_store = SessionStore::new(&workdir)?;
    let plugin_registry = PluginRegistry::new();
    let provider = to_provider_selection(cli.provider);
    let initial_prompt = join_prompt_tokens(&cli.prompt);
    let session_options = OpenSessionOptions {
        continue_last: cli.continue_last,
        session_id: cli.session.as_deref(),
    };

    if cli.fork {
        info!("--fork accepted for compatibility (current build does not fork sessions)");
    }

    match cli.command {
        None => {
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                tui::run(tui::TuiSession {
                    workdir: &workdir,
                    default_provider: provider,
                    plugin_registry: &plugin_registry,
                    session_store: &session_store,
                    session_options,
                    initial_prompt: initial_prompt.as_deref(),
                })?;
            } else {
                interactive::run(interactive::InteractiveSession {
                    workdir: &workdir,
                    default_provider: provider,
                    plugin_registry: &plugin_registry,
                    session_store: &session_store,
                    session_options,
                    initial_prompt: initial_prompt.as_deref(),
                })?;
            }
        }
        Some(Command::Run(args)) => {
            run_alias_prd(&plugin_registry, &workdir, provider, args)?;
        }
        Some(Command::Plugin(args)) => {
            dispatch_plugin_tokens(&plugin_registry, &workdir, provider, args)?;
        }
        Some(Command::Prd(args)) => {
            let context = PluginDispatchContext {
                default_provider: provider,
            };
            plugin_registry.execute(&workdir, "prd-runner", &args.tokens, context)?;
        }
        Some(Command::Doctor) => {
            run_doctor(&workdir)?;
        }
        Some(Command::Session(args)) => {
            run_session_command(&session_store, args)?;
        }
    }

    Ok(())
}

fn run_alias_prd(
    plugin_registry: &PluginRegistry,
    workdir: &Path,
    provider: ProviderSelection,
    args: RunArgs,
) -> Result<()> {
    let mut tokens = vec![
        "run".to_string(),
        "--max-runtime".to_string(),
        args.max_runtime,
    ];
    if let Some(timeout) = args.provider_timeout {
        tokens.push("--provider-timeout".to_string());
        tokens.push(timeout);
    }
    if args.dry_run {
        tokens.push("--dry-run".to_string());
    }

    let context = PluginDispatchContext {
        default_provider: provider,
    };
    plugin_registry.execute(workdir, "prd-runner", &tokens, context)
}

fn dispatch_plugin_tokens(
    plugin_registry: &PluginRegistry,
    workdir: &Path,
    provider: ProviderSelection,
    args: PluginArgs,
) -> Result<()> {
    if args.tokens.is_empty() {
        print_plugin_help();
        return Ok(());
    }

    if args.tokens[0] == "list" {
        print_plugin_list(plugin_registry);
        return Ok(());
    }

    if args.tokens.len() < 2 {
        bail!(
            "usage: autocode plugin <id> <command> [args...]\nrun `autocode plugin list` to inspect available plugins"
        );
    }

    let plugin_id = &args.tokens[0];
    let tokens = args.tokens[1..].to_vec();
    let context = PluginDispatchContext {
        default_provider: provider,
    };
    plugin_registry.execute(workdir, plugin_id, &tokens, context)
}

fn print_plugin_help() {
    println!("usage:");
    println!("- autocode plugin list");
    println!("- autocode plugin <id> <command> [args...]");
    println!("- autocode prd <command> [args...]  # built-in alias");
}

fn print_plugin_list(registry: &PluginRegistry) {
    println!("Available plugins:");
    for plugin in registry.list() {
        let aliases = if plugin.aliases.is_empty() {
            String::new()
        } else {
            format!(" aliases={}", plugin.aliases.join(","))
        };
        println!("- {}: {}{}", plugin.id, plugin.description, aliases);
    }
}

fn run_doctor(workdir: &Path) -> Result<()> {
    println!("autocode doctor");
    println!("- cwd: {}", workdir.display());
    println!("- prd: {}", workdir.join("PRD.md").exists());

    for provider in [ProviderKind::Claude, ProviderKind::Opencode] {
        println!(
            "- provider {} available: {}",
            provider.as_str(),
            provider_available(provider)
        );
    }

    let checkpoint_root = workdir.join(".autocode").join("checkpoints");
    println!("- checkpoint root: {}", checkpoint_root.display());
    println!("- checkpoint root exists: {}", checkpoint_root.exists());

    info!("doctor completed");
    Ok(())
}

fn run_session_command(session_store: &SessionStore, args: SessionArgs) -> Result<()> {
    match args.command {
        SessionCommand::List(list) => {
            let sessions = session_store.list_recent(list.max_count)?;
            let format = list.format.to_ascii_lowercase();
            if format == "json" {
                let value = sessions
                    .into_iter()
                    .map(|s| {
                        json!({
                            "id": s.id,
                            "title": s.title,
                            "provider": s.provider,
                            "created_at": s.created_at,
                            "updated_at": s.updated_at,
                            "message_count": s.message_count,
                        })
                    })
                    .collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&value)?);
                return Ok(());
            }

            if format != "table" {
                bail!(
                    "unsupported --format '{}', expected table|json",
                    list.format
                );
            }

            if sessions.is_empty() {
                println!("No sessions found.");
                return Ok(());
            }

            println!(
                "{:<32}  {:<7}  {:<6}  {:<19}  Title",
                "Session ID", "Provider", "Msgs", "Updated"
            );
            println!("{}", "-".repeat(92));
            for s in sessions {
                println!(
                    "{:<32}  {:<7}  {:<6}  {:<19}  {}",
                    s.id,
                    s.provider,
                    s.message_count,
                    s.updated_at.format("%Y-%m-%d %H:%M:%S"),
                    s.title
                );
            }
        }
        SessionCommand::Delete(delete) => {
            session_store.delete(&delete.session_id)?;
            println!("Deleted session {}", delete.session_id);
        }
    }
    Ok(())
}

fn init_tracing(verbose: bool) -> Result<()> {
    let filter = if verbose {
        "auto_code=debug"
    } else {
        "auto_code=warn"
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init()
        .map_err(|err| anyhow::anyhow!("failed to initialize logger: {}", err))
}

fn join_prompt_tokens(tokens: &[String]) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }
    let joined = tokens.join(" ");
    if joined.trim().is_empty() {
        return None;
    }
    Some(joined)
}

fn to_provider_selection(input: ProviderArg) -> ProviderSelection {
    match input {
        ProviderArg::Auto => ProviderSelection::Auto,
        ProviderArg::Claude => ProviderSelection::Claude,
        ProviderArg::Opencode => ProviderSelection::Opencode,
    }
}

#[allow(dead_code)]
fn print_cli_help() {
    let mut command = Cli::command();
    let _ = command.print_help();
    println!();
}
