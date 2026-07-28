//! Session persistence for tracking review progress and enabling resume.
//!
//! Each review run creates a session file at `.reviewer/sessions/<id>.jsonl`.
//! Records are appended as JSON lines: session_start, file_done, file_skipped,
//! and session_end. On resume, completed files are skipped.

use crate::error::{AgentError, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Unique session identifier.
pub type SessionId = String;

/// A single record in the session JSONL file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionRecord {
    #[serde(rename = "session_start")]
    Start {
        session_id: SessionId,
        model: String,
        domain: String,
        timestamp: u64,
        pr_url: Option<String>,
    },
    #[serde(rename = "file_done")]
    FileDone {
        path: String,
        fingerprint: String,
        finding_count: usize,
        timestamp: u64,
    },
    #[serde(rename = "file_skipped")]
    FileSkipped {
        path: String,
        reason: String,
        timestamp: u64,
    },
    #[serde(rename = "session_end")]
    End {
        session_id: SessionId,
        total_files: usize,
        reviewed_files: usize,
        total_findings: usize,
        timestamp: u64,
    },
}

/// Session state for a single review run.
pub struct Session {
    id: SessionId,
    writer: Option<std::io::BufWriter<std::fs::File>>,
    reviewed_count: usize,
    total_findings: usize,
}

impl Session {
    /// Create a new session with a generated ID.
    pub fn new(model: &str, domain: &str, pr_url: Option<&str>) -> Result<Self> {
        let id = generate_session_id();
        let dir = Path::new(".reviewer").join("sessions");
        std::fs::create_dir_all(&dir).map_err(|e| {
            AgentError::Config(format!(
                "Cannot create sessions directory '{}': {}",
                dir.display(),
                e
            ))
        })?;

        let file_path = dir.join(format!("{}.jsonl", id));
        let file = std::fs::File::create(&file_path).map_err(AgentError::Io)?;
        let mut writer = std::io::BufWriter::new(file);

        let record = SessionRecord::Start {
            session_id: id.clone(),
            model: model.to_string(),
            domain: domain.to_string(),
            timestamp: now(),
            pr_url: pr_url.map(|s| s.to_string()),
        };
        writeln!(writer, "{}", serde_json::to_string(&record).unwrap()).map_err(AgentError::Io)?;

        Ok(Self {
            id,
            writer: Some(writer),
            reviewed_count: 0,
            total_findings: 0,
        })
    }

    /// The session ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Record that a file was reviewed.
    pub fn record_file_done(&mut self, path: &str, fingerprint: &str, finding_count: usize) {
        self.reviewed_count += 1;
        self.total_findings += finding_count;
        self.write_record(&SessionRecord::FileDone {
            path: path.to_string(),
            fingerprint: fingerprint.to_string(),
            finding_count,
            timestamp: now(),
        });
    }

    /// Record that a file was skipped.
    pub fn record_file_skipped(&mut self, path: &str, reason: &str) {
        self.write_record(&SessionRecord::FileSkipped {
            path: path.to_string(),
            reason: reason.to_string(),
            timestamp: now(),
        });
    }

    /// Finalize the session.
    pub fn finalize(mut self, total_files: usize) {
        self.write_record(&SessionRecord::End {
            session_id: self.id.clone(),
            total_files,
            reviewed_files: self.reviewed_count,
            total_findings: self.total_findings,
            timestamp: now(),
        });
        self.writer.take();
    }

    fn write_record(&mut self, record: &SessionRecord) {
        if let Some(ref mut writer) = self.writer {
            match serde_json::to_string(record) {
                Ok(line) => {
                    if let Err(e) = writeln!(writer, "{}", line) {
                        tracing::warn!(error = %e, "Failed to write session record");
                    }
                    if let Err(e) = writer.flush() {
                        tracing::warn!(error = %e, "Failed to flush session file");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to serialize session record");
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.writer.take();
    }
}

/// Compute a fingerprint for a file review (used for resume dedup).
/// Uses SHA-256 for deterministic, cross-session stable hashing.
pub fn file_fingerprint(mode: &str, path: &str, diff_hash: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(mode.as_bytes());
    hasher.update(b"\x00");
    hasher.update(path.as_bytes());
    hasher.update(b"\x00");
    hasher.update(diff_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Load a previous session state for resume.
pub fn load_resume_state(session_id: &str) -> Result<ResumeState> {
    // Validate session_id to prevent path traversal
    if !session_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AgentError::Config(format!(
            "Invalid session ID '{}': must be alphanumeric, dash, or underscore only",
            session_id
        )));
    }
    let path = Path::new(".reviewer")
        .join("sessions")
        .join(format!("{}.jsonl", session_id));
    let content = std::fs::read_to_string(&path).map_err(|e| {
        AgentError::Config(format!(
            "Failed to read session file '{}': {}",
            path.display(),
            e
        ))
    })?;

    let mut state = ResumeState {
        session_id: session_id.to_string(),
        completed_files: Vec::new(),
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(SessionRecord::FileDone {
            path, fingerprint, ..
        }) = serde_json::from_str::<SessionRecord>(line)
        {
            state
                .completed_files
                .push(CompletedFile { path, fingerprint });
        }
    }

    Ok(state)
}

/// State loaded from a previous session for resume.
pub struct ResumeState {
    pub session_id: String,
    pub completed_files: Vec<CompletedFile>,
}

impl ResumeState {
    /// Check if a file with the given fingerprint was already reviewed.
    pub fn is_completed(&self, fingerprint: &str) -> bool {
        self.completed_files
            .iter()
            .any(|f| f.fingerprint == fingerprint)
    }
}

pub struct CompletedFile {
    pub path: String,
    pub fingerprint: String,
}

fn generate_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("rev-{:x}", nanos)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_session_id() {
        let id = generate_session_id();
        assert!(id.starts_with("rev-"));
        assert!(id.len() > 5);
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let a = file_fingerprint("diff", "src/main.rs", "abc123");
        let b = file_fingerprint("diff", "src/main.rs", "abc123");
        assert_eq!(a, b);
    }

    #[test]
    fn test_fingerprint_differs() {
        let a = file_fingerprint("diff", "src/main.rs", "abc");
        let b = file_fingerprint("diff", "src/main.rs", "def");
        assert_ne!(a, b);
    }

    #[test]
    fn test_session_create_and_finalize() {
        let mut session =
            Session::new("test-model", "code", Some("https://github.com/o/r/pull/1")).unwrap();
        let sid = session.id().to_string();
        session.record_file_done("src/main.rs", "fp1", 3);
        session.record_file_skipped("Cargo.lock", "skippable");
        session.finalize(2);
        // Session file should exist
        let path = Path::new(".reviewer")
            .join("sessions")
            .join(format!("{}.jsonl", sid));
        assert!(path.exists(), "Session file should exist");
        // Clean up
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_resume_state_loading() {
        let mut session = Session::new("test-model", "code", None).unwrap();
        let sid = session.id().to_string();
        session.record_file_done("src/main.rs", "fp_main", 2);
        session.record_file_done("src/lib.rs", "fp_lib", 1);
        session.finalize(2);

        let state = load_resume_state(&sid).unwrap();
        assert!(state.is_completed("fp_main"));
        assert!(state.is_completed("fp_lib"));
        assert!(!state.is_completed("fp_other"));

        // Clean up
        let path = Path::new(".reviewer")
            .join("sessions")
            .join(format!("{sid}.jsonl"));
        let _ = std::fs::remove_file(&path);
    }
}
