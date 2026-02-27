use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use regex::Regex;

use crate::config::prd::{PrdDocument, Requirement};
use crate::core::provider::Provider;

#[derive(Debug, Clone)]
pub struct AiInstruction {
    pub raw_output: String,
    pub commands: Vec<String>,
}

pub struct AiProcess {
    provider: Box<dyn Provider>,
    response_timeout: Duration,
    dry_run: bool,
}

impl AiProcess {
    pub fn new(
        mut provider: Box<dyn Provider>,
        response_timeout: Duration,
        dry_run: bool,
    ) -> Result<Self> {
        if !dry_run {
            provider.start().context("failed to start provider")?;
        }

        Ok(Self {
            provider,
            response_timeout,
            dry_run,
        })
    }

    pub fn build_prompt(&self, prd: &PrdDocument, req: &Requirement, workdir: &Path) -> String {
        let mut text = format!(
            "You are coding for project '{}'.\n\
Workspace root: {}\n\
Requirement: {} - {}\n\
Description: {}\n\
Tasks:\n{}\n\
Validation command: {}\n\
Pass condition: {}\n\
Respond with a short plan and shell commands in fenced bash blocks.\n",
            prd.project_name.as_deref().unwrap_or("unknown"),
            workdir.display(),
            req.id,
            req.title,
            req.description,
            req.tasks
                .iter()
                .map(|task| format!("- {}", task))
                .collect::<Vec<String>>()
                .join("\n"),
            req.validate_command,
            req.pass_condition,
        );

        text.push_str(
            "\nOutput rules:\n\
- Only output commands that should be executed in the workspace.\n\
- Operate directly in workspace root. Do not create a nested project directory like `rust-tetris/`.\n\
- Prefer relative paths from workspace root.\n\
- Put executable commands inside ```bash fenced blocks.\n\
- If you think requirement is already complete, still output a verification command in a bash block.\n\
- If nothing can be executed, output exactly: CMD: # no-op\n",
        );
        text
    }

    pub fn generate_instruction(&mut self, prompt: &str) -> Result<AiInstruction> {
        if self.dry_run {
            return Ok(AiInstruction {
                raw_output: "[dry-run] provider execution skipped".to_string(),
                commands: Vec::new(),
            });
        }

        self.provider
            .send(prompt)
            .context("failed to send prompt to provider")?;
        let output = self
            .provider
            .read_output(self.response_timeout)
            .context("failed to read output from provider")?;

        let commands = extract_commands_from_output(&output)?;
        Ok(AiInstruction {
            raw_output: output,
            commands,
        })
    }

    pub fn set_response_timeout(&mut self, timeout: Duration) {
        self.response_timeout = timeout;
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

fn extract_commands_from_output(output: &str) -> Result<Vec<String>> {
    let mut commands = Vec::new();
    let normalized = normalize_model_output(output)?;

    let mut in_shell_block = false;
    let mut block_content = String::new();

    for event in Parser::new(&normalized) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                in_shell_block = is_shell_language(lang.as_ref());
                block_content.clear();
            }
            Event::Text(text) if in_shell_block => {
                block_content.push_str(&text);
            }
            Event::End(TagEnd::CodeBlock) if in_shell_block => {
                if let Some(script) = shell_block_to_command(&block_content) {
                    commands.push(script);
                }
                in_shell_block = false;
                block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_shell_block = false;
                block_content.clear();
            }
            _ => {}
        }
    }

    let cmd_regex =
        Regex::new(r"(?m)^\s*CMD\s*:\s*(.+?)\s*$").context("failed to compile CMD line regex")?;
    for cap in cmd_regex.captures_iter(&normalized) {
        commands.push(cap[1].trim().to_string());
    }

    let shell_prompt_regex =
        Regex::new(r"(?m)^\s*\$\s+(.+?)\s*$").context("failed to compile $ command regex")?;
    for cap in shell_prompt_regex.captures_iter(&normalized) {
        commands.push(cap[1].trim().to_string());
    }

    commands.retain(|cmd| !cmd.is_empty());
    commands.retain(|cmd| !is_comment_only_command(cmd));
    commands.dedup();

    Ok(commands)
}

fn normalize_model_output(output: &str) -> Result<String> {
    let ansi_regex =
        Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").context("failed to compile ANSI control regex")?;
    let stripped = ansi_regex.replace_all(output, "");
    Ok(stripped.replace('\r', ""))
}

fn is_shell_language(lang: &str) -> bool {
    let lower = lang.trim().to_ascii_lowercase();
    matches!(lower.as_str(), "bash" | "sh" | "shell" | "zsh")
}

fn shell_block_to_command(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Ignore blocks that only contain comments.
    let has_executable_line = trimmed.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    });

    if !has_executable_line {
        return None;
    }

    Some(
        trimmed
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn is_comment_only_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return true;
    }

    !trimmed.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    })
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::extract_commands_from_output;

    #[test]
    fn extracts_commands_from_fenced_block() -> Result<()> {
        let output = r#"
Plan:
```bash
cargo fmt
cargo test
```
"#;

        let commands = extract_commands_from_output(output)?;
        assert_eq!(commands, vec!["cargo fmt\ncargo test"]);
        Ok(())
    }

    #[test]
    fn keeps_heredoc_script_as_single_command() -> Result<()> {
        let output = r#"
```bash
cat > Cargo.toml << 'EOF'
[package]
name = "demo"
EOF
cargo test
```
"#;

        let commands = extract_commands_from_output(output)?;
        assert_eq!(commands.len(), 1);
        assert!(commands[0].contains("<< 'EOF'"));
        assert!(commands[0].contains("[package]"));
        assert!(commands[0].contains("cargo test"));
        Ok(())
    }

    #[test]
    fn extracts_commands_from_cmd_lines() -> Result<()> {
        let output = "CMD: cargo build\nCMD: cargo test";
        let commands = extract_commands_from_output(output)?;
        assert_eq!(commands, vec!["cargo build", "cargo test"]);
        Ok(())
    }

    #[test]
    fn extracts_commands_from_shell_prompt_lines() -> Result<()> {
        let output = "$ cargo build\n$ cargo test";
        let commands = extract_commands_from_output(output)?;
        assert_eq!(commands, vec!["cargo build", "cargo test"]);
        Ok(())
    }

    #[test]
    fn strips_ansi_sequences_before_parsing() -> Result<()> {
        let output = "\u{1b}[31mCMD: cargo check\u{1b}[0m";
        let commands = extract_commands_from_output(output)?;
        assert_eq!(commands, vec!["cargo check"]);
        Ok(())
    }

    #[test]
    fn ignores_comment_only_commands() -> Result<()> {
        let output = "CMD: # no-op";
        let commands = extract_commands_from_output(output)?;
        assert!(commands.is_empty());
        Ok(())
    }

    #[test]
    fn keeps_script_with_leading_comment() -> Result<()> {
        let output = r#"
```bash
# validate only
cargo test
```
"#;
        let commands = extract_commands_from_output(output)?;
        assert_eq!(commands, vec!["# validate only\ncargo test"]);
        Ok(())
    }
}
