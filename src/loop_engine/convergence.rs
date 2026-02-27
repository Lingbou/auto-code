use std::fmt;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::ConvergenceConfig;

#[derive(Debug, Clone)]
pub struct ConvergenceGuard {
    start: Instant,
    config: ConvergenceConfig,
    max_runtime: std::time::Duration,
}

#[derive(Debug, Clone)]
pub enum StopReason {
    MaxRuntime(String),
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::MaxRuntime(v) => write!(f, "reached max_runtime ({})", v),
        }
    }
}

impl ConvergenceGuard {
    pub fn new(config: ConvergenceConfig) -> Result<Self> {
        let max_runtime = config.max_runtime_duration()?;
        Ok(Self {
            start: Instant::now(),
            config,
            max_runtime,
        })
    }

    pub fn check(&self) -> Option<StopReason> {
        if self.start.elapsed() >= self.max_runtime {
            return Some(StopReason::MaxRuntime(self.config.max_runtime.clone()));
        }

        None
    }

    pub fn remaining(&self) -> Duration {
        self.max_runtime.saturating_sub(self.start.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::ConvergenceConfig;

    use super::ConvergenceGuard;

    #[test]
    fn stops_by_runtime_threshold() {
        let guard = ConvergenceGuard::new(ConvergenceConfig {
            max_runtime: "0s".to_string(),
        })
        .expect("guard should be created");

        let reason = guard.check().expect("expected to stop on max runtime");
        assert!(reason.to_string().contains("max_runtime"));
    }
}
