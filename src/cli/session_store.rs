use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredRole {
    User,
    Assistant,
    System,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: StoredRole,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTranscript {
    pub id: String,
    pub workdir: String,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<SessionMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SessionIndex {
    latest: Option<String>,
    sessions: Vec<SessionMeta>,
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub transcript: SessionTranscript,
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
    index_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct OpenSessionOptions<'a> {
    pub continue_last: bool,
    pub session_id: Option<&'a str>,
}

impl SessionStore {
    pub fn new(workdir: &Path) -> Result<Self> {
        let root = workdir.join(".autocode").join("sessions");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("failed to create session root {}", root.display()))?;
        let index_path = root.join("index.json");
        if !index_path.exists() {
            let content = serde_json::to_string_pretty(&SessionIndex::default())
                .context("failed to serialize default session index")?;
            std::fs::write(&index_path, content)
                .with_context(|| format!("failed to write {}", index_path.display()))?;
        }
        Ok(Self { root, index_path })
    }

    pub fn open_or_create(
        &self,
        options: OpenSessionOptions<'_>,
        provider: &str,
        workdir: &Path,
    ) -> Result<SessionHandle> {
        if let Some(session_id) = options.session_id {
            return self.load(session_id);
        }

        if options.continue_last {
            if let Some(last_id) = self.read_index()?.latest {
                return self.load(&last_id);
            }
        }

        self.create(provider, workdir)
    }

    pub fn create(&self, provider: &str, workdir: &Path) -> Result<SessionHandle> {
        let now = Utc::now();
        let mut id = format!("ses_{}", now.format("%Y%m%d_%H%M%S_%3f"));
        if self.session_file(&id).exists() {
            let mut suffix = 1usize;
            loop {
                let candidate = format!("{}_{}", id, suffix);
                if !self.session_file(&candidate).exists() {
                    id = candidate;
                    break;
                }
                suffix = suffix.saturating_add(1);
            }
        }
        let transcript = SessionTranscript {
            id: id.clone(),
            workdir: workdir.display().to_string(),
            provider: provider.to_string(),
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
        };
        self.write_transcript(&transcript)?;
        self.upsert_meta(&transcript, derive_title(&transcript.messages), true)?;
        Ok(SessionHandle { id, transcript })
    }

    pub fn load(&self, id: &str) -> Result<SessionHandle> {
        let path = self.session_file(id);
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read session {}", path.display()))?;
        let transcript: SessionTranscript = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse session {}", path.display()))?;
        self.touch_latest(&transcript.id)?;
        Ok(SessionHandle {
            id: transcript.id.clone(),
            transcript,
        })
    }

    pub fn append_message(&self, session_id: &str, role: StoredRole, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut handle = self.load(session_id)?;
        handle.transcript.messages.push(SessionMessage {
            role,
            text: text.to_string(),
            timestamp: Utc::now(),
        });
        handle.transcript.updated_at = Utc::now();
        self.write_transcript(&handle.transcript)?;
        self.upsert_meta(
            &handle.transcript,
            derive_title(&handle.transcript.messages),
            true,
        )?;
        Ok(())
    }

    pub fn set_provider(&self, session_id: &str, provider: &str) -> Result<()> {
        let mut handle = self.load(session_id)?;
        handle.transcript.provider = provider.to_string();
        handle.transcript.updated_at = Utc::now();
        self.write_transcript(&handle.transcript)?;
        self.upsert_meta(
            &handle.transcript,
            derive_title(&handle.transcript.messages),
            true,
        )?;
        Ok(())
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<SessionMeta>> {
        let mut sessions = self.read_index()?.sessions;
        sessions.sort_by_key(|meta| meta.updated_at);
        sessions.reverse();
        if limit == 0 || sessions.len() <= limit {
            return Ok(sessions);
        }
        Ok(sessions.into_iter().take(limit).collect())
    }

    pub fn delete(&self, session_id: &str) -> Result<()> {
        let mut index = self.read_index()?;
        let before = index.sessions.len();
        index.sessions.retain(|v| v.id != session_id);
        if before == index.sessions.len() {
            bail!("session not found: {}", session_id);
        }

        if index.latest.as_deref() == Some(session_id) {
            index.latest = index.sessions.first().map(|v| v.id.clone());
        }

        let path = self.session_file(session_id);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove session {}", path.display()))?;
        }
        self.write_index(&index)?;
        Ok(())
    }

    fn read_index(&self) -> Result<SessionIndex> {
        let raw = std::fs::read_to_string(&self.index_path)
            .with_context(|| format!("failed to read {}", self.index_path.display()))?;
        let index: SessionIndex = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", self.index_path.display()))?;
        Ok(index)
    }

    fn write_index(&self, index: &SessionIndex) -> Result<()> {
        let raw =
            serde_json::to_string_pretty(index).context("failed to serialize session index")?;
        std::fs::write(&self.index_path, raw)
            .with_context(|| format!("failed to write {}", self.index_path.display()))
    }

    fn touch_latest(&self, session_id: &str) -> Result<()> {
        let mut index = self.read_index()?;
        if !index.sessions.iter().any(|v| v.id == session_id) {
            return Err(anyhow!("session id {} missing from index", session_id));
        }
        index.latest = Some(session_id.to_string());
        self.write_index(&index)
    }

    fn upsert_meta(
        &self,
        transcript: &SessionTranscript,
        title: String,
        mark_latest: bool,
    ) -> Result<()> {
        let mut index = self.read_index()?;
        let meta = SessionMeta {
            id: transcript.id.clone(),
            title,
            provider: transcript.provider.clone(),
            created_at: transcript.created_at,
            updated_at: transcript.updated_at,
            message_count: transcript.messages.len(),
        };

        if let Some(existing) = index.sessions.iter_mut().find(|v| v.id == transcript.id) {
            *existing = meta;
        } else {
            index.sessions.push(meta);
        }
        index.sessions.sort_by_key(|m| m.updated_at);
        index.sessions.reverse();
        if mark_latest {
            index.latest = Some(transcript.id.clone());
        }
        self.write_index(&index)
    }

    fn write_transcript(&self, transcript: &SessionTranscript) -> Result<()> {
        let path = self.session_file(&transcript.id);
        let raw = serde_json::to_string_pretty(transcript)
            .context("failed to serialize session transcript")?;
        std::fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))
    }

    fn session_file(&self, id: &str) -> PathBuf {
        self.root.join(format!("{}.json", id))
    }
}

fn derive_title(messages: &[SessionMessage]) -> String {
    let user = messages
        .iter()
        .find(|m| m.role == StoredRole::User)
        .map(|m| m.text.trim())
        .filter(|v| !v.is_empty())
        .unwrap_or("New session");
    let truncated = user.chars().take(60).collect::<String>();
    if user.chars().count() > 60 {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{OpenSessionOptions, SessionStore, StoredRole};

    #[test]
    fn creates_and_continues_latest_session() {
        let tmp = TempDir::new().expect("tmp");
        let store = SessionStore::new(tmp.path()).expect("store");
        let created = store
            .open_or_create(
                OpenSessionOptions {
                    continue_last: false,
                    session_id: None,
                },
                "claude",
                tmp.path(),
            )
            .expect("create");
        store
            .append_message(&created.id, StoredRole::User, "hello")
            .expect("append");
        let resumed = store
            .open_or_create(
                OpenSessionOptions {
                    continue_last: true,
                    session_id: None,
                },
                "claude",
                tmp.path(),
            )
            .expect("resume");
        assert_eq!(resumed.id, created.id);
        assert_eq!(resumed.transcript.messages.len(), 1);
    }

    #[test]
    fn lists_and_deletes_sessions() {
        let tmp = TempDir::new().expect("tmp");
        let store = SessionStore::new(tmp.path()).expect("store");
        let a = store.create("claude", tmp.path()).expect("a");
        let b = store.create("opencode", tmp.path()).expect("b");
        let listed = store.list_recent(10).expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, b.id);
        store.delete(&a.id).expect("delete");
        let listed = store.list_recent(10).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, b.id);
    }
}
