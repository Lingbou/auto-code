use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

const RESET: &str = "\x1b[0m";
const FG_CYAN_BOLD: &str = "\x1b[96m\x1b[1m";
const FG_DIM: &str = "\x1b[90m";
const FG_GREEN_BOLD: &str = "\x1b[92m\x1b[1m";
const FG_YELLOW_BOLD: &str = "\x1b[93m\x1b[1m";
const FG_RED_BOLD: &str = "\x1b[91m\x1b[1m";
const FG_BLUE_BOLD: &str = "\x1b[94m\x1b[1m";

pub fn clear_screen() {
    if stdout_is_terminal() {
        print!("\x1b[2J\x1b[H");
        let _ = io::stdout().flush();
    }
}

pub fn print_header(workdir: &Path, provider: &str) {
    println!(
        "{}AUTO-CODE{}  terminal coding agent",
        style(FG_CYAN_BOLD),
        style(RESET)
    );
    println!(
        "{}mode{} interactive   {}provider{} {}   {}cwd{} {}",
        style(FG_DIM),
        style(RESET),
        style(FG_DIM),
        style(RESET),
        provider,
        style(FG_DIM),
        style(RESET),
        workdir.display()
    );
    println!("{}", line(96));
}

pub fn print_system(message: &str) {
    println!(
        "{}[system]{} {}",
        style(FG_BLUE_BOLD),
        style(RESET),
        message
    );
}

pub fn print_warn(message: &str) {
    println!(
        "{}[warning]{} {}",
        style(FG_YELLOW_BOLD),
        style(RESET),
        message
    );
}

pub fn print_error(message: &str) {
    println!("{}[error]{} {}", style(FG_RED_BOLD), style(RESET), message);
}

pub fn print_help() {
    println!("{}", line(96));
    println!("Commands");
    println!("  /help");
    println!("  /exit | /quit");
    println!("  /provider auto|claude|opencode");
    println!("  /plugin <id> <cmd> [args...]");
    println!("  /prd <cmd> [args...]");
    println!("  /session");
    println!("  /sessions");
    println!("  /resume [id]");
    println!("  /clear");
    println!("  <message> send prompt to provider");
    println!("{}", line(96));
}

pub fn print_prompt(provider: &str) {
    print!(
        "{}autocode:{}{}> {}",
        style(FG_CYAN_BOLD),
        style(RESET),
        provider,
        style(RESET)
    );
    let _ = io::stdout().flush();
}

pub fn print_wait(provider: &str, frame: &str, elapsed: Duration) {
    if !stdout_is_terminal() {
        return;
    }
    let elapsed_secs = elapsed.as_secs();
    print!(
        "\r\x1b[2K{}[thinking]{} provider={} elapsed={}s {}",
        style(FG_DIM),
        style(RESET),
        provider,
        elapsed_secs,
        frame
    );
    let _ = io::stdout().flush();
}

pub fn clear_wait() {
    if !stdout_is_terminal() {
        return;
    }
    print!("\r\x1b[2K");
    let _ = io::stdout().flush();
}

pub fn print_assistant(provider: &str, output: &str, elapsed: Duration) {
    println!(
        "{}[assistant:{}]{} elapsed={}s",
        style(FG_GREEN_BOLD),
        provider,
        style(RESET),
        elapsed.as_secs()
    );
    println!("{}", line(96));
    println!("{}", output.trim());
    println!("{}", line(96));
}

pub fn print_exit() {
    println!("{}[system]{} bye.", style(FG_BLUE_BOLD), style(RESET));
}

fn style(input: &'static str) -> &'static str {
    if stdout_is_terminal() {
        input
    } else {
        ""
    }
}

fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

fn line(width: usize) -> String {
    "-".repeat(width)
}
