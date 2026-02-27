use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::ExecutionConfig;
use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub timed_out: bool,
    pub attempt: u32,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    pub fn output_summary(&self, max_chars: usize) -> String {
        let mut text = String::new();
        if !self.stdout.trim().is_empty() {
            text.push_str(self.stdout.trim());
        }

        if !self.stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(self.stderr.trim());
        }

        if text.is_empty() {
            text = format!("exit_code={}", self.exit_code);
        }

        if text.len() > max_chars {
            format!("{}...", &text[..max_chars])
        } else {
            text
        }
    }
}

#[derive(Debug)]
pub struct CommandExecutor {
    config: ExecutionConfig,
    workdir: PathBuf,
}

impl CommandExecutor {
    pub fn new(config: ExecutionConfig, workdir: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            config,
            workdir: workdir.as_ref().to_path_buf(),
        })
    }

    pub fn run(&self, command: &str) -> Result<CommandResult> {
        if command.trim().is_empty() {
            bail!("empty command is not allowed");
        }
        self.ensure_workspace_command_guard(command)?;

        let attempts = self.config.max_retry.saturating_add(1);
        let mut last = None;

        for attempt in 1..=attempts {
            let result = self.run_once(command, attempt)?;
            if result.success() {
                return Ok(result);
            }
            last = Some(result);
        }

        last.ok_or_else(|| anyhow!("unexpected empty execution result"))
    }

    fn ensure_workspace_command_guard(&self, command: &str) -> Result<()> {
        if !cargo_command_requires_manifest(command) {
            return Ok(());
        }

        let manifest_path = self.workdir.join("Cargo.toml");
        if manifest_path.exists() {
            return Ok(());
        }

        bail!(
            "command '{}' requires local Cargo.toml in {} (run `cargo init` first)",
            command,
            self.workdir.display()
        )
    }

    fn run_once(&self, command: &str, attempt: u32) -> Result<CommandResult> {
        let timeout = self
            .config
            .command_timeout_duration()
            .context("failed to parse command timeout")?;

        let started = Instant::now();

        let mut child = Command::new("bash")
            .arg("-lc")
            .arg(command)
            .current_dir(&self.workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn command: {}", command))?;

        let timed_out = wait_with_timeout(&mut child, timeout)?;

        if timed_out {
            child.kill().with_context(|| {
                format!(
                    "command timed out and kill failed: '{}' after {:?}",
                    command, timeout
                )
            })?;
        }

        let output = child
            .wait_with_output()
            .with_context(|| format!("failed to capture command output: {}", command))?;

        let duration_ms = started.elapsed().as_millis();
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(CommandResult {
            command: command.to_string(),
            exit_code,
            stdout,
            stderr,
            duration_ms,
            timed_out,
            attempt,
        })
    }
}

fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Result<bool> {
    let poll_interval = Duration::from_millis(20);
    let started = Instant::now();

    loop {
        if child
            .try_wait()
            .context("failed while waiting for command")?
            .is_some()
        {
            return Ok(false);
        }

        if started.elapsed() >= timeout {
            return Ok(true);
        }

        std::thread::sleep(poll_interval);
    }
}

fn cargo_command_requires_manifest(command: &str) -> bool {
    let mut iter = command.split_whitespace();
    let Some(first) = iter.next() else {
        return false;
    };

    if first != "cargo" {
        return false;
    }

    if command.contains("--manifest-path") {
        return false;
    }

    let subcommand = iter
        .find(|token| !token.starts_with('-') && !token.starts_with('+') && !token.contains('='));

    let Some(subcommand) = subcommand else {
        return true;
    };

    !matches!(
        subcommand,
        "new"
            | "init"
            | "install"
            | "search"
            | "login"
            | "logout"
            | "help"
            | "version"
            | "--version"
            | "-V"
    )
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::CommandExecutor;
    use crate::config::ExecutionConfig;

    #[test]
    fn rejects_empty_command() {
        let tmp = TempDir::new().expect("tmp dir should be created");
        let config = ExecutionConfig {
            command_timeout: "1s".to_string(),
            max_retry: 0,
        };

        let executor =
            CommandExecutor::new(config, tmp.path()).expect("executor should be created");
        let err = executor.run("  ").expect_err("empty command should fail");
        assert!(err.to_string().contains("empty command"));
    }

    #[test]
    fn runs_command() {
        let tmp = TempDir::new().expect("tmp dir should be created");
        let config = ExecutionConfig {
            command_timeout: "1s".to_string(),
            max_retry: 0,
        };

        let executor =
            CommandExecutor::new(config, tmp.path()).expect("executor should be created");
        let result = executor
            .run("echo hello")
            .expect("command should execute successfully");
        assert!(result.success());
        assert!(result.stdout.contains("hello"));
    }

    #[test]
    fn blocks_cargo_commands_without_local_manifest() {
        let tmp = TempDir::new().expect("tmp dir should be created");
        let config = ExecutionConfig {
            command_timeout: "1s".to_string(),
            max_retry: 0,
        };

        let executor =
            CommandExecutor::new(config, tmp.path()).expect("executor should be created");
        let err = executor
            .run("cargo test")
            .expect_err("cargo test should fail without local manifest");
        assert!(err.to_string().contains("requires local Cargo.toml"));
    }

    #[test]
    fn allows_cargo_init_without_manifest() {
        assert!(!super::cargo_command_requires_manifest(
            "cargo init --name demo ."
        ));
    }
}
