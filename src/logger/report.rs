use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationReport {
    pub iteration: u32,
    pub timestamp: DateTime<Utc>,
    pub duration_seconds: u64,
    pub req_status: BTreeMap<String, ReqReport>,
    pub overall_progress: f64,
    pub has_progress: bool,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReqReport {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceReport {
    pub command: String,
    pub exit_code: i32,
    pub output_summary: String,
}

pub fn write_report(path: &Path, report: &IterationReport) -> Result<()> {
    let content = serde_json::to_string_pretty(report).context("failed to serialize report")?;
    std::fs::write(path, content)
        .with_context(|| format!("failed to write report file {}", path.display()))
}
