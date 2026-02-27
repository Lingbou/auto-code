use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProviderArg {
    Auto,
    Claude,
    Opencode,
}

#[derive(Debug, Parser)]
#[command(name = "autocode")]
#[command(about = "Autocode terminal agent (Rust), with plugin-based PRD runner")]
pub struct Cli {
    /// Provider backend (auto/claude/opencode)
    #[arg(global = true, long, value_enum, default_value_t = ProviderArg::Auto)]
    pub provider: ProviderArg,

    /// Enable verbose logs
    #[arg(global = true, long)]
    pub verbose: bool,

    /// Continue the latest session in current workspace
    #[arg(global = true, long = "continue", short = 'c')]
    pub continue_last: bool,

    /// Resume a specific session id
    #[arg(global = true, long, short = 's')]
    pub session: Option<String>,

    /// Fork from resumed session (compatible surface, reserved for future use)
    #[arg(global = true, long)]
    pub fork: bool,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// Optional prompt tokens for direct entry
    #[arg(value_name = "PROMPT", num_args = 0.., trailing_var_arg = true)]
    pub prompt: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run PRD runner (compat alias of plugin prd-runner run)
    Run(RunArgs),
    /// Plugin command router: autocode plugin <id> <cmd> ...
    Plugin(PluginArgs),
    /// Built-in alias for prd-runner plugin: autocode prd <cmd> ...
    Prd(PrdArgs),
    /// Inspect environment and provider availability
    Doctor,
    /// Manage local chat sessions
    Session(SessionArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long, default_value = "10m")]
    pub max_runtime: String,
    #[arg(long)]
    pub provider_timeout: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PluginArgs {
    /// Plugin invocation tokens: <plugin-id> <command> [args...]
    #[arg(value_name = "TOKENS", num_args = 0.., trailing_var_arg = true)]
    pub tokens: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PrdArgs {
    /// prd alias tokens: <command> [args...]
    #[arg(value_name = "TOKENS", num_args = 0.., trailing_var_arg = true)]
    pub tokens: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: SessionCommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// List sessions
    List(SessionListArgs),
    /// Delete session by id
    Delete(SessionDeleteArgs),
}

#[derive(Debug, Args)]
pub struct SessionListArgs {
    /// Show at most N most recent sessions (0 = all)
    #[arg(long, short = 'n', default_value_t = 20)]
    pub max_count: usize,
    /// Output format
    #[arg(long, default_value = "table")]
    pub format: String,
}

#[derive(Debug, Args)]
pub struct SessionDeleteArgs {
    /// Session id
    pub session_id: String,
}

pub fn parse_cli() -> Cli {
    let rewritten = rewrite_legacy_args(std::env::args().collect());
    Cli::parse_from(rewritten)
}

fn rewrite_legacy_args(args: Vec<String>) -> Vec<String> {
    if args.len() < 2 {
        return args;
    }

    let first = args[1].as_str();
    if first != "claude" && first != "opencode" {
        return args;
    }

    // Backward compatibility:
    // autocode claude --max-runtime 10m  ==> autocode run --provider claude --max-runtime 10m
    let mut rewritten = vec![args[0].clone(), "run".to_string(), "--provider".to_string()];
    rewritten.push(first.to_string());
    rewritten.extend(args.into_iter().skip(2));
    rewritten
}

#[cfg(test)]
mod tests {
    use super::rewrite_legacy_args;

    #[test]
    fn rewrites_legacy_provider_entrypoint() {
        let input = vec![
            "autocode".to_string(),
            "claude".to_string(),
            "--max-runtime".to_string(),
            "10m".to_string(),
        ];

        let rewritten = rewrite_legacy_args(input);
        assert_eq!(
            rewritten,
            vec![
                "autocode".to_string(),
                "run".to_string(),
                "--provider".to_string(),
                "claude".to_string(),
                "--max-runtime".to_string(),
                "10m".to_string(),
            ]
        );
    }

    #[test]
    fn keeps_normal_commands_unchanged() {
        let input = vec![
            "autocode".to_string(),
            "plugin".to_string(),
            "list".to_string(),
        ];
        let rewritten = rewrite_legacy_args(input.clone());
        assert_eq!(rewritten, input);
    }
}
