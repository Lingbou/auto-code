use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::prd::Requirement;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReqStatus {
    Todo,
    InProgress,
    Done,
    Blocked,
    Failed,
}

impl ReqStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReqStatus::Todo => "todo",
            ReqStatus::InProgress => "in_progress",
            ReqStatus::Done => "done",
            ReqStatus::Blocked => "blocked",
            ReqStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReqEvidence {
    pub command: String,
    pub exit_code: i32,
    pub output_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReqRecord {
    pub status: ReqStatus,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ReqEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for ReqRecord {
    fn default() -> Self {
        Self {
            status: ReqStatus::Todo,
            attempts: 0,
            evidence: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineState {
    pub iteration: u32,
    pub req_status: BTreeMap<String, ReqRecord>,
}

impl EngineState {
    pub fn new(requirements: &[Requirement]) -> Self {
        let mut req_status = BTreeMap::new();
        for req in requirements {
            req_status.insert(req.id.clone(), ReqRecord::default());
        }

        Self {
            iteration: 0,
            req_status,
        }
    }

    pub fn all_done(&self) -> bool {
        self.req_status
            .values()
            .all(|record| record.status == ReqStatus::Done)
    }

    pub fn overall_progress(&self) -> f64 {
        if self.req_status.is_empty() {
            return 0.0;
        }

        let done = self
            .req_status
            .values()
            .filter(|record| record.status == ReqStatus::Done)
            .count() as f64;

        done / self.req_status.len() as f64
    }

    pub fn status_snapshot(&self) -> BTreeMap<String, ReqStatus> {
        self.req_status
            .iter()
            .map(|(id, record)| (id.clone(), record.status))
            .collect()
    }

    pub fn has_progress_since(&self, prev: &BTreeMap<String, ReqStatus>) -> bool {
        self.req_status.iter().any(|(id, record)| {
            let old = prev.get(id).copied().unwrap_or(ReqStatus::Todo);
            old != ReqStatus::Done && record.status == ReqStatus::Done
        })
    }

    pub fn mark_in_progress(&mut self, req_id: &str) {
        if let Some(record) = self.req_status.get_mut(req_id) {
            record.status = ReqStatus::InProgress;
        }
    }

    pub fn update(
        &mut self,
        req_id: &str,
        status: ReqStatus,
        evidence: Option<ReqEvidence>,
        error: Option<String>,
        attempt_increment: u32,
    ) {
        if let Some(record) = self.req_status.get_mut(req_id) {
            record.status = status;
            record.attempts = record.attempts.saturating_add(attempt_increment);
            record.evidence = evidence;
            record.last_error = error;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::prd::Requirement;

    use super::{EngineState, ReqEvidence, ReqStatus};

    fn sample_requirements() -> Vec<Requirement> {
        vec![
            Requirement {
                id: "REQ-001".to_string(),
                title: "build".to_string(),
                priority: Some("high".to_string()),
                description: "build project".to_string(),
                validate_command: "echo ok".to_string(),
                pass_condition: "退出码 = 0".to_string(),
                tasks: vec!["do build".to_string()],
            },
            Requirement {
                id: "REQ-002".to_string(),
                title: "test".to_string(),
                priority: Some("high".to_string()),
                description: "run tests".to_string(),
                validate_command: "echo ok".to_string(),
                pass_condition: "退出码 = 0".to_string(),
                tasks: vec!["do test".to_string()],
            },
        ]
    }

    #[test]
    fn reports_overall_progress() {
        let reqs = sample_requirements();
        let mut state = EngineState::new(&reqs);
        assert_eq!(state.overall_progress(), 0.0);

        state.update(
            "REQ-001",
            ReqStatus::Done,
            Some(ReqEvidence {
                command: "echo ok".to_string(),
                exit_code: 0,
                output_summary: "ok".to_string(),
            }),
            None,
            1,
        );

        assert!((state.overall_progress() - 0.5).abs() < f64::EPSILON);
        assert!(!state.all_done());
    }

    #[test]
    fn detects_progress_between_snapshots() {
        let reqs = sample_requirements();
        let mut state = EngineState::new(&reqs);
        let before = state.status_snapshot();
        state.update("REQ-001", ReqStatus::Done, None, None, 1);
        assert!(state.has_progress_since(&before));
    }
}
