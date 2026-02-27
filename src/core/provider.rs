use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use tracing::info;

use crate::runtime::signal;

pub trait Provider: Send {
    fn name(&self) -> &str;
    fn start(&mut self) -> Result<()>;
    fn restart(&mut self) -> Result<()>;
    fn send(&mut self, input: &str) -> Result<()>;
    fn read_output(&mut self, timeout: Duration) -> Result<String>;
    fn is_waiting_for_input(&self, output: &str) -> bool;
}

#[derive(Debug)]
pub struct CliPrintProvider {
    command: String,
    workdir: PathBuf,
    pending_prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderFlavor {
    Claude,
    Opencode,
    Generic,
}

#[derive(Debug, Clone)]
struct ClaudeParsedOutput {
    text: String,
    is_error: bool,
}

impl CliPrintProvider {
    pub fn new(command: String, workdir: impl AsRef<Path>) -> Self {
        Self {
            command,
            workdir: workdir.as_ref().to_path_buf(),
            pending_prompt: None,
        }
    }

    fn flavor(&self) -> ProviderFlavor {
        match self.command.trim() {
            "claude" => ProviderFlavor::Claude,
            "opencode" => ProviderFlavor::Opencode,
            _ => ProviderFlavor::Generic,
        }
    }

    fn run_prompt(&self, prompt: &str, timeout: Duration) -> Result<String> {
        if prompt.trim().is_empty() {
            bail!("provider prompt is empty");
        }

        match self.flavor() {
            ProviderFlavor::Claude => self.run_claude(prompt, timeout),
            ProviderFlavor::Opencode => self.run_opencode(prompt, timeout),
            ProviderFlavor::Generic => self.run_generic(prompt, timeout),
        }
    }

    fn run_claude(&self, prompt: &str, timeout: Duration) -> Result<String> {
        let mut command = Command::new("claude");
        apply_claude_config_fallback(&mut command, &self.workdir)?;
        command
            .arg("-p")
            .arg("--output-format")
            .arg("json")
            .arg("--input-format")
            .arg("text")
            .arg("--no-session-persistence")
            .arg("--permission-mode")
            .arg("bypassPermissions")
            .arg(prompt)
            .current_dir(&self.workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = run_with_timeout(command, timeout).context("failed to execute claude")?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let parsed = parse_claude_output(&stdout);
        let normalized = parsed
            .as_ref()
            .map(|v| v.text.clone())
            .unwrap_or_else(|_| stdout.trim().to_string());

        if let Ok(parsed) = parsed {
            if parsed.is_error {
                if stderr.trim().is_empty() {
                    return Err(anyhow!("claude returned error: {}", parsed.text));
                }
                return Err(anyhow!(
                    "claude returned error: {} [{}]",
                    parsed.text,
                    stderr.trim()
                ));
            }
        }

        if normalized.is_empty() {
            if !stderr.trim().is_empty() {
                return Err(anyhow!("claude returned empty output: {}", stderr.trim()));
            }

            return Err(anyhow!("claude returned empty output"));
        }

        if output.status.success() {
            return Ok(normalized);
        }

        if stderr.trim().is_empty() {
            return Ok(normalized);
        }

        Ok(format!(
            "{}\n\n[provider-stderr]\n{}",
            normalized,
            stderr.trim()
        ))
    }

    fn run_opencode(&self, prompt: &str, timeout: Duration) -> Result<String> {
        let mut command = Command::new("opencode");
        command
            .arg("-p")
            .arg(prompt)
            .current_dir(&self.workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = run_with_timeout(command, timeout).context("failed to execute opencode")?;
        output_to_text("opencode", output)
    }

    fn run_generic(&self, prompt: &str, timeout: Duration) -> Result<String> {
        let mut command = Command::new(&self.command);
        command
            .arg(prompt)
            .current_dir(&self.workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = run_with_timeout(command, timeout)
            .with_context(|| format!("failed to execute provider '{}'", self.command))?;
        output_to_text(&self.command, output)
    }
}

impl Provider for CliPrintProvider {
    fn name(&self) -> &str {
        "cli-print"
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn restart(&mut self) -> Result<()> {
        self.pending_prompt = None;
        Ok(())
    }

    fn send(&mut self, input: &str) -> Result<()> {
        self.pending_prompt = Some(input.to_string());
        Ok(())
    }

    fn read_output(&mut self, timeout: Duration) -> Result<String> {
        let prompt = self
            .pending_prompt
            .take()
            .context("provider has no pending prompt; send() must be called first")?;

        self.run_prompt(&prompt, timeout)
    }

    fn is_waiting_for_input(&self, _output: &str) -> bool {
        false
    }
}

fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<Output> {
    let mut child = command
        .spawn()
        .context("failed to spawn provider process")?;

    let timed_out = wait_with_timeout(&mut child, timeout)?;
    if timed_out {
        child
            .kill()
            .context("failed to kill timed-out provider process")?;
        let _ = child.wait();
        bail!("provider timed out after {:?}", timeout);
    }

    let output = child
        .wait_with_output()
        .context("failed to collect provider output")?;

    Ok(output)
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<bool> {
    let started = Instant::now();
    let poll_interval = Duration::from_millis(20);
    let heartbeat_interval = Duration::from_secs(30);
    let mut next_heartbeat = heartbeat_interval;

    loop {
        if signal::interrupted() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("execution interrupted by Ctrl+C");
        }

        if child
            .try_wait()
            .context("failed while waiting for provider process")?
            .is_some()
        {
            return Ok(false);
        }

        if started.elapsed() >= next_heartbeat {
            info!(
                elapsed_s = started.elapsed().as_secs(),
                timeout_s = timeout.as_secs(),
                "still waiting for provider response"
            );
            next_heartbeat += heartbeat_interval;
        }

        if started.elapsed() >= timeout {
            return Ok(true);
        }

        std::thread::sleep(poll_interval);
    }
}

fn output_to_text(provider: &str, output: Output) -> Result<String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        if stdout.is_empty() {
            if stderr.is_empty() {
                bail!("provider '{}' returned empty output", provider);
            }
            return Ok(stderr);
        }
        return Ok(stdout);
    }

    let message = if stdout.is_empty() {
        stderr
    } else if stderr.is_empty() {
        stdout
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    Err(anyhow!(
        "provider '{}' exited with code {:?}: {}",
        provider,
        output.status.code(),
        message
    ))
}

fn apply_claude_config_fallback(command: &mut Command, workdir: &Path) -> Result<()> {
    if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        return Ok(());
    }

    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };

    let default_config = PathBuf::from(home).join(".claude");
    if can_write_claude_config(&default_config) {
        return Ok(());
    }

    let local_config = workdir.join(".autocode").join("claude_config");
    std::fs::create_dir_all(&local_config).with_context(|| {
        format!(
            "failed to create fallback Claude config dir {}",
            local_config.display()
        )
    })?;
    command.env("CLAUDE_CONFIG_DIR", local_config);
    Ok(())
}

fn can_write_claude_config(config_dir: &Path) -> bool {
    let debug_dir = config_dir.join("debug");
    if std::fs::create_dir_all(&debug_dir).is_err() {
        return false;
    }

    let probe_path = debug_dir.join(".autocode_write_probe");
    let opened = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe_path);

    match opened {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(probe_path);
            true
        }
        Err(_) => false,
    }
}

fn parse_claude_output(stdout: &str) -> Result<ClaudeParsedOutput> {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        if let Ok(parsed) = extract_claude_result(value) {
            return Ok(parsed);
        }
    }

    bail!("no Claude JSON payload found in provider output")
}

fn extract_claude_result(value: Value) -> Result<ClaudeParsedOutput> {
    let Some(obj) = value.as_object() else {
        bail!("Claude output root is not a JSON object");
    };
    let is_error = obj
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if let Some(result) = obj.get("result") {
        return match result {
            Value::String(text) => Ok(ClaudeParsedOutput {
                text: text.trim().to_string(),
                is_error,
            }),
            other => Ok(ClaudeParsedOutput {
                text: serde_json::to_string_pretty(other)?,
                is_error,
            }),
        };
    }

    if let Some(message) = obj.get("message") {
        if let Some(text) = extract_text_from_message(message) {
            return Ok(ClaudeParsedOutput { text, is_error });
        }
    }

    if let Some(errors) = obj.get("errors").and_then(Value::as_array) {
        let joined = errors
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; ");

        if !joined.trim().is_empty() {
            return Ok(ClaudeParsedOutput {
                text: joined,
                is_error,
            });
        }
    }

    Ok(ClaudeParsedOutput {
        text: serde_json::to_string_pretty(&value)?,
        is_error,
    })
}

fn extract_text_from_message(message: &Value) -> Option<String> {
    let content = message.get("content")?.as_array()?;

    let mut fragments = Vec::new();
    for block in content {
        if block.get("type")?.as_str()? != "text" {
            continue;
        }

        if let Some(text) = block.get("text").and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                fragments.push(trimmed.to_string());
            }
        }
    }

    if fragments.is_empty() {
        None
    } else {
        Some(fragments.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::parse_claude_output;

    #[test]
    fn parse_claude_result_string() -> Result<()> {
        let raw = r#"{"type":"result","subtype":"success","is_error":false,"result":"```bash\necho ok\n```"}"#;
        let parsed = parse_claude_output(raw)?;
        assert!(parsed.text.contains("echo ok"));
        assert!(!parsed.is_error);
        Ok(())
    }

    #[test]
    fn parse_claude_result_errors() -> Result<()> {
        let raw = r#"{"type":"result","is_error":true,"errors":["not logged in"]}"#;
        let parsed = parse_claude_output(raw)?;
        assert!(parsed.text.contains("not logged in"));
        assert!(parsed.is_error);
        Ok(())
    }
}
