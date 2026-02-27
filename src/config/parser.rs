use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;

use super::prd::{AcceptanceCriterion, PrdDocument, Requirement};

pub fn parse_prd_file(path: &Path) -> Result<PrdDocument> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read PRD file {}", path.display()))?;
    parse_prd_str(&content)
}

pub fn parse_prd_str(markdown: &str) -> Result<PrdDocument> {
    let lines: Vec<&str> = markdown.lines().collect();

    let project_name = extract_project_name(&lines);
    let project_context = extract_section_text(&lines, "项目上下文")
        .ok_or_else(|| anyhow!("missing required section heading: 项目上下文"))?;
    let requirement_section = extract_section_text(&lines, "需求列表")
        .ok_or_else(|| anyhow!("missing required section heading: 需求列表"))?;
    let acceptance_section = extract_section_text(&lines, "验收标准")
        .ok_or_else(|| anyhow!("missing required section heading: 验收标准"))?;

    let requirements = parse_requirements(requirement_section.as_lines())?;
    let acceptance_criteria = parse_acceptance_criteria(acceptance_section.as_lines())?;

    let doc = PrdDocument {
        project_name,
        project_context: project_context.text,
        requirements,
        acceptance_criteria,
        raw_markdown: markdown.to_string(),
    };

    doc.validate()?;
    Ok(doc)
}

#[derive(Debug)]
struct Section {
    text: String,
}

impl Section {
    fn as_lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

fn extract_section_text(lines: &[&str], heading_keyword: &str) -> Option<Section> {
    let mut start = None;
    let mut end = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") && trimmed.contains(heading_keyword) {
            start = Some(idx + 1);
            continue;
        }

        if start.is_some() && trimmed.starts_with("## ") {
            end = idx;
            break;
        }
    }

    let start_idx = start?;
    let section_text = lines[start_idx..end].join("\n");
    Some(Section { text: section_text })
}

fn extract_project_name(lines: &[&str]) -> Option<String> {
    let regex = Regex::new(r"^>\s*项目名称[:：]\s*(.+?)\s*(?:<!--.*)?$").ok()?;
    for line in lines {
        let trimmed = line.trim();
        if let Some(caps) = regex.captures(trimmed) {
            return Some(caps[1].trim().to_string());
        }
    }
    None
}

fn parse_requirements(lines: Vec<&str>) -> Result<Vec<Requirement>> {
    let req_heading = Regex::new(r"^###\s*(REQ-[A-Za-z0-9_-]+)\s*:\s*(.+?)\s*$")
        .context("failed to compile requirement heading regex")?;

    let mut reqs = Vec::new();
    let mut current: Option<RequirementBuilder> = None;

    for line in lines {
        let trimmed = line.trim();

        if let Some(caps) = req_heading.captures(trimmed) {
            if let Some(builder) = current.take() {
                reqs.push(builder.build()?);
            }

            current = Some(RequirementBuilder {
                id: caps[1].trim().to_string(),
                title: caps[2].trim().to_string(),
                priority: None,
                description: None,
                validate_command: None,
                pass_condition: None,
                tasks: Vec::new(),
            });
            continue;
        }

        let Some(builder) = current.as_mut() else {
            continue;
        };

        if let Some((key, value)) = parse_two_column_row(trimmed) {
            let norm_key = strip_markdown(&key).to_lowercase();
            let norm_val = strip_markdown(&value);
            if norm_key.contains("优先级") {
                builder.priority = Some(norm_val);
            } else if norm_key.contains("描述") {
                builder.description = Some(norm_val);
            } else if norm_key.contains("验证命令") {
                builder.validate_command = Some(strip_code_fence(&norm_val));
            } else if norm_key.contains("通过条件") {
                builder.pass_condition = Some(norm_val);
            }
            continue;
        }

        if let Some(task) = parse_task(trimmed) {
            builder.tasks.push(task);
        }
    }

    if let Some(builder) = current.take() {
        reqs.push(builder.build()?);
    }

    if reqs.is_empty() {
        bail!("no REQ-* entries found in 需求列表 section");
    }

    Ok(reqs)
}

fn parse_acceptance_criteria(lines: Vec<&str>) -> Result<Vec<AcceptanceCriterion>> {
    let mut criteria = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        let Some(cells) = parse_table_row(trimmed) else {
            continue;
        };

        if cells.len() < 3 {
            continue;
        }

        if is_separator_row(&cells) {
            continue;
        }

        let first = strip_markdown(&cells[0]);
        if first.contains("标准") || first.contains("字段") {
            continue;
        }

        let name = first;
        let validate_command = strip_code_fence(&strip_markdown(&cells[1]));
        let pass_condition = strip_markdown(&cells[2]);

        if name.is_empty() || validate_command.is_empty() {
            continue;
        }

        criteria.push(AcceptanceCriterion {
            name,
            validate_command,
            pass_condition,
        });
    }

    if criteria.is_empty() {
        bail!("no acceptance criteria table rows found");
    }

    Ok(criteria)
}

fn parse_task(line: &str) -> Option<String> {
    if let Some(task) = line.strip_prefix("- [ ]") {
        let clean = task.trim();
        if !clean.is_empty() {
            return Some(clean.to_string());
        }
    }

    None
}

fn parse_two_column_row(line: &str) -> Option<(String, String)> {
    let cells = parse_table_row(line)?;
    if cells.len() < 2 || is_separator_row(&cells) {
        return None;
    }
    Some((cells[0].clone(), cells[1].clone()))
}

fn parse_table_row(line: &str) -> Option<Vec<String>> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return None;
    }

    let cells = line
        .trim_matches('|')
        .split('|')
        .map(|v| v.trim().to_string())
        .collect::<Vec<String>>();

    Some(cells)
}

fn is_separator_row(cells: &[String]) -> bool {
    cells
        .iter()
        .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':'))
}

fn strip_markdown(input: &str) -> String {
    input
        .replace("**", "")
        .replace('*', "")
        .replace("<!--", "")
        .replace("-->", "")
        .trim()
        .to_string()
}

fn strip_code_fence(input: &str) -> String {
    input.trim().trim_matches('`').trim().to_string()
}

#[derive(Debug)]
struct RequirementBuilder {
    id: String,
    title: String,
    priority: Option<String>,
    description: Option<String>,
    validate_command: Option<String>,
    pass_condition: Option<String>,
    tasks: Vec<String>,
}

impl RequirementBuilder {
    fn build(self) -> Result<Requirement> {
        let validate_command = self
            .validate_command
            .ok_or_else(|| anyhow!("{} missing 验证命令", self.id))?;
        let pass_condition = self
            .pass_condition
            .ok_or_else(|| anyhow!("{} missing 通过条件", self.id))?;

        Ok(Requirement {
            id: self.id,
            title: self.title.clone(),
            priority: self.priority,
            description: self.description.unwrap_or(self.title),
            validate_command,
            pass_condition,
            tasks: self.tasks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_prd_str;

    #[test]
    fn parse_minimal_prd() {
        let src = r#"
# PRD: test
> 项目名称：demo

## 1. 项目上下文
- type: rust

## 2. 需求列表
### REQ-001: build
| 字段 | 值 |
|------|-----|
| **优先级** | high |
| **描述** | run build |
| **验证命令** | `echo ok` |
| **通过条件** | 退出码 = 0 |

- [ ] do x

## 3. 验收标准
| 标准 | 验证命令 | 通过条件 |
|------|----------|----------|
| 构建成功 | `echo ok` | 退出码 = 0 |
"#;

        let doc = parse_prd_str(src).expect("expected parser to succeed");
        assert_eq!(doc.project_name.as_deref(), Some("demo"));
        assert_eq!(doc.requirements.len(), 1);
        assert_eq!(doc.requirements[0].id, "REQ-001");
        assert_eq!(doc.acceptance_criteria.len(), 1);
    }
}
