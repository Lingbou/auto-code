use std::time::Duration;

use anyhow::{Context, Result};
use humantime::parse_duration;
use serde::{Deserialize, Serialize};

pub mod parser;
pub mod prd;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub convergence: ConvergenceConfig,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub checkpoint: CheckpointConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub provider: ProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergenceConfig {
    #[serde(default = "defaults::max_runtime")]
    pub max_runtime: String,
}

impl ConvergenceConfig {
    pub fn max_runtime_duration(&self) -> Result<Duration> {
        parse_duration(&self.max_runtime)
            .with_context(|| format!("invalid convergence.max_runtime: {}", self.max_runtime))
    }
}

impl Default for ConvergenceConfig {
    fn default() -> Self {
        Self {
            max_runtime: defaults::max_runtime(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default = "defaults::command_timeout")]
    pub command_timeout: String,
    #[serde(default = "defaults::max_retry")]
    pub max_retry: u32,
}

impl ExecutionConfig {
    pub fn command_timeout_duration(&self) -> Result<Duration> {
        parse_duration(&self.command_timeout).with_context(|| {
            format!(
                "invalid execution.command_timeout: {}",
                self.command_timeout
            )
        })
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            command_timeout: defaults::command_timeout(),
            max_retry: defaults::max_retry(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    #[serde(default = "defaults::checkpoint_enabled")]
    pub enabled: bool,
    #[serde(default = "defaults::max_checkpoints")]
    pub max_checkpoints: usize,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::checkpoint_enabled(),
            max_checkpoints: defaults::max_checkpoints(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "defaults::log_level")]
    pub level: String,
    #[serde(default = "defaults::save_all_output")]
    pub save_all_output: bool,
    #[serde(default = "defaults::output_summary_max_chars")]
    pub output_summary_max_chars: usize,
    #[serde(default = "defaults::log_max_file_size_bytes")]
    pub max_file_size_bytes: u64,
    #[serde(default = "defaults::log_max_rotated_files")]
    pub max_rotated_files: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: defaults::log_level(),
            save_all_output: defaults::save_all_output(),
            output_summary_max_chars: defaults::output_summary_max_chars(),
            max_file_size_bytes: defaults::log_max_file_size_bytes(),
            max_rotated_files: defaults::log_max_rotated_files(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "defaults::provider_response_timeout")]
    pub response_timeout: String,
}

impl ProviderConfig {
    pub fn response_timeout_duration(&self) -> Result<Duration> {
        parse_duration(&self.response_timeout).with_context(|| {
            format!(
                "invalid provider.response_timeout: {}",
                self.response_timeout
            )
        })
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            response_timeout: defaults::provider_response_timeout(),
        }
    }
}

mod defaults {
    pub fn max_runtime() -> String {
        "4h".to_string()
    }

    pub fn command_timeout() -> String {
        "60s".to_string()
    }

    pub fn max_retry() -> u32 {
        3
    }

    pub fn checkpoint_enabled() -> bool {
        true
    }

    pub fn max_checkpoints() -> usize {
        10
    }

    pub fn log_level() -> String {
        "info".to_string()
    }

    pub fn save_all_output() -> bool {
        true
    }

    pub fn output_summary_max_chars() -> usize {
        400
    }

    pub fn log_max_file_size_bytes() -> u64 {
        10 * 1024 * 1024
    }

    pub fn log_max_rotated_files() -> usize {
        5
    }

    pub fn provider_response_timeout() -> String {
        "20m".to_string()
    }
}
