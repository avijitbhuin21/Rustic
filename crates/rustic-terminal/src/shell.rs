use crate::pty::{PtySession, SessionId};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub label: String,
    pub cwd: String,
    pub is_agent: bool,
}

pub struct TerminalManager {
    sessions: HashMap<SessionId, PtySession>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Create a new terminal session. Returns the session info and the reader
    /// that should be used to stream output in a background thread.
    pub fn create_session(
        &mut self,
        cwd: PathBuf,
        label: String,
        is_agent: bool,
    ) -> Result<(SessionInfo, Box<dyn std::io::Read + Send>)> {
        let mut session = PtySession::new(cwd, label, is_agent)?;
        let reader = session
            .take_reader()
            .ok_or_else(|| anyhow::anyhow!("Reader already taken"))?;

        let info = SessionInfo {
            id: session.id,
            label: session.label.clone(),
            cwd: session.cwd.to_string_lossy().to_string(),
            is_agent: session.is_agent,
        };

        self.sessions.insert(session.id, session);
        Ok((info, reader))
    }

    pub fn write_session(&mut self, id: SessionId, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        session.write(data)
    }

    pub fn resize_session(&self, id: SessionId, cols: u16, rows: u16) -> Result<()> {
        let session = self
            .sessions
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        session.resize(cols, rows)
    }

    pub fn destroy_session(&mut self, id: SessionId) {
        self.sessions.remove(&id);
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|s| SessionInfo {
                id: s.id,
                label: s.label.clone(),
                cwd: s.cwd.to_string_lossy().to_string(),
                is_agent: s.is_agent,
            })
            .collect()
    }
}
