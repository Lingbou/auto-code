use std::process::{Command, Stdio};

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Claude,
    Opencode,
}

impl ProviderKind {
    pub fn command(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Opencode => "opencode",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Opencode => "opencode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSelection {
    Auto,
    Claude,
    Opencode,
}

impl ProviderSelection {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "claude" => Some(Self::Claude),
            "opencode" => Some(Self::Opencode),
            _ => None,
        }
    }
}

pub fn resolve_provider(selection: ProviderSelection) -> Result<ProviderKind> {
    let resolved = match selection {
        ProviderSelection::Claude => ProviderKind::Claude,
        ProviderSelection::Opencode => ProviderKind::Opencode,
        ProviderSelection::Auto => detect_default_provider()?,
    };

    Ok(resolved)
}

pub fn detect_default_provider() -> Result<ProviderKind> {
    if provider_available(ProviderKind::Claude) {
        return Ok(ProviderKind::Claude);
    }

    if provider_available(ProviderKind::Opencode) {
        return Ok(ProviderKind::Opencode);
    }

    bail!(
        "no available provider found in PATH. install `claude` or `opencode`, or pass --provider explicitly"
    )
}

pub fn provider_available(provider: ProviderKind) -> bool {
    let command = provider.command();
    let status = Command::new(command)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    status.map(|v| v.success()).unwrap_or(false)
}
