//! Server implementation of the `rustic_agent::PeerAgents` broker trait.
//!
//! Mirrors the desktop `commands::agent::peers::TauriPeerAgents`, but reaches
//! state and emits through a cloned [`ServerContext`] instead of an `AppHandle`,
//! and delivers peer messages by re-entering this crate's `send_message`
//! command body.

use rustic_agent::{
    peer_info_from_history, peer_status_is_active, peer_status_str,
    unreplied_inbound_peer_messages, InboundPeerMessage, PeerAgentInfo, PeerAgents,
};
use rustic_app::context::{AppContext, EventEmitterExt};
use rustic_app::state::AppState;
use rustic_app::sync_ext::MutexExt;

use crate::context::ServerContext;

/// Server-side `PeerAgents` broker. Cheap to construct (holds a cloned ctx).
pub(crate) struct ServerPeerAgents {
    ctx: ServerContext,
}

impl ServerPeerAgents {
    pub(crate) fn new(ctx: ServerContext) -> Self {
        Self { ctx }
    }
}

impl PeerAgents for ServerPeerAgents {
    fn list_active_peers(&self, self_task_id: &str) -> Vec<PeerAgentInfo> {
        let state: &AppState = self.ctx.state();

        // Snapshot under the agent lock, then drop it before touching the DB —
        // the two locks must never nest.
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
        let state: &AppState = self.ctx.state();
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
        let state: &AppState = self.ctx.state();

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
                    "task '{}' belongs to a different project — peer messaging is scoped to \
                     agents sharing your project.",
                    to_task_id
                ));
            }
            if !peer_status_is_active(&target.info.status) {
                return Err(format!(
                    "task '{}' is no longer active (status: {}) — it would never read the \
                     message. Re-run `check_other_active_agents`.",
                    to_task_id,
                    peer_status_str(&target.info.status)
                ));
            }
            from.info.title.clone()
        };

        let text = rustic_agent::format_peer_message(from_task_id, &from_title, body);

        // Show the message in the receiving transcript immediately — the
        // frontend appends user messages optimistically for its own sends and
        // has no other signal for one that originates in the backend.
        self.ctx.emit(
            "agent-peer-message",
            serde_json::json!({
                "task_id": to_task_id,
                "from_task_id": from_task_id,
                "from_title": from_title,
                "text": text,
            }),
        );

        // Delivery starts a whole turn, so it must not block the calling
        // agent's tool call. `send_message` cancels the target's in-flight run.
        let ctx = self.ctx.clone();
        let to = to_task_id.to_string();
        // `send_message` parses its args as camelCase (`SendMessageArg`), so the
        // key must be `taskId` — `task_id` deserializes to a 400 and the message
        // would silently never arrive.
        let args = serde_json::json!({ "taskId": to, "message": text });
        state.agent_runtime.spawn(async move {
            if let Err(e) = crate::commands::agent_chat::send_message(&ctx, &args).await {
                tracing::warn!(task = %to, error = %e.message, "peer message delivery failed");
            }
        });

        Ok(())
    }
}
