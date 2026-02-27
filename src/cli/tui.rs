use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};

use crate::cli::session_store::{OpenSessionOptions, SessionStore, SessionTranscript, StoredRole};
use crate::cli::utils::split_command_tokens;
use crate::plugin::prd_runner::core::provider::{CliPrintProvider, Provider};
use crate::plugin::prd_runner::PluginDispatchContext;
use crate::plugin::registry::PluginRegistry;
use crate::provider::{resolve_provider, ProviderKind, ProviderSelection};
use crate::runtime::signal;

const CHAT_TIMEOUT: Duration = Duration::from_secs(600);
const TICK_INTERVAL: Duration = Duration::from_millis(80);
const HISTORY_LIMIT: usize = 12;
const MESSAGE_LIMIT: usize = 400;
const WAIT_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];

#[derive(Debug, Clone, Copy)]
enum MessageRole {
    User,
    Assistant,
    System,
    Error,
}

#[derive(Debug, Clone)]
struct Message {
    role: MessageRole,
    text: String,
}

#[derive(Debug)]
struct PendingResponse {
    started: Instant,
    rx: mpsc::Receiver<std::result::Result<String, String>>,
}

#[derive(Debug, Clone)]
struct RenderLine {
    color: Color,
    text: String,
}

struct App<'a> {
    workdir: &'a Path,
    provider_selection: ProviderSelection,
    provider_kind: ProviderKind,
    provider: Arc<Mutex<CliPrintProvider>>,
    plugin_registry: &'a PluginRegistry,
    session_store: &'a SessionStore,
    session_id: String,
    messages: Vec<Message>,
    input: String,
    pending: Option<PendingResponse>,
    status: String,
    quitting: bool,
}

impl<'a> App<'a> {
    fn push_message(&mut self, role: MessageRole, text: impl Into<String>) {
        self.messages.push(Message {
            role,
            text: text.into(),
        });
        if self.messages.len() > MESSAGE_LIMIT {
            let drop_count = self.messages.len().saturating_sub(MESSAGE_LIMIT);
            self.messages.drain(0..drop_count);
        }
    }

    fn push_system(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.push_message(MessageRole::System, text.clone());
        let _ = self
            .session_store
            .append_message(&self.session_id, StoredRole::System, &text);
    }

    fn push_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.push_message(MessageRole::Error, text.clone());
        let _ = self
            .session_store
            .append_message(&self.session_id, StoredRole::Error, &text);
    }
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable raw mode")?;
        execute!(io::stdout(), EnterAlternateScreen).context("failed to enter alternate screen")?;
        Ok(Self { active: true })
    }

    fn suspend(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        terminal::disable_raw_mode().context("failed to disable raw mode")?;
        execute!(io::stdout(), LeaveAlternateScreen).context("failed to leave alternate screen")?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        execute!(io::stdout(), EnterAlternateScreen).context("failed to enter alternate screen")?;
        terminal::enable_raw_mode().context("failed to enable raw mode")?;
        self.active = true;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
}

pub struct TuiSession<'a> {
    pub workdir: &'a Path,
    pub default_provider: ProviderSelection,
    pub plugin_registry: &'a PluginRegistry,
    pub session_store: &'a SessionStore,
    pub session_options: OpenSessionOptions<'a>,
    pub initial_prompt: Option<&'a str>,
}

pub fn run(session: TuiSession<'_>) -> Result<()> {
    signal::reset_interrupted();
    let mut provider_selection = session.default_provider;
    let mut provider_kind = resolve_provider(provider_selection)?;
    let opened = session.session_store.open_or_create(
        session.session_options,
        provider_kind.as_str(),
        session.workdir,
    )?;
    if let Some(saved_selection) = ProviderSelection::parse(&opened.transcript.provider) {
        provider_selection = saved_selection;
        provider_kind = resolve_provider(provider_selection)?;
    }
    let provider = Arc::new(Mutex::new(start_provider(provider_kind, session.workdir)?));
    let mut app = App {
        workdir: session.workdir,
        provider_selection,
        provider_kind,
        provider,
        plugin_registry: session.plugin_registry,
        session_store: session.session_store,
        session_id: opened.id.clone(),
        messages: transcript_to_messages(&opened.transcript),
        input: String::new(),
        pending: None,
        status: "ready".to_string(),
        quitting: false,
    };
    app.push_system("Welcome to AUTO-CODE TUI. /help for commands.");
    app.push_system(format!("session: {}", app.session_id));

    // Ensure meta provider tracks the currently active backend for this session.
    app.session_store
        .set_provider(&app.session_id, app.provider_kind.as_str())?;

    let mut guard = TerminalGuard::enter()?;
    if let Some(initial_prompt) = session.initial_prompt {
        app.input = initial_prompt.to_string();
        submit_input(&mut app, &mut guard)?;
    }
    loop {
        poll_pending_response(&mut app);
        render(&app)?;

        if app.quitting || signal::interrupted() {
            break;
        }

        if event::poll(TICK_INTERVAL).context("failed to poll terminal event")? {
            let event = event::read().context("failed to read terminal event")?;
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Press {
                    handle_key(key, &mut app, &mut guard)?;
                }
            }
        }
    }

    guard.suspend()?;
    println!("AUTO-CODE exited.");
    Ok(())
}

fn start_provider(kind: ProviderKind, workdir: &Path) -> Result<CliPrintProvider> {
    let mut provider = CliPrintProvider::new(kind.command().to_string(), workdir);
    provider.start().context("failed to start provider")?;
    Ok(provider)
}

fn handle_key(key: KeyEvent, app: &mut App<'_>, guard: &mut TerminalGuard) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        app.quitting = true;
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => submit_input(app, guard)?,
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Esc => {
            app.input.clear();
        }
        KeyCode::Char(ch) => {
            app.input.push(ch);
        }
        _ => {}
    }
    Ok(())
}

fn submit_input(app: &mut App<'_>, guard: &mut TerminalGuard) -> Result<()> {
    let raw_input = std::mem::take(&mut app.input);
    let input = raw_input.trim();
    if input.is_empty() {
        return Ok(());
    }

    if input == "/exit" || input == "/quit" {
        app.quitting = true;
        return Ok(());
    }

    if input == "/help" {
        app.push_system(
            "/help /exit /provider auto|claude|opencode /plugin <id> <cmd> /prd <cmd> /clear /session /sessions /resume [id]",
        );
        return Ok(());
    }

    if input == "/session" {
        app.push_system(format!(
            "session={} provider={} messages={}",
            app.session_id,
            app.provider_kind.as_str(),
            app.messages.len()
        ));
        return Ok(());
    }

    if input == "/sessions" {
        let recent = app.session_store.list_recent(10)?;
        if recent.is_empty() {
            app.push_system("no sessions");
            return Ok(());
        }
        for item in recent {
            app.push_system(format!(
                "{}  {}  {}  {}",
                item.id,
                item.provider,
                item.updated_at.format("%Y-%m-%d %H:%M:%S"),
                item.title
            ));
        }
        return Ok(());
    }

    if input == "/resume" || input.starts_with("/resume ") {
        if app.pending.is_some() {
            app.push_error("provider is busy; wait for current response");
            return Ok(());
        }
        let requested = input.strip_prefix("/resume ").map(str::trim);
        let loaded = if let Some(id) = requested {
            if id.is_empty() {
                app.session_store.open_or_create(
                    OpenSessionOptions {
                        continue_last: true,
                        session_id: None,
                    },
                    app.provider_kind.as_str(),
                    app.workdir,
                )?
            } else {
                app.session_store.load(id)?
            }
        } else {
            app.session_store.open_or_create(
                OpenSessionOptions {
                    continue_last: true,
                    session_id: None,
                },
                app.provider_kind.as_str(),
                app.workdir,
            )?
        };

        if let Some(sel) = ProviderSelection::parse(&loaded.transcript.provider) {
            let kind = resolve_provider(sel)?;
            let provider = Arc::new(Mutex::new(start_provider(kind, app.workdir)?));
            app.provider_selection = sel;
            app.provider_kind = kind;
            app.provider = provider;
        }
        app.session_id = loaded.id.clone();
        app.messages = transcript_to_messages(&loaded.transcript);
        app.push_system(format!(
            "resumed session {} (provider={})",
            app.session_id,
            app.provider_kind.as_str()
        ));
        app.status = "ready".to_string();
        return Ok(());
    }

    if input == "/clear" {
        app.messages.clear();
        app.push_system("history cleared");
        return Ok(());
    }

    if let Some(rest) = input.strip_prefix("/provider ") {
        if app.pending.is_some() {
            app.push_error("provider is busy; wait for current response");
            return Ok(());
        }
        let Some(selection) = ProviderSelection::parse(rest.trim()) else {
            app.push_error("invalid provider, expected auto|claude|opencode");
            return Ok(());
        };
        let kind = resolve_provider(selection)?;
        let provider = Arc::new(Mutex::new(start_provider(kind, app.workdir)?));
        app.provider_selection = selection;
        app.provider_kind = kind;
        app.provider = provider;
        app.session_store
            .set_provider(&app.session_id, app.provider_kind.as_str())?;
        app.push_system(format!("provider switched to {}", kind.as_str()));
        return Ok(());
    }

    if let Some(rest) = input.strip_prefix("/plugin ") {
        let tokens = match split_command_tokens(rest) {
            Ok(tokens) => tokens,
            Err(err) => {
                app.push_error(err);
                return Ok(());
            }
        };
        if tokens.len() < 2 {
            app.push_error("usage: /plugin <id> <cmd> [args...]");
            return Ok(());
        }
        let plugin_id = tokens[0].clone();
        let args = tokens[1..].to_vec();
        run_plugin_command(app, guard, &plugin_id, &args)?;
        return Ok(());
    }

    if let Some(rest) = input.strip_prefix("/prd ") {
        let tokens = match split_command_tokens(rest) {
            Ok(tokens) => tokens,
            Err(err) => {
                app.push_error(err);
                return Ok(());
            }
        };
        if tokens.is_empty() {
            app.push_error("usage: /prd <cmd> [args...]");
            return Ok(());
        }
        run_plugin_command(app, guard, "prd-runner", &tokens)?;
        return Ok(());
    }

    if app.pending.is_some() {
        app.push_error("provider is busy; wait for current response");
        return Ok(());
    }

    app.push_message(MessageRole::User, raw_input.clone());
    app.session_store
        .append_message(&app.session_id, StoredRole::User, &raw_input)?;
    app.status = format!("waiting for {}...", app.provider_kind.as_str());

    let prompt = build_prompt(&app.messages, &raw_input);
    {
        let mut provider = app
            .provider
            .lock()
            .map_err(|_| anyhow::anyhow!("provider state lock poisoned"))?;
        provider
            .send(&prompt)
            .context("failed to send prompt to provider")?;
    }

    let provider = Arc::clone(&app.provider);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = match provider.lock() {
            Ok(mut handle) => handle.read_output(CHAT_TIMEOUT).map_err(|e| e.to_string()),
            Err(_) => Err("provider state lock poisoned".to_string()),
        };
        let _ = tx.send(result);
    });

    app.pending = Some(PendingResponse {
        started: Instant::now(),
        rx,
    });
    Ok(())
}

fn run_plugin_command(
    app: &mut App<'_>,
    guard: &mut TerminalGuard,
    plugin_id: &str,
    args: &[String],
) -> Result<()> {
    if app.pending.is_some() {
        app.push_error("provider is busy; wait for current response");
        return Ok(());
    }

    guard.suspend()?;
    println!(
        "[autocode] running plugin: {} {}",
        plugin_id,
        args.join(" ")
    );
    let context = PluginDispatchContext {
        default_provider: app.provider_selection,
    };
    let result = app
        .plugin_registry
        .execute(app.workdir, plugin_id, args, context);
    match &result {
        Ok(_) => println!("[autocode] plugin finished."),
        Err(err) => println!("[autocode] plugin error: {}", err),
    }
    println!("[autocode] Press Enter to return to TUI...");
    let mut buf = String::new();
    let _ = io::stdin().read_line(&mut buf);
    guard.resume()?;

    match result {
        Ok(_) => app.push_system(format!("plugin {} completed", plugin_id)),
        Err(err) => app.push_error(format!("plugin {} failed: {}", plugin_id, err)),
    }
    signal::reset_interrupted();
    Ok(())
}

fn poll_pending_response(app: &mut App<'_>) {
    let Some(pending) = app.pending.as_ref() else {
        return;
    };

    match pending.rx.try_recv() {
        Ok(Ok(output)) => {
            app.push_message(MessageRole::Assistant, output.trim().to_string());
            let _ = app.session_store.append_message(
                &app.session_id,
                StoredRole::Assistant,
                output.trim(),
            );
            app.status = "ready".to_string();
            app.pending = None;
        }
        Ok(Err(err)) => {
            app.push_error(format!("provider error: {}", err));
            app.status = "error".to_string();
            app.pending = None;
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            app.push_error("provider channel disconnected");
            app.status = "error".to_string();
            app.pending = None;
        }
        Err(mpsc::TryRecvError::Empty) => {}
    }
}

fn render(app: &App<'_>) -> Result<()> {
    let mut out = io::stdout();
    let (width, height) = terminal::size().context("failed to read terminal size")?;

    queue!(out, cursor::MoveTo(0, 0), Clear(ClearType::All))?;

    let header = format!(
        "AUTO-CODE  provider={}  cwd={}",
        app.provider_kind.as_str(),
        app.workdir.display()
    );
    draw_line(&mut out, 0, width, &header, Color::Cyan)?;

    let status_text = if let Some(pending) = app.pending.as_ref() {
        let elapsed = pending.started.elapsed();
        let frame = WAIT_FRAMES[(elapsed.as_millis() as usize / 200) % WAIT_FRAMES.len()];
        format!(
            "status=thinking {} elapsed={}s  (/help for commands)",
            frame,
            elapsed.as_secs()
        )
    } else {
        format!("status={}  (/help for commands)", app.status)
    };
    draw_line(&mut out, 1, width, &status_text, Color::DarkGrey)?;
    draw_line(
        &mut out,
        2,
        width,
        &"-".repeat(width as usize),
        Color::DarkGrey,
    )?;

    let input_separator_row = height.saturating_sub(2);
    let input_row = height.saturating_sub(1);
    let message_top = 3usize;
    let message_height = (input_separator_row as usize).saturating_sub(message_top);

    let lines = collect_recent_render_lines(&app.messages, width as usize, message_height);
    for (idx, line) in lines.iter().enumerate() {
        draw_line(
            &mut out,
            (message_top + idx) as u16,
            width,
            &line.text,
            line.color,
        )?;
    }

    draw_line(
        &mut out,
        input_separator_row,
        width,
        &"-".repeat(width as usize),
        Color::DarkGrey,
    )?;

    let prompt = if app.pending.is_some() { "… " } else { "> " };
    let max_input_width = (width as usize).saturating_sub(prompt.chars().count());
    let input_tail = tail_chars(&app.input, max_input_width);
    let input_text = format!("{}{}", prompt, input_tail);
    draw_line(&mut out, input_row, width, &input_text, Color::White)?;

    let cursor_x = (prompt.chars().count() + input_tail.chars().count()) as u16;
    queue!(
        out,
        cursor::MoveTo(cursor_x.min(width.saturating_sub(1)), input_row),
        cursor::Show
    )?;
    out.flush().context("failed to flush terminal")?;
    Ok(())
}

fn draw_line(out: &mut io::Stdout, row: u16, width: u16, text: &str, color: Color) -> Result<()> {
    let truncated = truncate_chars(text, width as usize);
    queue!(
        out,
        cursor::MoveTo(0, row),
        SetForegroundColor(color),
        Print(truncated),
        ResetColor
    )?;
    Ok(())
}

fn collect_recent_render_lines(
    messages: &[Message],
    width: usize,
    max_lines: usize,
) -> Vec<RenderLine> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut lines = VecDeque::new();
    for message in messages.iter().rev() {
        let (prefix, color) = match message.role {
            MessageRole::User => ("you> ", Color::Cyan),
            MessageRole::Assistant => ("ai > ", Color::Green),
            MessageRole::System => ("sys> ", Color::Blue),
            MessageRole::Error => ("err> ", Color::Red),
        };

        let wrapped = wrap_with_prefix(&message.text, prefix, width);
        for line in wrapped.into_iter().rev() {
            lines.push_front(RenderLine { color, text: line });
            if lines.len() > max_lines {
                let _ = lines.pop_back();
            }
        }

        if lines.len() >= max_lines {
            break;
        }
    }

    lines.into_iter().collect::<Vec<_>>()
}

fn wrap_with_prefix(text: &str, prefix: &str, width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let indent = " ".repeat(prefix.chars().count());
    let first_width = width.saturating_sub(prefix.chars().count()).max(1);
    let rest_width = width.saturating_sub(indent.chars().count()).max(1);

    let normalized = if text.is_empty() {
        vec![""]
    } else {
        text.lines().collect()
    };
    for line in normalized {
        let chunks = wrap_line(line, first_width, rest_width);
        for (idx, chunk) in chunks.into_iter().enumerate() {
            if idx == 0 {
                result.push(format!("{}{}", prefix, chunk));
            } else {
                result.push(format!("{}{}", indent, chunk));
            }
        }
    }

    result
}

fn wrap_line(line: &str, first_width: usize, rest_width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let chars = line.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    let mut width = first_width;

    while index < chars.len() {
        let end = (index + width).min(chars.len());
        let slice = chars[index..end].iter().collect::<String>();
        chunks.push(slice);
        index = end;
        width = rest_width;
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn truncate_chars(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    text.chars().take(width).collect::<String>()
}

fn tail_chars(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= width {
        return text.to_string();
    }
    chars[chars.len() - width..].iter().collect()
}

fn build_prompt(messages: &[Message], input: &str) -> String {
    let mut prompt = String::from(
        "You are autocode interactive coding assistant. Respond concisely and with executable guidance when needed.\n\n",
    );

    let mut convo = messages
        .iter()
        .filter_map(|msg| match msg.role {
            MessageRole::User => Some(("user", msg.text.as_str())),
            MessageRole::Assistant => Some(("assistant", msg.text.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    if convo.len() > HISTORY_LIMIT * 2 {
        let drain = convo.len().saturating_sub(HISTORY_LIMIT * 2);
        convo.drain(0..drain);
    }

    if !convo.is_empty() {
        prompt.push_str("Conversation:\n");
        for (role, text) in convo {
            prompt.push_str(&format!("{}: {}\n", role, text));
        }
        prompt.push('\n');
    }

    prompt.push_str(&format!("user: {}\nassistant:", input));
    prompt
}

fn transcript_to_messages(transcript: &SessionTranscript) -> Vec<Message> {
    transcript
        .messages
        .iter()
        .map(|msg| Message {
            role: match msg.role {
                StoredRole::User => MessageRole::User,
                StoredRole::Assistant => MessageRole::Assistant,
                StoredRole::System => MessageRole::System,
                StoredRole::Error => MessageRole::Error,
            },
            text: msg.text.clone(),
        })
        .collect()
}
