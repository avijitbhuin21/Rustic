//! rustic-server as a sync peer: the same pairing / sharing / approval / push
//! and pull logic the desktop runs (`rustic_app::peer`), hosted on the
//! server's own HTTP(S) address under `/lan/*`. No TLS listener, mDNS or
//! tunnel — the server is already reachable at its URL. Incoming pushes ask in
//! the web UI like pulls do: a push can carry metadata (MCP server commands,
//! provider base URLs), so a paired device's token alone must not be enough.
//! `RUSTIC_PEER_AUTO_ACCEPT_PUSH=1` restores unattended acceptance for a
//! headless server whose paired devices are all trusted.

use std::path::PathBuf;
use std::sync::Arc;

use rustic_app::peer::{HostRef, PeerHost};

use crate::context::ServerContext;

/// [`PeerHost`] backed by the server context.
pub struct ServerPeerHost {
    ctx: ServerContext,
}

impl ServerPeerHost {
    /// Shared host handle for `ctx`.
    pub fn arc(ctx: &ServerContext) -> HostRef {
        Arc::new(ServerPeerHost { ctx: ctx.clone() })
    }
}

impl PeerHost for ServerPeerHost {
    fn state(&self) -> &rustic_app::state::AppState {
        &self.ctx.state
    }

    fn data_dir(&self) -> Result<PathBuf, String> {
        Ok(self.ctx.data_dir.clone())
    }

    fn home_dir(&self) -> PathBuf {
        self.ctx.home_dir.clone()
    }

    fn emitter(&self) -> Arc<dyn rustic_app::EventEmitter> {
        Arc::new(self.ctx.clone())
    }

    fn secrets(&self) -> &dyn rustic_app::secrets::SecretStore {
        &*self.ctx.secrets
    }

    fn auto_accept_push(&self) -> bool {
        auto_accept_push_enabled(std::env::var(AUTO_ACCEPT_PUSH_ENV).ok().as_deref())
    }
}

/// Opt-in for approving incoming pushes with nobody at the web UI.
pub const AUTO_ACCEPT_PUSH_ENV: &str = "RUSTIC_PEER_AUTO_ACCEPT_PUSH";

/// Whether the opt-in variable's value turns push auto-accept on. Off unless set to `1` or `true`.
fn auto_accept_push_enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true")
    )
}

/// Peer routes (`/lan/*`) for the public router. They authenticate paired
/// devices by bearer token themselves, so they sit outside the password gate.
pub fn router(ctx: &ServerContext) -> anyhow::Result<axum::Router> {
    rustic_app::peer::listener::build_router(ServerPeerHost::arc(ctx), ctx.lan.clone(), false)
        .map_err(|e| anyhow::anyhow!(e))
}

/// Re-enable peer sync at boot if it was on (identity only — no listener).
pub async fn restore(ctx: &ServerContext) {
    let host = ServerPeerHost::arc(ctx);
    if rustic_app::peer::is_enabled_persisted(&ctx.data_dir) {
        if let Err(e) = rustic_app::peer::ops::start_identity_only(&host, &ctx.lan).await {
            tracing::warn!("peer sync did not start: {e}");
        }
    }
    if let Ok(url) = std::env::var("RUSTIC_PUBLIC_URL") {
        let url = url.trim().trim_end_matches('/').to_string();
        if !url.is_empty() {
            rustic_app::peer::set_public_url(Some(url));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::auto_accept_push_enabled;

    #[test]
    fn push_auto_accept_is_off_unless_opted_in() {
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("yes"),
        ] {
            assert!(!auto_accept_push_enabled(off), "{off:?}");
        }
        for on in [Some("1"), Some("true"), Some(" TRUE ")] {
            assert!(auto_accept_push_enabled(on), "{on:?}");
        }
    }
}
