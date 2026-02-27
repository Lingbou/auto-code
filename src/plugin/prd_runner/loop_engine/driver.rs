use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tracing::info;

use crate::plugin::prd_runner::checkpoint::saver::CheckpointManager;
use crate::plugin::prd_runner::config::prd::PrdDocument;
use crate::plugin::prd_runner::config::AppConfig;
use crate::plugin::prd_runner::core::executor::CommandExecutor;
use crate::plugin::prd_runner::core::process::AiProcess;
use crate::plugin::prd_runner::core::provider::CliPrintProvider;
use crate::plugin::prd_runner::logger::report::{
    write_report, EvidenceReport, IterationReport, ReqReport,
};
use crate::plugin::prd_runner::logger::writer::LogWriter;
use crate::plugin::prd_runner::loop_engine::convergence::ConvergenceGuard;
use crate::plugin::prd_runner::loop_engine::evaluator::{
    evaluate_requirement, evaluate_requirement_dry_run,
};
use crate::plugin::prd_runner::loop_engine::pass_condition::evaluate_pass_condition;
use crate::plugin::prd_runner::loop_engine::state::{EngineState, ReqStatus};
use crate::runtime::signal;

#[derive(Debug, Clone)]
pub struct EngineRuntime {
    pub prd: PrdDocument,
    pub prd_path: PathBuf,
    pub config: AppConfig,
    pub workdir: PathBuf,
    pub log_dir: PathBuf,
    pub checkpoint_root: PathBuf,
    pub provider_command: String,
    pub provider_timeout: Duration,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub completed: bool,
    pub iterations: u32,
    pub stop_reason: Option<String>,
    pub last_checkpoint: Option<PathBuf>,
}

impl EngineRuntime {
    pub fn run(self, resume_state: Option<EngineState>) -> Result<RunSummary> {
        let mut logger = LogWriter::new(
            &self.log_dir,
            self.config.logging.max_file_size_bytes,
            self.config.logging.max_rotated_files,
        )?;
        logger.log_session("session initialized")?;
        logger.save_prd_snapshot(&self.prd.raw_markdown)?;
        logger.log_event(
            "START",
            &format!(
                "engine started, project={}, workdir={}, provider={}, dry_run={}",
                self.prd.project_name.as_deref().unwrap_or("unknown"),
                self.workdir.display(),
                self.provider_command,
                self.dry_run
            ),
        )?;

        let executor = CommandExecutor::new(self.config.execution.clone(), &self.workdir)?;
        let provider = CliPrintProvider::new(self.provider_command.clone(), &self.workdir);
        let mut ai_process =
            AiProcess::new(Box::new(provider), self.provider_timeout, self.dry_run)?;
        let mut state = resume_state.unwrap_or_else(|| EngineState::new(&self.prd.requirements));

        let convergence = ConvergenceGuard::new(self.config.convergence.clone())?;
        let checkpoint_manager = if self.config.checkpoint.enabled {
            Some(CheckpointManager::new(
                &self.checkpoint_root,
                self.config.checkpoint.max_checkpoints,
            )?)
        } else {
            None
        };

        let mut last_checkpoint = None;
        let mut acceptance_passed = false;
        let output_summary_limit = self.config.logging.output_summary_max_chars;

        loop {
            if signal::interrupted() {
                let reason = "received Ctrl+C".to_string();
                logger.log_event("STOP", &reason)?;
                return Ok(RunSummary {
                    completed: is_run_completed(&state, acceptance_passed),
                    iterations: state.iteration,
                    stop_reason: Some(reason),
                    last_checkpoint,
                });
            }

            if let Some(reason) = convergence.check() {
                logger.log_event("STOP", &reason.to_string())?;
                return Ok(RunSummary {
                    completed: is_run_completed(&state, acceptance_passed),
                    iterations: state.iteration,
                    stop_reason: Some(reason.to_string()),
                    last_checkpoint,
                });
            }

            state.iteration = state.iteration.saturating_add(1);
            let iter_started = Instant::now();
            let previous_status = state.status_snapshot();

            logger.log_event("ITERATION_START", &format!("iteration={}", state.iteration))?;
            info!(iteration = state.iteration, "iteration started");

            for (req_idx, req) in self.prd.requirements.iter().enumerate() {
                if signal::interrupted() {
                    let reason = "received Ctrl+C".to_string();
                    logger.log_event("STOP", &reason)?;
                    return Ok(RunSummary {
                        completed: is_run_completed(&state, acceptance_passed),
                        iterations: state.iteration,
                        stop_reason: Some(reason),
                        last_checkpoint,
                    });
                }

                state.mark_in_progress(&req.id);
                let remaining_runtime = convergence.remaining();
                if remaining_runtime.is_zero() {
                    let reason = "reached max_runtime (0s remaining)".to_string();
                    logger.log_event("STOP", &reason)?;
                    return Ok(RunSummary {
                        completed: is_run_completed(&state, acceptance_passed),
                        iterations: state.iteration,
                        stop_reason: Some(reason),
                        last_checkpoint,
                    });
                }

                let pending_requirements = self.prd.requirements.len().saturating_sub(req_idx);
                let fair_share_timeout = if pending_requirements > 0 {
                    remaining_runtime / pending_requirements as u32
                } else {
                    remaining_runtime
                };
                let fair_share_timeout = fair_share_timeout.max(Duration::from_secs(1));
                let effective_provider_timeout =
                    std::cmp::min(self.provider_timeout, fair_share_timeout);
                ai_process.set_response_timeout(effective_provider_timeout);
                logger.log_event(
                    "AI_PROVIDER_START",
                    &format!(
                        "iteration={} req={} timeout_secs={} fair_share_secs={} remaining_runtime_secs={} pending_requirements={}",
                        state.iteration,
                        req.id,
                        effective_provider_timeout.as_secs(),
                        fair_share_timeout.as_secs(),
                        remaining_runtime.as_secs(),
                        pending_requirements
                    ),
                )?;
                info!(
                    iteration = state.iteration,
                    req = %req.id,
                    provider_timeout_s = effective_provider_timeout.as_secs(),
                    fair_share_timeout_s = fair_share_timeout.as_secs(),
                    pending_requirements,
                    remaining_runtime_s = remaining_runtime.as_secs(),
                    "requesting provider output"
                );

                let prompt = ai_process.build_prompt(&self.prd, req, &self.workdir);
                let instruction = match ai_process.generate_instruction(&prompt) {
                    Ok(instruction) => instruction,
                    Err(err) => {
                        let err_chain = err
                            .chain()
                            .map(|cause| cause.to_string())
                            .collect::<Vec<_>>()
                            .join(" | caused by: ");
                        logger.log_event(
                            "AI_ERROR",
                            &format!(
                                "iteration={} req={} error={}",
                                state.iteration, req.id, err_chain
                            ),
                        )?;
                        info!(
                            iteration = state.iteration,
                            req = %req.id,
                            "provider returned error"
                        );
                        if let Some(stop_reason) = fatal_provider_stop_reason(&err_chain) {
                            logger.log_event(
                                "STOP",
                                &format!(
                                    "iteration={} req={} {}",
                                    state.iteration, req.id, stop_reason
                                ),
                            )?;
                            return Ok(RunSummary {
                                completed: false,
                                iterations: state.iteration,
                                stop_reason: Some(stop_reason),
                                last_checkpoint,
                            });
                        }
                        crate::plugin::prd_runner::core::process::AiInstruction {
                            raw_output: format!("[provider-error] {}", err_chain),
                            commands: Vec::new(),
                        }
                    }
                };
                logger.log_ai(&format!(
                    "[ITER_{}][{}][provider={}] {}",
                    state.iteration,
                    req.id,
                    ai_process.provider_name(),
                    instruction.raw_output
                ))?;
                info!(
                    iteration = state.iteration,
                    req = %req.id,
                    command_count = instruction.commands.len(),
                    "provider response parsed"
                );
                if instruction.commands.is_empty() {
                    logger.log_event(
                        "AI_NO_COMMANDS",
                        &format!(
                            "iteration={} req={} provider={} output_preview='{}'",
                            state.iteration,
                            req.id,
                            ai_process.provider_name(),
                            instruction
                                .raw_output
                                .chars()
                                .take(200)
                                .collect::<String>()
                                .replace('\n', "\\n")
                        ),
                    )?;
                }

                for command in &instruction.commands {
                    if self.dry_run {
                        logger.log_event(
                            "AI_CMD_DRY_RUN",
                            &format!(
                                "iteration={} req={} command='{}'",
                                state.iteration, req.id, command
                            ),
                        )?;
                        continue;
                    }

                    logger.log_event(
                        "AI_CMD_EXEC",
                        &format!(
                            "iteration={} req={} command='{}'",
                            state.iteration, req.id, command
                        ),
                    )?;
                    info!(
                        iteration = state.iteration,
                        req = %req.id,
                        command = %command,
                        "executing AI command"
                    );

                    match executor.run(command) {
                        Ok(cmd_result) => {
                            logger.log_terminal(&format!(
                                "[ITER_{}][{}] ai_cmd='{}' exit={} output={}",
                                state.iteration,
                                req.id,
                                command,
                                cmd_result.exit_code,
                                cmd_result.output_summary(output_summary_limit)
                            ))?;
                        }
                        Err(err) => {
                            logger.log_event(
                                "AI_CMD_FAIL",
                                &format!(
                                    "iteration={} req={} command='{}' error={}",
                                    state.iteration, req.id, command, err
                                ),
                            )?;
                        }
                    }
                }

                let eval = if self.dry_run {
                    evaluate_requirement_dry_run(req)
                } else {
                    evaluate_requirement(req, &executor, output_summary_limit)
                };
                let req_status = eval.status;
                let req_error = eval.error.clone();

                if let Some(evidence) = &eval.evidence {
                    logger.log_terminal(&format!(
                        "[ITER_{}][{}] cmd='{}' exit={} output={}",
                        state.iteration,
                        req.id,
                        evidence.command,
                        evidence.exit_code,
                        evidence.output_summary
                    ))?;
                }

                if let Some(error) = &req_error {
                    logger.log_event(
                        "REQ_FAILED",
                        &format!(
                            "iteration={} req={} error={}",
                            state.iteration, req.id, error
                        ),
                    )?;
                }

                state.update(
                    &req.id,
                    req_status,
                    eval.evidence,
                    req_error,
                    eval.attempt_increment,
                );
            }

            let failed_criteria = run_acceptance_checks(
                &executor,
                &self.prd.acceptance_criteria,
                &mut logger,
                state.iteration,
                self.dry_run,
            )?;
            acceptance_passed = failed_criteria.is_empty();
            if failed_criteria.is_empty() {
                logger.log_event(
                    "ACCEPTANCE_STATUS",
                    &format!(
                        "iteration={} all acceptance criteria passed",
                        state.iteration
                    ),
                )?;
            } else {
                logger.log_event(
                    "ACCEPTANCE_STATUS",
                    &format!(
                        "iteration={} failed criteria: {}",
                        state.iteration,
                        failed_criteria.join(", ")
                    ),
                )?;
            }

            let has_progress = state.has_progress_since(&previous_status);

            let report = build_iteration_report(
                state.iteration,
                &state,
                has_progress,
                iter_started.elapsed().as_secs(),
            );

            let report_path = self.log_dir.join("iteration_report.json");
            write_report(&report_path, &report)
                .with_context(|| format!("failed to write report {}", report_path.display()))?;

            logger.log_event(
                "ITERATION_END",
                &format!(
                    "iteration={} progress={:.2} has_progress={}",
                    state.iteration,
                    state.overall_progress(),
                    has_progress
                ),
            )?;
            info!(
                iteration = state.iteration,
                progress = state.overall_progress(),
                has_progress,
                "iteration finished"
            );

            if let Some(checkpoint_manager) = &checkpoint_manager {
                let checkpoint = checkpoint_manager.save(
                    state.iteration,
                    &self.prd_path,
                    &state,
                    &report,
                    &self.workdir,
                )?;
                last_checkpoint = Some(checkpoint.clone());
                logger.log_event(
                    "CHECKPOINT",
                    &format!("saved checkpoint {}", checkpoint.display()),
                )?;
            }
        }
    }
}

fn run_acceptance_checks(
    executor: &CommandExecutor,
    criteria: &[crate::plugin::prd_runner::config::prd::AcceptanceCriterion],
    logger: &mut LogWriter,
    iteration: u32,
    dry_run: bool,
) -> Result<Vec<String>> {
    let mut failed = Vec::new();

    for criterion in criteria {
        if dry_run {
            logger.log_event(
                "ACCEPTANCE_DRY_RUN",
                &format!(
                    "iteration={} criterion='{}' command='{}'",
                    iteration, criterion.name, criterion.validate_command
                ),
            )?;
            continue;
        }

        let result = executor.run(&criterion.validate_command);
        match result {
            Ok(output) => match evaluate_pass_condition(&criterion.pass_condition, &output) {
                Ok(evaluation) if evaluation.passed => {
                    logger.log_event(
                        "ACCEPTANCE_PASS",
                        &format!(
                            "iteration={} criterion='{}' command='{}'",
                            iteration, criterion.name, criterion.validate_command
                        ),
                    )?;
                }
                Ok(evaluation) => {
                    logger.log_event(
                        "ACCEPTANCE_FAIL",
                        &format!(
                            "iteration={} criterion='{}' reason={} command='{}'",
                            iteration,
                            criterion.name,
                            evaluation.reason,
                            criterion.validate_command
                        ),
                    )?;
                    failed.push(criterion.name.clone());
                }
                Err(err) => {
                    logger.log_event(
                        "ACCEPTANCE_FAIL",
                        &format!(
                            "iteration={} criterion='{}' error={} command='{}'",
                            iteration, criterion.name, err, criterion.validate_command
                        ),
                    )?;
                    failed.push(criterion.name.clone());
                }
            },
            Err(err) => {
                logger.log_event(
                    "ACCEPTANCE_FAIL",
                    &format!(
                        "iteration={} criterion='{}' error={} command='{}'",
                        iteration, criterion.name, err, criterion.validate_command
                    ),
                )?;
                failed.push(criterion.name.clone());
            }
        }
    }

    Ok(failed)
}

fn build_iteration_report(
    iteration: u32,
    state: &EngineState,
    has_progress: bool,
    duration_seconds: u64,
) -> IterationReport {
    let mut req_status = BTreeMap::new();

    for (req_id, record) in &state.req_status {
        let progress = match record.status {
            ReqStatus::Done => Some(1.0),
            ReqStatus::InProgress => Some(0.5),
            _ => Some(0.0),
        };

        let evidence = record.evidence.as_ref().map(|e| EvidenceReport {
            command: e.command.clone(),
            exit_code: e.exit_code,
            output_summary: e.output_summary.clone(),
        });

        req_status.insert(
            req_id.clone(),
            ReqReport {
                status: record.status.as_str().to_string(),
                progress,
                evidence,
                error: record.last_error.clone(),
            },
        );
    }

    let next_actions = state
        .req_status
        .iter()
        .filter(|(_, record)| record.status != ReqStatus::Done)
        .take(3)
        .map(|(id, _)| format!("continue working on {}", id))
        .collect::<Vec<String>>();

    IterationReport {
        iteration,
        timestamp: Utc::now(),
        duration_seconds,
        req_status,
        overall_progress: state.overall_progress(),
        has_progress,
        next_actions,
    }
}

fn is_run_completed(state: &EngineState, acceptance_passed: bool) -> bool {
    state.all_done() && acceptance_passed
}

fn fatal_provider_stop_reason(err: &str) -> Option<String> {
    let lower = err.to_ascii_lowercase();

    if lower.contains("not logged in")
        || lower.contains("please run /login")
        || lower.contains("run /login")
    {
        return Some(
            "provider authentication required. run `claude /login` (or provider login) and rerun."
                .to_string(),
        );
    }

    if lower.contains("failed to spawn provider process")
        && (lower.contains("no such file or directory") || lower.contains("not found"))
    {
        return Some(
            "provider command not found in PATH. install/configure provider cli and rerun."
                .to_string(),
        );
    }

    None
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Result;
    use tempfile::TempDir;

    use crate::plugin::prd_runner::config::prd::{AcceptanceCriterion, PrdDocument, Requirement};
    use crate::plugin::prd_runner::config::AppConfig;
    use crate::plugin::prd_runner::loop_engine::driver::{
        fatal_provider_stop_reason, is_run_completed, EngineRuntime,
    };
    use crate::plugin::prd_runner::loop_engine::state::{EngineState, ReqRecord, ReqStatus};

    #[test]
    fn run_stops_when_runtime_limit_reached() -> Result<()> {
        let tmp = TempDir::new()?;
        let prd_path = tmp.path().join("prd.md");
        std::fs::write(&prd_path, "# PRD")?;

        let prd = PrdDocument {
            project_name: Some("demo".to_string()),
            project_context: "ctx".to_string(),
            requirements: vec![Requirement {
                id: "REQ-001".to_string(),
                title: "echo".to_string(),
                priority: Some("high".to_string()),
                description: "run echo".to_string(),
                validate_command: "echo ok".to_string(),
                pass_condition: "退出码 = 0".to_string(),
                tasks: vec!["echo".to_string()],
            }],
            acceptance_criteria: vec![AcceptanceCriterion {
                name: "echo".to_string(),
                validate_command: "echo ok".to_string(),
                pass_condition: "退出码 = 0".to_string(),
            }],
            raw_markdown: "# PRD".to_string(),
        };

        let mut config = AppConfig::default();
        config.convergence.max_runtime = "0s".to_string();
        config.checkpoint.enabled = false;

        let runtime = EngineRuntime {
            prd,
            prd_path,
            config,
            workdir: tmp.path().to_path_buf(),
            log_dir: tmp.path().join("logs"),
            checkpoint_root: tmp.path().join("checkpoints"),
            provider_command: "cat".to_string(),
            provider_timeout: Duration::from_secs(1),
            dry_run: true,
        };

        let summary = runtime.run(None)?;
        assert!(matches!(summary.stop_reason, Some(reason) if reason.contains("max_runtime")));
        Ok(())
    }

    #[test]
    fn detects_authentication_provider_error() {
        let err = "failed to read output from provider | caused by: claude returned error: Not logged in · Please run /login";
        let reason = fatal_provider_stop_reason(err);
        assert!(reason.is_some());
    }

    #[test]
    fn does_not_treat_rate_limit_as_fatal() {
        let err = "failed to read output from provider | caused by: claude returned error: API Error: 429";
        let reason = fatal_provider_stop_reason(err);
        assert!(reason.is_none());
    }

    #[test]
    fn completion_requires_acceptance_to_pass() {
        let mut state = EngineState {
            iteration: 1,
            req_status: std::collections::BTreeMap::new(),
        };
        state.req_status.insert(
            "REQ-001".to_string(),
            ReqRecord {
                status: ReqStatus::Done,
                attempts: 1,
                evidence: None,
                last_error: None,
            },
        );

        assert!(is_run_completed(&state, true));
        assert!(!is_run_completed(&state, false));
    }
}
