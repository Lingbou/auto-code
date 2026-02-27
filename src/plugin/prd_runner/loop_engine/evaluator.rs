use crate::plugin::prd_runner::config::prd::Requirement;
use crate::plugin::prd_runner::core::executor::CommandExecutor;
use crate::plugin::prd_runner::loop_engine::pass_condition::evaluate_pass_condition;
use crate::plugin::prd_runner::loop_engine::state::{ReqEvidence, ReqStatus};

#[derive(Debug, Clone)]
pub struct EvalResult {
    pub status: ReqStatus,
    pub evidence: Option<ReqEvidence>,
    pub error: Option<String>,
    pub attempt_increment: u32,
}

pub fn evaluate_requirement(
    req: &Requirement,
    executor: &CommandExecutor,
    output_summary_max_chars: usize,
) -> EvalResult {
    match executor.run(&req.validate_command) {
        Ok(result) => {
            let condition = evaluate_pass_condition(&req.pass_condition, &result)
                .map_err(|err| err.to_string());
            let output_summary = result.output_summary(output_summary_max_chars);
            let evidence = ReqEvidence {
                command: result.command,
                exit_code: result.exit_code,
                output_summary,
            };

            match condition {
                Ok(check) if check.passed => EvalResult {
                    status: ReqStatus::Done,
                    evidence: Some(evidence),
                    error: None,
                    attempt_increment: result.attempt,
                },
                Ok(check) => EvalResult {
                    status: ReqStatus::Failed,
                    evidence: Some(evidence),
                    error: Some(format!(
                        "pass condition not satisfied: {} (condition='{}')",
                        check.reason, req.pass_condition
                    )),
                    attempt_increment: result.attempt,
                },
                Err(err) => EvalResult {
                    status: ReqStatus::Failed,
                    evidence: Some(evidence),
                    error: Some(format!("failed to evaluate pass condition: {}", err)),
                    attempt_increment: result.attempt,
                },
            }
        }
        Err(err) => EvalResult {
            status: ReqStatus::Failed,
            evidence: None,
            error: Some(err.to_string()),
            attempt_increment: 1,
        },
    }
}

pub fn evaluate_requirement_dry_run(req: &Requirement) -> EvalResult {
    EvalResult {
        status: ReqStatus::Done,
        evidence: Some(ReqEvidence {
            command: format!("{} [dry-run]", req.validate_command),
            exit_code: 0,
            output_summary: "dry-run: validation skipped".to_string(),
        }),
        error: None,
        attempt_increment: 0,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{evaluate_requirement, evaluate_requirement_dry_run};
    use crate::plugin::prd_runner::config::prd::Requirement;
    use crate::plugin::prd_runner::config::ExecutionConfig;
    use crate::plugin::prd_runner::core::executor::CommandExecutor;
    use crate::plugin::prd_runner::loop_engine::state::ReqStatus;

    fn requirement(command: &str, pass_condition: &str) -> Requirement {
        Requirement {
            id: "REQ-001".to_string(),
            title: "test".to_string(),
            priority: Some("high".to_string()),
            description: "desc".to_string(),
            validate_command: command.to_string(),
            pass_condition: pass_condition.to_string(),
            tasks: vec![],
        }
    }

    #[test]
    fn evaluates_pass_condition() {
        let tmp = TempDir::new().expect("tmp dir should be created");
        let config = ExecutionConfig {
            command_timeout: "1s".to_string(),
            max_retry: 0,
        };

        let executor = CommandExecutor::new(config, tmp.path()).expect("executor should be built");
        let req = requirement("echo coverage 85%", "contains coverage");
        let result = evaluate_requirement(&req, &executor, 400);
        assert_eq!(result.status, ReqStatus::Done);
    }

    #[test]
    fn returns_done_in_dry_run() {
        let req = requirement("echo no-op", "退出码 = 0");
        let result = evaluate_requirement_dry_run(&req);
        assert_eq!(result.status, ReqStatus::Done);
    }
}
