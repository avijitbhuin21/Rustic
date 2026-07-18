//! `rustic-server` library: the headless web transport for Rustic.
//!
//! Exposes the building blocks (router, context, auth, hub) so integration
//! tests can exercise them, plus [`run`] which the binary calls.

pub mod api;
pub mod app;
pub mod auth;
pub mod browser;
pub mod cloudflared;
pub mod commands;
pub mod context;
pub mod git_credentials;
pub mod github;
pub mod hub;
pub mod port_monitor;
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
    refuse_burned_password(&config)?;
    // PaaS health probes (Railway etc.) target the $PORT they injected. If an
    // explicit RUSTIC_BIND_ADDR overrides it with a different port, the app
    // runs fine but every probe fails with "service unavailable" — make that
    // misconfiguration impossible to miss.
    if let Ok(p) = std::env::var("PORT") {
        if let Ok(p) = p.trim().parse::<u16>() {
            if p != config.bind_addr.port() {
                tracing::warn!(
                    env_port = p,
                    bound_port = config.bind_addr.port(),
                    "PORT is set but RUSTIC_BIND_ADDR binds a different port — \
                     platform health checks will probe PORT and fail"
                );
            }
        }
    }
    std::fs::create_dir_all(&config.data_dir).ok();

    tracing::info!(
        bind = %config.bind_addr,
        data_dir = %config.data_dir.display(),
        static_dir = %config.static_dir.display(),
        "starting rustic-server"
    );

    let shared = build_shared(config.clone())?;
    // Terminal auto-resume: session monitors run on std::threads and need a
    // runtime handle to start the async resume turn when a background command
    // finishes while its task is idle.
    commands::terminal::init_resume_runtime(tokio::runtime::Handle::current());
    // Feed the connected GitHub token to terminal `git` via GIT_ASKPASS so
    // private clone/fetch/push work there without an interactive prompt.
    {
        let token = shared
            .ctx
            .state
            .git_token
            .lock()
            .ok()
            .and_then(|g| (*g).clone());
        git_credentials::apply(&config.data_dir, token.as_deref());
        if let Some(tok) = token {
            tokio::spawn(async move {
                commands::git::apply_git_identity(&tok).await;
            });
        }
    }
    // Keep a handle so we can tear Chromium down in the graceful-shutdown path
    // (SIGTERM on container stop) — the strict "nothing runs when closed" rule.
    let browser = shared.ctx.browser.clone();

    // Watch for dev servers starting/stopping in the VM: auto-expose new ones
    // via Cloudflare (when enabled) and reap tunnels whose upstream port dies.
    port_monitor::spawn(
        shared.ctx.clone(),
        config.bind_addr.port(),
        shared.ctx.browser.port(),
    );

    // GitHub auto-issue-resolve: the FIFO worker that turns labeled issues
    // into fixer tasks. Idles (30s ticks) while the feature is disabled.
    github::worker::spawn(shared.ctx.clone());

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

/// Refuse to start on a non-loopback bind with a known/burned default password.
/// The `.env.example` password has been published in this repo's history, and
/// placeholders like `change-me` are the first thing a scanner tries — either
/// one on a reachable interface is effectively no password at all. Loopback
/// binds are exempt so local dev stays frictionless.
fn refuse_burned_password(config: &ServerConfig) -> anyhow::Result<()> {
    const BURNED: &[&str] = &[
        "avijitbhuin21", // the .env.example default — published, hence burned
        "change-me",
        "changeme",
        "change_me",
        "change-me-please",
        "password",
        // NOTE: the run-server.ps1 dev default "rustic" is deliberately NOT
        // here — the script itself refuses -BindAll with it unless the caller
        // passes an explicit -AllowInsecurePassword override.
        "example",
        "admin",
        "123456",
    ];
    if config.bind_addr.ip().is_loopback() {
        return Ok(());
    }
    let pw = config.auth_password.trim().to_ascii_lowercase();
    if BURNED.contains(&pw.as_str()) {
        anyhow::bail!(
            "RUSTIC_AUTH_PASSWORD is a known default/placeholder password and the bind \
             address {} is not loopback. Refusing to start: set a real password \
             (or bind 127.0.0.1 for local use).",
            config.bind_addr
        );
    }
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

    let tunnel = initial_tunnel_config(&config, &boot.state);
    let session_gen = initial_session_generation(&boot.state);

    let ctx = ServerContext {
        state: boot.state,
        hub,
        data_dir: config.data_dir.clone(),
        home_dir: home_dir(),
        secrets,
        browser,
        tunnel: Arc::new(std::sync::RwLock::new(tunnel)),
        cloudflared: Arc::new(crate::cloudflared::CloudflaredManager::new()),
        session_gen: Arc::new(std::sync::atomic::AtomicU64::new(session_gen)),
        github_notify: Arc::new(tokio::sync::Notify::new()),
    };

    Ok(Arc::new(Shared {
        rate: auth::RateLimiter::new(config.login_max_attempts, config.login_lockout_secs),
        tickets: auth::TicketStore::new(),
        ctx,
        config,
    }))
}

/// Resolve the tunnel config at boot: a DB-persisted UI setting wins; otherwise
/// fall back to the `RUSTIC_PREVIEW_DOMAIN` / `RUSTIC_COOKIE_DOMAIN` env vars;
/// otherwise default to path mode.
fn initial_tunnel_config(
    config: &rustic_app::config::ServerConfig,
    state: &std::sync::Arc<rustic_app::state::AppState>,
) -> crate::context::TunnelConfig {
    use rustic_app::sync_ext::MutexExt;
    if let Ok(Some(json)) = state.db.lock_safe().get_setting("tunnel_config") {
        if let Ok(tc) = serde_json::from_str::<crate::context::TunnelConfig>(&json) {
            return tc;
        }
    }
    if let Some(pd) = config.preview_domain.clone() {
        return crate::context::TunnelConfig {
            mode: "subdomain".to_string(),
            preview_domain: Some(pd),
            cookie_domain: config.cookie_domain.clone(),
            auto_expose: true,
        };
    }
    crate::context::TunnelConfig::default()
}

/// Load the persisted session generation (bumped by every logout/power-off).
/// Persisting it means a logout survives a server restart even with a stable
/// `RUSTIC_SESSION_SECRET`. Absent/garbage value → start at 0.
fn initial_session_generation(state: &std::sync::Arc<rustic_app::state::AppState>) -> u64 {
    use rustic_app::sync_ext::MutexExt;
    state
        .db
        .lock_safe()
        .get_setting("session_generation")
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
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
