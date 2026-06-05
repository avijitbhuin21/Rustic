//! Embedded VM browser commands (web/server build only).
//!
//! Dispatched via `POST /api/<command>` like every other command. These drive
//! Chromium's lifecycle + tabs through the [`BrowserManager`] and a handful of
//! one-shot CDP calls. High-throughput traffic (screencast, DevTools) does NOT
//! come through here — it's the raw proxy in [`crate::browser::proxy`].
//!
//! Events emitted on the `/ws` hub: `browser-tabs-changed` (tab list mutated)
//! and `browser-stopped` (Chromium gone — crash, last-tab-close, or teardown).

use serde::Deserialize;
use serde_json::{json, Value};

use rustic_app::context::EventEmitterExt;

use crate::api::{ok, parse, ApiError};
use crate::browser::cdp;
use crate::context::ServerContext;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "browser_open" => browser_open(ctx).await,
        "browser_status" => browser_status(ctx).await,
        "browser_list_tabs" => browser_list_tabs(ctx).await,
        "browser_new_tab" => browser_new_tab(ctx, args).await,
        "browser_close_tab" => browser_close_tab(ctx, args).await,
        "browser_navigate" => browser_navigate(ctx, args).await,
        "browser_close" => browser_close(ctx).await,
        _ => return None,
    })
}

/// Notify the frontend the tab list changed so it re-fetches via `browser_status`.
fn emit_tabs_changed(ctx: &ServerContext) {
    ctx.emit("browser-tabs-changed", json!({}));
}

/// `browser_open` — ensure Chromium is up, guarantee at least one tab, return
/// the current state. Idempotent: opening an already-open browser just lists.
async fn browser_open(ctx: &ServerContext) -> Result<Value, ApiError> {
    let ep = ctx.browser.ensure_started().await?;
    let mut tabs = cdp::list_pages(&ep.http_base).await?;
    if tabs.is_empty() {
        cdp::create_target(&ep.browser_ws, "about:blank").await?;
        tabs = cdp::list_pages(&ep.http_base).await?;
    }
    emit_tabs_changed(ctx);
    ok(json!({ "running": true, "tabs": tabs }))
}

/// `browser_status` — current running flag + tab list (empty when stopped).
async fn browser_status(ctx: &ServerContext) -> Result<Value, ApiError> {
    match ctx.browser.endpoint_if_running().await {
        Some(ep) => {
            let tabs = cdp::list_pages(&ep.http_base).await.unwrap_or_default();
            ok(json!({ "running": true, "tabs": tabs }))
        }
        None => ok(json!({ "running": false, "tabs": [] })),
    }
}

/// `browser_list_tabs` — proxy the CDP target list (running browser only).
async fn browser_list_tabs(ctx: &ServerContext) -> Result<Value, ApiError> {
    match ctx.browser.endpoint_if_running().await {
        Some(ep) => {
            let tabs = cdp::list_pages(&ep.http_base).await?;
            ok(json!({ "tabs": tabs }))
        }
        None => ok(json!({ "tabs": [] })),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewTabArgs {
    url: Option<String>,
}

/// `browser_new_tab { url? }` — start Chromium if needed, create a target.
async fn browser_new_tab(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: NewTabArgs = parse(args)?;
    let ep = ctx.browser.ensure_started().await?;
    let url = a.url.unwrap_or_else(|| "about:blank".to_string());
    let target_id = cdp::create_target(&ep.browser_ws, &url).await?;
    let tabs = cdp::list_pages(&ep.http_base).await?;
    emit_tabs_changed(ctx);
    ok(json!({ "running": true, "tabs": tabs, "activeTabId": target_id }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetArgs {
    target_id: String,
}

/// `browser_close_tab { targetId }` — close a tab. If it was the last one, the
/// browser shuts down entirely (the strict lifecycle rule).
async fn browser_close_tab(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: TargetArgs = parse(args)?;
    let Some(ep) = ctx.browser.endpoint_if_running().await else {
        return ok(json!({ "running": false, "tabs": [] }));
    };
    cdp::close_target(&ep.browser_ws, &a.target_id).await?;
    let tabs = cdp::list_pages(&ep.http_base).await.unwrap_or_default();
    if tabs.is_empty() {
        // Last tab closed → tear Chromium all the way down. `stop` emits
        // `browser-stopped`, so the UI closes the window.
        ctx.browser.stop().await;
        return ok(json!({ "running": false, "tabs": [] }));
    }
    emit_tabs_changed(ctx);
    ok(json!({ "running": true, "tabs": tabs }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NavigateArgs {
    target_id: String,
    url: String,
}

/// `browser_navigate { targetId, url }` — `Page.navigate` on the target's socket.
async fn browser_navigate(ctx: &ServerContext, args: &Value) -> Result<Value, ApiError> {
    let a: NavigateArgs = parse(args)?;
    if ctx.browser.endpoint_if_running().await.is_none() {
        return Err(ApiError::bad("browser is not running"));
    }
    let page_ws = ctx.browser.page_ws(&a.target_id);
    cdp::navigate(&page_ws, &a.url).await?;
    // The new title/url propagate asynchronously; nudge the UI to re-fetch.
    emit_tabs_changed(ctx);
    ok(json!({ "ok": true }))
}

/// `browser_close` — window closed: terminate Chromium. Idempotent.
async fn browser_close(ctx: &ServerContext) -> Result<Value, ApiError> {
    ctx.browser.stop().await;
    ok(json!({ "running": false }))
}
