//! Peer sync commands for the web UI — the same `lan_*` command names and
//! argument shapes as the desktop, backed by the shared `rustic_app::peer`
//! ops. On the server "enabled" means "accept pairing" (identity only — the
//! peer routes are always mounted on the server's own address); there is no
//! TLS listener, mDNS or tunnel.

use serde::Deserialize;
use serde_json::Value;

use rustic_app::peer::{self, ops};

use crate::api::{ok, parse, ApiError};
use crate::context::ServerContext;
use crate::peer::ServerPeerHost;

pub async fn dispatch(
    ctx: &ServerContext,
    command: &str,
    args: &Value,
) -> Option<Result<Value, ApiError>> {
    if !command.starts_with("lan_") {
        return None;
    }
    Some(run(ctx, command, args).await)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceArg {
    device_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondArg {
    request_id: String,
    accept: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameArg {
    device_id: String,
    nickname: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareArg {
    device_id: String,
    share: peer::Share,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectionArg {
    device_id: String,
    direction: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetaApplyArg {
    device_id: String,
    direction: String,
    #[serde(default)]
    overwrite: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncItemsArg {
    device_id: String,
    direction: String,
    #[serde(default)]
    projects: Vec<ops::SyncProject>,
    #[serde(default)]
    meta: Vec<peer::consent::RequestedMeta>,
}

#[derive(Deserialize)]
struct EnabledArg {
    enabled: bool,
}

#[derive(Deserialize)]
struct AddressArg {
    address: String,
}

#[derive(Deserialize)]
struct NameArg {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FsPathArg {
    device_id: String,
    project_id: String,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemsArg {
    device_id: String,
    items: Vec<peer::consent::RequestedFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalConflictsArg {
    dest_dir: String,
    items: Vec<peer::consent::RequestedFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteConflictsArg {
    device_id: String,
    project_id: String,
    #[serde(default)]
    dir: String,
    names: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullFilesArg {
    device_id: String,
    items: Vec<peer::consent::RequestedFile>,
    dest_dir: String,
    #[serde(default)]
    opts: ops::FileTransferOpts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushFilesArg {
    device_id: String,
    local_paths: Vec<String>,
    project_id: String,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    opts: ops::FileTransferOpts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetaViewArg {
    device_id: String,
    allowed: bool,
}

#[derive(Deserialize)]
struct TransferIdArg {
    #[serde(default)]
    id: Option<String>,
}

/// Switch pairing on/off for the server (persisted). Identity only.
async fn set_enabled(ctx: &ServerContext, enabled: bool) -> Result<(), String> {
    let host = ServerPeerHost::arc(ctx);
    peer::set_enabled_persisted(&ctx.data_dir, enabled)?;
    if enabled {
        ops::start_identity_only(&host, &ctx.lan).await
    } else {
        ops::stop(&ctx.lan);
        Ok(())
    }
}

/// Run one `lan_*` command.
async fn run(ctx: &ServerContext, command: &str, args: &Value) -> Result<Value, ApiError> {
    let host = ServerPeerHost::arc(ctx);
    let lan = &ctx.lan;
    match command {
        "lan_status" => ok(ops::status(&host, lan).await?),
        "lan_set_enabled" => {
            let a: EnabledArg = parse(args)?;
            ok(set_enabled(ctx, a.enabled).await?)
        }
        "lan_devices" => ok(ops::devices(&host, lan).await?),
        "lan_add_manual" => {
            let a: AddressArg = parse(args)?;
            ok(ops::add_manual(&host, lan, &a.address).await?)
        }
        "lan_pair_code" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::pair_code_for(&host, lan, &a.device_id)?)
        }
        "lan_pair" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::pair(&host, lan, &a.device_id).await?)
        }
        "lan_cancel_outgoing" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::cancel_outgoing(lan, &a.device_id)?)
        }
        "lan_respond_pair" => {
            let a: RespondArg = parse(args)?;
            ok(ops::respond_pair(lan, &a.request_id, a.accept)?)
        }
        "lan_forget" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::forget(&host, lan, &a.device_id).await?)
        }
        "lan_list_files" => {
            let a: FsPathArg = parse(args)?;
            ok(ops::list_files(&host, lan, &a.device_id, &a.project_id, &a.path).await?)
        }
        "lan_preview_file" => {
            let a: FsPathArg = parse(args)?;
            ok(ops::preview_file(&host, lan, &a.device_id, &a.project_id, &a.path).await?)
        }
        "lan_remote_size" => {
            let a: ItemsArg = parse(args)?;
            ok(ops::remote_size(&host, lan, &a.device_id, &a.items).await?)
        }
        "lan_local_conflicts" => {
            let a: LocalConflictsArg = parse(args)?;
            ok(ops::local_conflicts(&a.dest_dir, &a.items))
        }
        "lan_remote_conflicts" => {
            let a: RemoteConflictsArg = parse(args)?;
            ok(ops::remote_conflicts(&host, lan, &a.device_id, &a.project_id, &a.dir, a.names).await?)
        }
        "lan_pull_files" => {
            let a: PullFilesArg = parse(args)?;
            ok(ops::pull_files(&host, lan, &a.device_id, a.items, a.dest_dir, a.opts).await?)
        }
        "lan_push_files" => {
            let a: PushFilesArg = parse(args)?;
            ok(ops::push_files(&host, lan, &a.device_id, a.local_paths, a.project_id, a.dir, a.opts).await?)
        }
        "lan_request_meta_access" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::request_meta_access(&host, lan, &a.device_id).await?)
        }
        "lan_meta_browse" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::meta_browse(&host, lan, &a.device_id).await?)
        }
        "lan_set_meta_view" => {
            let a: MetaViewArg = parse(args)?;
            ok(ops::set_meta_view(&host, &a.device_id, a.allowed)?)
        }
        "lan_get_meta_view" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::paired(&host, &a.device_id)?.meta_view)
        }
        "lan_announce" => {
            ops::announce(&host, lan).await;
            ok(())
        }
        "lan_transfers" => ok(rustic_app::transfers::list()),
        "lan_transfer_cancel" => {
            let a: TransferIdArg = parse(args)?;
            ok(rustic_app::transfers::cancel(a.id.as_deref().unwrap_or_default())?)
        }
        "lan_transfer_pause" => {
            let a: TransferIdArg = parse(args)?;
            ok(rustic_app::transfers::pause(a.id.as_deref().unwrap_or_default())?)
        }
        "lan_transfer_resume" => {
            let a: TransferIdArg = parse(args)?;
            ok(rustic_app::transfers::resume(a.id.as_deref().unwrap_or_default())?)
        }
        "lan_transfer_clear" => {
            let a: TransferIdArg = parse(args)?;
            rustic_app::transfers::clear(a.id.as_deref());
            ok(())
        }
        "lan_version" => ok(peer::app_version()),
        "lan_rename" => {
            let a: RenameArg = parse(args)?;
            ok(ops::rename(&host, &a.device_id, &a.nickname)?)
        }
        "lan_set_device_name" => {
            let a: NameArg = parse(args)?;
            peer::set_custom_device_name(&ctx.data_dir, &a.name)?;
            // Reload the identity so the new name is served from /lan/info.
            if lan.lock().identity.is_some() {
                ops::stop(lan);
                ops::start_identity_only(&host, lan).await?;
            }
            ok(())
        }
        "lan_local_meta" => ok(ops::local_meta(&host).await?),
        "lan_get_share" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::get_share(&host, &a.device_id)?)
        }
        "lan_set_share" => {
            let a: ShareArg = parse(args)?;
            ok(ops::set_share(&host, &a.device_id, a.share)?)
        }
        "lan_respond_transfer" => {
            let a: RespondArg = parse(args)?;
            ok(ops::respond_transfer(lan, &a.request_id, a.accept)?)
        }
        "lan_sync_items" => {
            let a: SyncItemsArg = parse(args)?;
            ok(ops::sync_items(&host, lan, &a.device_id, &a.direction, a.projects, a.meta).await?)
        }
        "lan_list_projects" => {
            let a: DeviceArg = parse(args)?;
            ok(ops::list_projects(&host, lan, &a.device_id).await?)
        }
        "lan_meta_preview" => {
            let a: DirectionArg = parse(args)?;
            ok(ops::meta_preview(&host, lan, &a.device_id, &a.direction).await?)
        }
        "lan_meta_apply" => {
            let a: MetaApplyArg = parse(args)?;
            ok(ops::meta_apply(&host, lan, &a.device_id, &a.direction, a.overwrite).await?)
        }
        "lan_set_internet_mode" => Err(ApiError::from(
            "The server is already reachable at its own URL — no tunnel needed.".to_string(),
        )),
        other => Err(ApiError::from(format!("{other} isn't available on the server"))),
    }
}
