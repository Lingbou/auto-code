use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::logger::report::IterationReport;
use crate::loop_engine::state::EngineState;

#[derive(Debug, Clone)]
pub struct CheckpointManager {
    root: PathBuf,
    max_keep: usize,
}

impl CheckpointManager {
    pub fn new(root: impl AsRef<Path>, max_keep: usize) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("failed to create checkpoint root {}", root.display()))?;

        Ok(Self {
            root,
            max_keep: max_keep.max(1),
        })
    }

    pub fn save(
        &self,
        iteration: u32,
        prd_path: &Path,
        state: &EngineState,
        report: &IterationReport,
        workdir: &Path,
    ) -> Result<PathBuf> {
        let mut checkpoint_dir = self.root.join(format!("checkpoint_{:03}", iteration));
        if checkpoint_dir.exists() {
            checkpoint_dir = self.root.join(format!("checkpoint_{:03}_dup", iteration));
        }

        std::fs::create_dir_all(&checkpoint_dir).with_context(|| {
            format!(
                "failed to create checkpoint directory {}",
                checkpoint_dir.display()
            )
        })?;

        let state_path = checkpoint_dir.join("state.json");
        let report_path = checkpoint_dir.join("iteration_report.json");
        let prd_out = checkpoint_dir.join("prd.md");

        let state_content =
            serde_json::to_string_pretty(state).context("failed to serialize checkpoint state")?;
        std::fs::write(&state_path, state_content)
            .with_context(|| format!("failed to write {}", state_path.display()))?;

        let report_content = serde_json::to_string_pretty(report)
            .context("failed to serialize checkpoint report")?;
        std::fs::write(&report_path, report_content)
            .with_context(|| format!("failed to write {}", report_path.display()))?;

        std::fs::copy(prd_path, &prd_out).with_context(|| {
            format!(
                "failed to copy PRD {} -> {}",
                prd_path.display(),
                prd_out.display()
            )
        })?;

        let snapshot_dir = checkpoint_dir.join("code_snapshot");
        copy_workspace_snapshot(workdir, &snapshot_dir)?;

        self.prune_old_checkpoints()?;

        Ok(checkpoint_dir)
    }

    fn prune_old_checkpoints(&self) -> Result<()> {
        let mut entries = std::fs::read_dir(&self.root)
            .with_context(|| format!("failed to read checkpoint root {}", self.root.display()))?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .collect::<Vec<_>>();

        if entries.len() <= self.max_keep {
            return Ok(());
        }

        entries.sort_by_key(|entry| entry.file_name());

        let remove_count = entries.len().saturating_sub(self.max_keep);
        for entry in entries.into_iter().take(remove_count) {
            let path = entry.path();
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove old checkpoint {}", path.display()))?;
        }

        Ok(())
    }
}

fn copy_workspace_snapshot(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("failed to create snapshot dir {}", dst.display()))?;

    let walker = WalkDir::new(src).into_iter().filter_entry(|entry| {
        let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
        !should_skip(rel)
    });

    for entry in walker {
        let entry = entry.with_context(|| format!("failed to walk workspace {}", src.display()))?;
        let path = entry.path();
        if path == src {
            continue;
        }

        let rel = path
            .strip_prefix(src)
            .with_context(|| format!("failed to strip prefix for {}", path.display()))?;
        let out = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&out)
                .with_context(|| format!("failed to create snapshot dir {}", out.display()))?;
            continue;
        }

        if entry.file_type().is_file() {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create snapshot parent {}", parent.display())
                })?;
            }

            std::fs::copy(path, &out).with_context(|| {
                format!(
                    "failed to copy snapshot file {} -> {}",
                    path.display(),
                    out.display()
                )
            })?;
        }
    }

    Ok(())
}

fn should_skip(path: &Path) -> bool {
    let Some(first) = path.components().next() else {
        return false;
    };

    let name = first.as_os_str().to_string_lossy();
    matches!(
        name.as_ref(),
        ".git" | "target" | "logs" | "checkpoints" | ".autocode"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anyhow::Result;
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::logger::report::{IterationReport, ReqReport};
    use crate::loop_engine::state::EngineState;

    use super::CheckpointManager;

    #[test]
    fn saves_checkpoint_files() -> Result<()> {
        let workspace = TempDir::new()?;
        let checkpoints = TempDir::new()?;
        let prd_path = workspace.path().join("prd.md");
        std::fs::write(&prd_path, "# PRD")?;
        std::fs::write(workspace.path().join("README.md"), "demo")?;
        std::fs::create_dir_all(workspace.path().join(".autocode/checkpoints/nested"))?;
        std::fs::write(
            workspace
                .path()
                .join(".autocode/checkpoints/nested/marker.txt"),
            "skip me",
        )?;

        let state = EngineState {
            iteration: 1,
            req_status: BTreeMap::new(),
        };
        let report = IterationReport {
            iteration: 1,
            timestamp: Utc::now(),
            duration_seconds: 1,
            req_status: BTreeMap::from([(
                "REQ-001".to_string(),
                ReqReport {
                    status: "done".to_string(),
                    progress: Some(1.0),
                    evidence: None,
                    error: None,
                },
            )]),
            overall_progress: 1.0,
            has_progress: true,
            next_actions: vec![],
        };

        let manager = CheckpointManager::new(checkpoints.path(), 2)?;
        let path = manager.save(1, &prd_path, &state, &report, workspace.path())?;

        assert!(path.join("state.json").exists());
        assert!(path.join("iteration_report.json").exists());
        assert!(path.join("prd.md").exists());
        assert!(path.join("code_snapshot/README.md").exists());
        assert!(!path.join("code_snapshot/.autocode").exists());
        Ok(())
    }
}
