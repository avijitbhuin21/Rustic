//! Implementation of the `PeerAgents` broker trait (defined in rustic-agent)
//! over the host's task table.
//!
//! `list_active_peers` reads `AgentState.tasks` plus the file-history write
//! ledger; `send_to_peer` re-enters the normal `send_message` path, so a peer
//! message goes through exactly the same machinery as one the user typed â€”
//! including cancelling the target's in-flight run before the new turn starts.

use crate::commands::agent::send_message;
use crate::state::AppState;
use crate::sync_ext::MutexExt;
use rustic_agent::{
    peer_info_from_history, peer_status_is_active, peer_status_str,
    unreplied_inbound_peer_messages, InboundPeerMessage, PeerAgentInfo, PeerAgents,
};
use tauri::{AppHandle, Emitter, Manager};

pub struct TauriPeerAgents {
    app: AppHandle,
}

impl TauriPeerAgents {
    /// Broker over the app's live task table for one running task.
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl PeerAgents for TauriPeerAgents {
    fn list_active_peers(&self, self_task_id: &str) -> Vec<PeerAgentInfo> {
        let state = self.app.state::<AppState>();

        // Snapshot everything we need under the agent lock, then drop it before
        // touching the DB â€” the two locks must never nest.
        let mut peers: Vec<PeerAgentInfo> = {
            let agent = state.agent.lock_safe();
            let Some(own_project) = agent
                .tasks
                .get(self_task_id)
                .map(|t| t.info.project_id.clone())
            else {
                return Vec::new();
            };
            agent
                .tasks
                .values()
                .filter(|t| {
                    t.info.id != self_task_id
                        && t.info.project_id == own_project
                        && peer_status_is_active(&t.info.status)
                })
                .map(|t| {
                    let mut peer = peer_info_from_history(&t.info, &t.messages);
                    peer.running_subagents = state.subagent_registry.running_count(&t.info.id);
                    peer
                })
                .collect()
        };

        if peers.is_empty() {
            return peers;
        }

        {
            let db = state.db.lock_safe();
            for p in peers.iter_mut() {
                p.written_paths = db.fh_list_task_writes(&p.task_id).unwrap_or_default();
                p.written_paths.sort();
            }
        }
        peers.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        peers
    }

    fn inbound_peer_messages(&self, self_task_id: &str) -> Vec<InboundPeerMessage> {
        let state = self.app.state::<AppState>();
        let agent = state.agent.lock_safe();
        let Some(own) = agent.tasks.get(self_task_id) else {
            return Vec::new();
        };
        let mut inbound = unreplied_inbound_peer_messages(&own.messages);
        for m in inbound.iter_mut() {
            m.sender_active = agent
                .tasks
                .get(&m.from_task_id)
                .map(|t| peer_status_is_active(&t.info.status))
                .unwrap_or(false);
        }
        inbound
    }

    fn send_to_peer(&self, from_task_id: &str, to_task_id: &str, body: &str) -> Result<(), String> {
        let state = self.app.state::<AppState>();

        // Validate against the live table before delivering: same project, and
        // still active. Anything else means nothing would ever read the message.
        let from_title = {
            let agent = state.agent.lock_safe();
            let from = agent.tasks.get(from_task_id).ok_or_else(|| {
                format!("your own task ({}) is not in the task table", from_task_id)
            })?;
            let target = agent.tasks.get(to_task_id).ok_or_else(|| {
                format!(
                    "no task '{}' exists. Re-run `check_other_active_agents` for current ids.",
                    to_task_id
                )
            })?;
            if target.info.project_id != from.info.project_id {
                return Err(format!(
                    "task '{}' belongs to a different project â€” peer messaging is scoped to \
                     agents sharing your project.",
                    to_task_id
                ));
            }
            if !peer_status_is_active(&target.info.status) {
                return Err(format!(
                    "task '{}' is no longer active (status: {}) â€” it would never read the \
                     message. Re-run `check_other_active_agents`.",
                    to_task_id,
                    peer_status_str(&target.info.status)
                ));
            }
            from.info.title.clone()
        };

        let text = rustic_agent::format_peer_message(from_task_id, &from_title, body);

        // Let the receiving UI show the message the moment it is sent: the
        // frontend appends user messages optimistically for its own sends and
        // has no other signal for a message that originates in the backend.
        let _ = self.app.emit(
            "agent-peer-message",
            serde_json::json!({
                "task_id": to_task_id,
                "from_task_id": from_task_id,
                "from_title": from_title,
                "text": text,
            }),
        );

        // Delivery itself is async (it starts a whole turn) and must not block
        // the calling agent's tool call, so hand it to the shared agent runtime.
        // `send_message` cancels the target's in-flight run on its own.
        let app = self.app.clone();
        let to = to_task_id.to_string();
        let runtime = state.agent_runtime.clone();
        runtime.spawn(async move {
            let inner = app.clone();
            if let Err(e) = send_message(
                app,
                inner.state::<AppState>(),
                to.clone(),
                text,
                None,
                None,
                None,
                None,
            )
            .await
            {
                tracing::warn!(task = %to, %e, "peer message delivery failed");
            }
        });

        Ok(())
    }
}
