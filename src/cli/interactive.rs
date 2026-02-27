use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::cli::session_store::{OpenSessionOptions, SessionStore, SessionTranscript, StoredRole};
use crate::cli::ui;
use crate::cli::utils::split_command_tokens;
use crate::plugin::prd_runner::core::provider::{CliPrintProvider, Provider};
use crate::plugin::prd_runner::PluginDispatchContext;
use crate::plugin::registry::PluginRegistry;
use crate::provider::{resolve_provider, ProviderKind, ProviderSelection};
use crate::runtime::signal;

const CHAT_TIMEOUT: Duration = Duration::from_secs(600);
const HISTORY_LIMIT: usize = 12;
const WAIT_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];

pub struct InteractiveSession<'a> {
    pub workdir: &'a Path,
    pub default_provider: ProviderSelection,
    pub plugin_registry: &'a PluginRegistry,
    pub session_store: &'a SessionStore,
    pub session_options: OpenSessionOptions<'a>,
    pub initial_prompt: Option<&'a str>,
}

pub fn run(session: InteractiveSession<'_>) -> Result<()> {
    signal::reset_interrupted();
    let mut provider_selection = session.default_provider;
    let mut provider_kind = resolve_provider(provider_selection)?;
    let mut current = session.session_store.open_or_create(
        session.session_options,
        provider_kind.as_str(),
        session.workdir,
    )?;
    if let Some(sel) = ProviderSelection::parse(&current.transcript.provider) {
        provider_selection = sel;
        provider_kind = resolve_provider(provider_selection)?;
    }
    let mut provider = start_provider(provider_kind, session.workdir)?;
    let mut history = history_from_transcript(&current.transcript);

    ui::clear_screen();
    ui::print_header(session.workdir, provider_kind.as_str());
    ui::print_system("type /help for commands");
    ui::print_system("type /exit to quit");
    ui::print_system(&format!("session: {}", current.id));
    session
        .session_store
        .set_provider(&current.id, provider_kind.as_str())?;

    if let Some(initial_prompt) = session.initial_prompt {
        submit_chat_prompt(
            &mut provider,
            provider_kind,
            &mut history,
            session.session_store,
            &current.id,
            initial_prompt,
        )?;
    }

    loop {
        if signal::interrupted() {
            ui::print_system("interrupted");
            break;
        }

        ui::print_prompt(provider_kind.as_str());

        let mut input = String::new();
        let read = io::stdin()
            .read_line(&mut input)
            .context("failed to read interactive input")?;
        if read == 0 {
            println!();
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if input == "/exit" || input == "/quit" {
            break;
        }

        if input == "/help" {
            ui::print_help();
            continue;
        }

        if input == "/session" {
            ui::print_system(&format!(
                "session={} provider={} messages={}",
                current.id,
                provider_kind.as_str(),
                history.len()
            ));
            continue;
        }

        if input == "/sessions" {
            let recent = session.session_store.list_recent(10)?;
            if recent.is_empty() {
                ui::print_system("no sessions");
                continue;
            }
            for item in recent {
                ui::print_system(&format!(
                    "{}  {}  {}  {}",
                    item.id,
                    item.provider,
                    item.updated_at.format("%Y-%m-%d %H:%M:%S"),
                    item.title
                ));
            }
            continue;
        }

        if input == "/resume" || input.starts_with("/resume ") {
            let requested = input.strip_prefix("/resume ").map(str::trim);
            let loaded = if let Some(id) = requested {
                if id.is_empty() {
                    session.session_store.open_or_create(
                        OpenSessionOptions {
                            continue_last: true,
                            session_id: None,
                        },
                        provider_kind.as_str(),
                        session.workdir,
                    )?
                } else {
                    session.session_store.load(id)?
                }
            } else {
                session.session_store.open_or_create(
                    OpenSessionOptions {
                        continue_last: true,
                        session_id: None,
                    },
                    provider_kind.as_str(),
                    session.workdir,
                )?
            };

            if let Some(sel) = ProviderSelection::parse(&loaded.transcript.provider) {
                provider_selection = sel;
                provider_kind = resolve_provider(provider_selection)?;
                provider = start_provider(provider_kind, session.workdir)?;
            }
            current = loaded;
            history = history_from_transcript(&current.transcript);
            ui::print_system(&format!(
                "resumed session {} (provider={})",
                current.id,
                provider_kind.as_str()
            ));
            continue;
        }

        if input == "/clear" {
            ui::clear_screen();
            ui::print_header(session.workdir, provider_kind.as_str());
            continue;
        }

        if let Some(rest) = input.strip_prefix("/provider ") {
            let value = rest.trim();
            let Some(selection) = ProviderSelection::parse(value) else {
                ui::print_error("invalid provider, expected auto|claude|opencode");
                continue;
            };

            provider_selection = selection;
            provider_kind = resolve_provider(provider_selection)?;
            provider = start_provider(provider_kind, session.workdir)?;
            session
                .session_store
                .set_provider(&current.id, provider_kind.as_str())?;
            ui::print_system(&format!("provider switched to {}", provider_kind.as_str()));
            continue;
        }

        if let Some(rest) = input.strip_prefix("/plugin ") {
            let tokens = match split_command_tokens(rest) {
                Ok(tokens) => tokens,
                Err(err) => {
                    ui::print_error(&err);
                    continue;
                }
            };
            if tokens.len() < 2 {
                ui::print_warn("usage: /plugin <id> <cmd> [args...]");
                continue;
            }
            let plugin_id = &tokens[0];
            let args = tokens[1..].to_vec();
            let context = PluginDispatchContext {
                default_provider: provider_selection,
            };
            if let Err(err) =
                session
                    .plugin_registry
                    .execute(session.workdir, plugin_id, &args, context)
            {
                ui::print_error(&format!("plugin error: {}", err));
            }
            continue;
        }

        if let Some(rest) = input.strip_prefix("/prd ") {
            let tokens = match split_command_tokens(rest) {
                Ok(tokens) => tokens,
                Err(err) => {
                    ui::print_error(&err);
                    continue;
                }
            };
            if tokens.is_empty() {
                ui::print_warn("usage: /prd <cmd> [args...]");
                continue;
            }
            let context = PluginDispatchContext {
                default_provider: provider_selection,
            };
            if let Err(err) =
                session
                    .plugin_registry
                    .execute(session.workdir, "prd-runner", &tokens, context)
            {
                ui::print_error(&format!("prd error: {}", err));
            }
            continue;
        }

        submit_chat_prompt(
            &mut provider,
            provider_kind,
            &mut history,
            session.session_store,
            &current.id,
            input,
        )?;
    }

    ui::print_exit();
    Ok(())
}

fn submit_chat_prompt(
    provider: &mut CliPrintProvider,
    provider_kind: ProviderKind,
    history: &mut Vec<(String, String)>,
    session_store: &SessionStore,
    session_id: &str,
    input: &str,
) -> Result<()> {
    let prompt = build_prompt(history, input);
    provider
        .send(&prompt)
        .context("failed to send prompt to provider")?;

    session_store.append_message(session_id, StoredRole::User, input)?;
    let started = Instant::now();
    let output = read_provider_with_wait(provider, CHAT_TIMEOUT, provider_kind.as_str())
        .context("failed to read provider output")?;
    ui::clear_wait();
    println!();
    ui::print_assistant(provider_kind.as_str(), output.trim(), started.elapsed());

    session_store.append_message(session_id, StoredRole::Assistant, output.trim())?;
    history.push(("user".to_string(), input.to_string()));
    history.push(("assistant".to_string(), output.trim().to_string()));
    if history.len() > HISTORY_LIMIT * 2 {
        let drain = history.len().saturating_sub(HISTORY_LIMIT * 2);
        history.drain(0..drain);
    }

    Ok(())
}

fn start_provider(kind: ProviderKind, workdir: &Path) -> Result<CliPrintProvider> {
    let mut provider = CliPrintProvider::new(kind.command().to_string(), workdir);
    provider.start().context("failed to start provider")?;
    Ok(provider)
}

fn read_provider_with_wait(
    provider: &mut CliPrintProvider,
    timeout: Duration,
    provider_name: &str,
) -> Result<String> {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let provider_name = provider_name.to_string();

    let spinner = std::thread::spawn(move || {
        let started = Instant::now();
        let mut frame_index = 0usize;
        loop {
            match done_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(_) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let frame = WAIT_FRAMES[frame_index % WAIT_FRAMES.len()];
                    ui::print_wait(&provider_name, frame, started.elapsed());
                    frame_index = frame_index.saturating_add(1);
                }
            }
        }
    });

    let output = provider.read_output(timeout);
    let _ = done_tx.send(());
    let _ = spinner.join();
    output
}

fn build_prompt(history: &[(String, String)], input: &str) -> String {
    let mut prompt = String::from(
        "You are autocode interactive coding assistant. Respond concisely and with executable guidance when needed.\n\n",
    );
    if !history.is_empty() {
        prompt.push_str("Conversation:\n");
        for (role, text) in history {
            prompt.push_str(&format!("{}: {}\n", role, text));
        }
        prompt.push('\n');
    }
    prompt.push_str(&format!("user: {}\nassistant:", input));
    prompt
}

fn history_from_transcript(transcript: &SessionTranscript) -> Vec<(String, String)> {
    transcript
        .messages
        .iter()
        .filter_map(|msg| match msg.role {
            StoredRole::User => Some(("user".to_string(), msg.text.clone())),
            StoredRole::Assistant => Some(("assistant".to_string(), msg.text.clone())),
            _ => None,
        })
        .collect()
}
