use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

/// Liveness probe used by Railway's healthcheck.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Resolves the static asset directory from `WEBSITE_STATIC_DIR` or the crate-local `static/` folder.
fn static_dir() -> PathBuf {
    env::var_os("WEBSITE_STATIC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static"))
}

/// Builds the router: `/healthz`, then static files with `index.html` as the SPA-style fallback.
fn app(static_dir: PathBuf) -> Router {
    let index = static_dir.join("index.html");
    let not_found = static_dir.join("404.html");
    let fallback = if not_found.exists() {
        ServeFile::new(not_found)
    } else {
        ServeFile::new(index)
    };
    let files = ServeDir::new(&static_dir)
        .precompressed_br()
        .precompressed_gzip()
        .append_index_html_on_directories(true)
        .not_found_service(fallback);

    Router::new()
        .route("/healthz", get(healthz))
        .fallback_service(files)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=3600, stale-while-revalidate=86400"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
}

/// Resolves the bind address from `PORT` (Railway) / `HOST`, defaulting to 0.0.0.0:8080.
fn bind_addr() -> SocketAddr {
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)))
}

/// Waits for Ctrl-C or SIGTERM so the container stops promptly on redeploy.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let dir = static_dir();
    if !dir.join("index.html").exists() {
        tracing::error!(path = %dir.display(), "static directory has no index.html");
        std::process::exit(1);
    }
    let addr = bind_addr();
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(err) => {
            tracing::error!(%addr, %err, "failed to bind");
            std::process::exit(1);
        }
    };
    tracing::info!(%addr, static_dir = %dir.display(), "rustic-website listening");
    if let Err(err) = axum::serve(listener, app(dir))
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(%err, "server exited with error");
    }
}
