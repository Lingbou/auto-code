use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

#[derive(Debug)]
struct LogFile {
    path: PathBuf,
    file: File,
}

#[derive(Debug)]
pub struct LogWriter {
    pub dir: PathBuf,
    session: LogFile,
    ai_output: LogFile,
    terminal_output: LogFile,
    events: LogFile,
    prd_snapshot: LogFile,
    max_file_size_bytes: u64,
    max_rotated_files: usize,
}

impl LogWriter {
    pub fn new(
        dir: impl AsRef<Path>,
        max_file_size_bytes: u64,
        max_rotated_files: usize,
    ) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create log dir {}", dir.display()))?;

        let session = open_log_file(dir.join("session.log"))?;
        let ai_output = open_log_file(dir.join("ai_output.log"))?;
        let terminal_output = open_log_file(dir.join("terminal_output.log"))?;
        let events = open_log_file(dir.join("events.log"))?;
        let prd_snapshot = open_log_file(dir.join("prd_snapshot.log"))?;

        Ok(Self {
            dir,
            session,
            ai_output,
            terminal_output,
            events,
            prd_snapshot,
            max_file_size_bytes,
            max_rotated_files,
        })
    }

    pub fn log_session(&mut self, message: &str) -> Result<()> {
        write_line(
            &mut self.session,
            "SESSION",
            message,
            self.max_file_size_bytes,
            self.max_rotated_files,
        )
    }

    pub fn log_ai(&mut self, message: &str) -> Result<()> {
        write_line(
            &mut self.ai_output,
            "AI",
            message,
            self.max_file_size_bytes,
            self.max_rotated_files,
        )
    }

    pub fn log_terminal(&mut self, message: &str) -> Result<()> {
        write_line(
            &mut self.terminal_output,
            "TERMINAL",
            message,
            self.max_file_size_bytes,
            self.max_rotated_files,
        )
    }

    pub fn log_event(&mut self, event: &str, message: &str) -> Result<()> {
        write_line(
            &mut self.events,
            event,
            message,
            self.max_file_size_bytes,
            self.max_rotated_files,
        )
    }

    pub fn save_prd_snapshot(&mut self, prd_markdown: &str) -> Result<()> {
        write_line(
            &mut self.prd_snapshot,
            "PRD_SNAPSHOT_START",
            "----------------------------------------",
            self.max_file_size_bytes,
            self.max_rotated_files,
        )?;
        write_line(
            &mut self.prd_snapshot,
            "PRD_CONTENT",
            prd_markdown,
            self.max_file_size_bytes,
            self.max_rotated_files,
        )?;
        write_line(
            &mut self.prd_snapshot,
            "PRD_SNAPSHOT_END",
            "----------------------------------------",
            self.max_file_size_bytes,
            self.max_rotated_files,
        )
    }
}

fn open_log_file(path: PathBuf) -> Result<LogFile> {
    let file = open_append(&path)?;
    Ok(LogFile { path, file })
}

fn open_append(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open log file {}", path.display()))
}

fn write_line(
    log_file: &mut LogFile,
    kind: &str,
    message: &str,
    max_file_size_bytes: u64,
    max_rotated_files: usize,
) -> Result<()> {
    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let line = format!("[{}] [{}] {}\n", ts, kind, message);

    rotate_if_needed(
        log_file,
        line.len() as u64,
        max_file_size_bytes,
        max_rotated_files,
    )?;

    log_file
        .file
        .write_all(line.as_bytes())
        .context("failed to write log line")?;
    log_file.file.flush().context("failed to flush log file")
}

fn rotate_if_needed(
    log_file: &mut LogFile,
    incoming_len: u64,
    max_file_size_bytes: u64,
    max_rotated_files: usize,
) -> Result<()> {
    if max_file_size_bytes == 0 || max_rotated_files == 0 {
        return Ok(());
    }

    let current_size = log_file
        .file
        .metadata()
        .with_context(|| format!("failed to read metadata for {}", log_file.path.display()))?
        .len();

    if current_size.saturating_add(incoming_len) <= max_file_size_bytes {
        return Ok(());
    }

    rotate_file_chain(&log_file.path, max_rotated_files)?;
    log_file.file = open_append(&log_file.path)?;
    Ok(())
}

fn rotate_file_chain(base_path: &Path, max_rotated_files: usize) -> Result<()> {
    for index in (1..=max_rotated_files).rev() {
        let dst = rotated_path(base_path, index);
        if dst.exists() {
            std::fs::remove_file(&dst)
                .with_context(|| format!("failed to remove old rotated log {}", dst.display()))?;
        }

        let src = if index == 1 {
            base_path.to_path_buf()
        } else {
            rotated_path(base_path, index - 1)
        };

        if src.exists() {
            std::fs::rename(&src, &dst).with_context(|| {
                format!(
                    "failed to rotate log file {} -> {}",
                    src.display(),
                    dst.display()
                )
            })?;
        }
    }

    Ok(())
}

fn rotated_path(base_path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", base_path.display(), index))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::TempDir;

    use super::LogWriter;

    #[test]
    fn writes_expected_log_files() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut writer = LogWriter::new(tmp.path(), 1024 * 1024, 3)?;
        writer.log_session("session started")?;
        writer.log_event("TEST", "event happened")?;
        writer.save_prd_snapshot("# PRD\ncontent")?;

        let session = std::fs::read_to_string(tmp.path().join("session.log"))?;
        let events = std::fs::read_to_string(tmp.path().join("events.log"))?;
        let prd = std::fs::read_to_string(tmp.path().join("prd_snapshot.log"))?;

        assert!(session.contains("session started"));
        assert!(events.contains("event happened"));
        assert!(prd.contains("# PRD"));
        Ok(())
    }

    #[test]
    fn rotates_when_file_reaches_limit() -> Result<()> {
        let tmp = TempDir::new()?;
        let mut writer = LogWriter::new(tmp.path(), 120, 2)?;

        for _ in 0..20 {
            writer.log_event("TEST", "abcdefghijklmnopqrstuvwxyz")?;
        }

        assert!(tmp.path().join("events.log").exists());
        assert!(tmp.path().join("events.log.1").exists());
        Ok(())
    }
}
