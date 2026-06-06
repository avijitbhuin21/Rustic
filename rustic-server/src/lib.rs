//! `rustic-server` library: the headless web transport for Rustic.
//!
//! Exposes the building blocks (router, context, auth, hub) so integration
//! tests can exercise them, plus [`run`] which the binary calls.

pub mod api;
pub mod app;
pub mod auth;
pub mod browser;
pub mod commands;
pub mod context;
pub mod git_credentials;
pub mod hub;
pub mod proxy;
pub mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use rustic_app::config::ServerConfig;
use rustic_app::secrets::{EnvSecretStore, FileSecretStore, SecretStore};

use crate::app::Shared;
use crate::context::ServerContext;
use crate::hub::EventHub;

/// Boot config + state, build the router, and serve until a shutdown signal.
pub async fn run() -> anyhow::Result<()> {
    load_dotenv();
    init_tracing();

    let config = ServerConfig::from_env().map_err(|e| anyhow::anyhow!(e))?;
    std::fs::create_dir_all(&config.data_dir).ok();

    tracing::info!(
        bind = %config.bind_addr,
        data_dir = %config.data_dir.display(),
        static_dir = %config.static_dir.display(),
        "starting rustic-server"
    );

    let shared = build_shared(config.clone())?;
    // Feed the connected GitHub token to terminal `git` via GIT_ASKPASS so
    // private clone/fetch/push work there without an interactive prompt.
    {
        let token = shared.ctx.state.git_token.lock().ok().and_then(|g| (*g).clone());
        git_credentials::apply(&config.data_dir, token.as_deref());
    }
    // Keep a handle so we can tear Chromium down in the graceful-shutdown path
    // (SIGTERM on container stop) — the strict "nothing runs when closed" rule.
    let browser = shared.ctx.browser.clone();
    let router = app::build_router(shared);

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "listening");

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    // Reap any running Chromium before exit so the container leaves nothing
    // behind. No-op when the browser was never started or is already stopped.
    browser.stop().await;

    tracing::info!("shut down cleanly");
    Ok(())
}

/// Bootstrap shared state from a resolved config. Factored out of [`run`] so
/// integration tests can build a `Shared` against a temp data dir.
pub fn build_shared(config: ServerConfig) -> anyhow::Result<Arc<Shared>> {
    let secrets: Arc<dyn SecretStore> =
        Arc::new(EnvSecretStore::new(FileSecretStore::new(&config.data_dir)));

    let hub = EventHub::new(1024);
    let boot_emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(HubEmitter(hub.clone()));
    let boot = rustic_app::bootstrap(&config.data_dir, &*secrets, boot_emitter)?;

    // The embedded-browser manager emits `browser-stopped` (crash/teardown) onto
    // the same hub the frontend already listens on.
    let browser_emitter: Arc<dyn rustic_app::EventEmitter> = Arc::new(HubEmitter(hub.clone()));
    let browser = Arc::new(crate::browser::BrowserManager::new(
        config.data_dir.clone(),
        browser_emitter,
    ));

    let ctx = ServerContext {
        state: boot.state,
        hub,
        data_dir: config.data_dir.clone(),
        home_dir: home_dir(),
        secrets,
        browser,
    };

    Ok(Arc::new(Shared {
        rate: auth::RateLimiter::new(config.login_max_attempts, config.login_lockout_secs),
        ctx,
        config,
    }))
}

/// A minimal emitter that publishes onto the hub (used during bootstrap before
/// the full `ServerContext` exists).
struct HubEmitter(EventHub);
impl rustic_app::EventEmitter for HubEmitter {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        self.0.publish(event, payload);
    }
}

fn home_dir() -> std::path::PathBuf {
    #[cfg(unix)]
    {
        if let Some(h) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(h);
        }
    }
    #[cfg(windows)]
    {
        if let Some(h) = std::env::var_os("USERPROFILE") {
            return std::path::PathBuf::from(h);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Load a `.env` file from the working directory into the process environment.
/// Hand-rolled (no `dotenvy` dep): `KEY=VALUE` lines, `#` comments, optional
/// surrounding quotes. Existing env vars are never overwritten.
fn load_dotenv() {
    let path = std::path::Path::new(".env");
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,reqwest=warn,hyper=warn,tower=warn,h2=warn,rustls=warn")
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .try_init();
}

/// Resolve on Ctrl-C or SIGTERM (container stop) so in-flight requests drain.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sig) = signal(SignalKind::terminate()) {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
