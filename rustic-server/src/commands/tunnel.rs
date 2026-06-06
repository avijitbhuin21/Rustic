//! Cloudflare quick-tunnel commands (server-only). Spawns/queries `cloudflared`
//! quick tunnels for the cloudflare port-forwarding mode.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortArg {
    port: u16,
}

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    Some(match command {
        "tunnel_open" => match parse::<PortArg>(args) {
            Ok(a) => match ctx.cloudflared.open(a.port).await {
                Ok(url) => ok(json!({ "url": url })),
                Err(e) => Err(ApiError::bad(e)),
            },
            Err(e) => Err(e),
        },
        "tunnel_close" => match parse::<PortArg>(args) {
            Ok(a) => {
                ctx.cloudflared.close(a.port).await;
                ok(())
            }
            Err(e) => Err(e),
        },
        "tunnel_list" => ok(ctx
            .cloudflared
            .list()
            .await
            .into_iter()
            .map(|(port, url)| json!({ "port": port, "url": url }))
            .collect::<Vec<_>>()),
        _ => return None,
    })
}
