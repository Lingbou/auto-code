use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrdDocument {
    pub project_name: Option<String>,
    pub project_context: String,
    pub requirements: Vec<Requirement>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub raw_markdown: String,
}

impl PrdDocument {
    pub fn validate(&self) -> Result<()> {
        if self.project_context.trim().is_empty() {
            bail!("missing required section: 项目上下文");
        }

        if self.requirements.is_empty() {
            bail!("missing required section: 需求列表");
        }

        if self.acceptance_criteria.is_empty() {
            bail!("missing required section: 验收标准");
        }

        for req in &self.requirements {
            if req.id.trim().is_empty() {
                bail!("requirement has empty id");
            }
            if req.validate_command.trim().is_empty() {
                bail!("requirement {} missing 验证命令", req.id);
            }
            if req.pass_condition.trim().is_empty() {
                bail!("requirement {} missing 通过条件", req.id);
            }
        }

        for criterion in &self.acceptance_criteria {
            if criterion.validate_command.trim().is_empty() {
                bail!("acceptance criterion '{}' missing 验证命令", criterion.name);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub id: String,
    pub title: String,
    pub priority: Option<String>,
    pub description: String,
    pub validate_command: String,
    pub pass_condition: String,
    pub tasks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub name: String,
    pub validate_command: String,
    pub pass_condition: String,
}
