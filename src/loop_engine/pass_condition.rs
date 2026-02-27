use anyhow::{Context, Result};
use regex::Regex;

use crate::core::executor::CommandResult;

#[derive(Debug, Clone)]
pub struct ConditionEvaluation {
    pub passed: bool,
    pub reason: String,
}

pub fn evaluate_pass_condition(
    condition: &str,
    result: &CommandResult,
) -> Result<ConditionEvaluation> {
    let normalized = condition.trim();
    if normalized.is_empty() {
        return Ok(ConditionEvaluation {
            passed: result.success(),
            reason: format!(
                "default check (exit_code={}, timed_out={})",
                result.exit_code, result.timed_out
            ),
        });
    }

    if let Some(expected_exit_code) = parse_exit_code(normalized)? {
        let passed = result.exit_code == expected_exit_code && !result.timed_out;
        return Ok(ConditionEvaluation {
            passed,
            reason: format!(
                "expect exit code {}, got {}",
                expected_exit_code, result.exit_code
            ),
        });
    }

    if let Some(coverage_required) = parse_coverage_threshold(normalized)? {
        let actual = extract_coverage_value(&format!("{}\n{}", result.stdout, result.stderr));
        let passed = actual
            .map(|value| value + f64::EPSILON >= coverage_required)
            .unwrap_or(false);
        return Ok(ConditionEvaluation {
            passed,
            reason: match actual {
                Some(value) => format!(
                    "expect coverage >= {:.2}, got {:.2}",
                    coverage_required, value
                ),
                None => format!(
                    "expect coverage >= {:.2}, no coverage found",
                    coverage_required
                ),
            },
        });
    }

    if is_clippy_warning_rule(normalized) {
        let merged = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
        let passed = result.exit_code == 0 && !merged.contains("warning:");
        return Ok(ConditionEvaluation {
            passed,
            reason: "expect no clippy warnings".to_string(),
        });
    }

    if let Some(needle) = parse_output_contains(normalized)? {
        let merged = format!("{}\n{}", result.stdout, result.stderr);
        let passed = merged.contains(&needle);
        return Ok(ConditionEvaluation {
            passed,
            reason: format!("expect output contains '{}'", needle),
        });
    }

    Ok(ConditionEvaluation {
        passed: result.success(),
        reason: format!(
            "fallback check (condition='{}', exit_code={})",
            normalized, result.exit_code
        ),
    })
}

fn parse_exit_code(condition: &str) -> Result<Option<i32>> {
    let regex = Regex::new(r"(?i)(?:退出码|exit\s*code)\s*[=:]\s*(-?\d+)")
        .context("failed to compile exit code regex")?;

    let value = regex
        .captures(condition)
        .and_then(|caps| caps.get(1).map(|m| m.as_str()))
        .map(str::parse::<i32>)
        .transpose()
        .context("failed to parse exit code in pass condition")?;

    Ok(value)
}

fn parse_coverage_threshold(condition: &str) -> Result<Option<f64>> {
    let regex = Regex::new(r"(?i)(?:覆盖率|coverage)\s*(?:>=|≥)\s*(\d+(?:\.\d+)?)")
        .context("failed to compile coverage regex")?;

    let value = regex
        .captures(condition)
        .and_then(|caps| caps.get(1).map(|m| m.as_str()))
        .map(str::parse::<f64>)
        .transpose()
        .context("failed to parse coverage threshold in pass condition")?;

    Ok(value)
}

fn parse_output_contains(condition: &str) -> Result<Option<String>> {
    let regex = Regex::new(r#"(?i)(?:包含|contains)\s*['"`]?(.+?)['"`]?$"#)
        .context("failed to compile contains regex")?;

    let value = regex
        .captures(condition)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
        .filter(|text| !text.is_empty());

    Ok(value)
}

fn is_clippy_warning_rule(condition: &str) -> bool {
    let lower = condition.to_ascii_lowercase();
    lower.contains("无 clippy 警告") || lower.contains("no clippy warning")
}

fn extract_coverage_value(output: &str) -> Option<f64> {
    let regex = Regex::new(r"(?i)(?:coverage|覆盖率)[^0-9]{0,20}(\d+(?:\.\d+)?)\s*%?").ok()?;
    regex
        .captures_iter(output)
        .last()
        .and_then(|caps| caps.get(1).map(|m| m.as_str()))
        .and_then(|raw| raw.parse::<f64>().ok())
}

#[cfg(test)]
mod tests {
    use super::evaluate_pass_condition;
    use crate::core::executor::CommandResult;

    fn sample_result(stdout: &str, stderr: &str, exit_code: i32) -> CommandResult {
        CommandResult {
            command: "echo test".to_string(),
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            duration_ms: 5,
            timed_out: false,
            attempt: 1,
        }
    }

    #[test]
    fn checks_exit_code_rule() {
        let result = sample_result("ok", "", 0);
        let check = evaluate_pass_condition("退出码 = 0", &result).expect("must evaluate");
        assert!(check.passed);
    }

    #[test]
    fn checks_coverage_rule() {
        let result = sample_result("Test coverage: 83.5%", "", 0);
        let check = evaluate_pass_condition("覆盖率 >= 80", &result).expect("must evaluate");
        assert!(check.passed);
    }

    #[test]
    fn checks_clippy_rule() {
        let result = sample_result("", "warning: this is bad", 0);
        let check = evaluate_pass_condition("无 clippy 警告", &result).expect("must evaluate");
        assert!(!check.passed);
    }
}
