//! Push / pull approvals for local-network sync. Every transfer between two
//! desktops must be approved on the machine that gives something up: a push
//! by the receiver, a pull by the sender. Approval yields a short-lived
//! ticket naming exactly which projects and metadata items may move; the
//! listener refuses transfers that don't carry a matching ticket.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How long an approved ticket stays usable.
pub const TICKET_TTL: Duration = Duration::from_secs(30 * 60);

/// How long the approval prompt waits for an answer.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// One project named in a transfer request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestedProject {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Push only: whether the receiver already has this project (it will be replaced).
    #[serde(default)]
    pub exists: bool,
}

/// One metadata item named in a transfer request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestedMeta {
    pub key: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub name: String,
}

/// Body of `POST /lan/request`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    /// `"push"` (caller sends to us) or `"pull"` (caller takes from us).
    pub kind: String,
    #[serde(default)]
    pub projects: Vec<RequestedProject>,
    #[serde(default)]
    pub meta: Vec<RequestedMeta>,
}

/// What an approved transfer may move, for one device.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub device_id: String,
    pub kind: String,
    pub projects: HashSet<String>,
    pub meta: HashSet<String>,
    pub expires: Instant,
}

impl Ticket {
    /// Ticket for `req` from `device_id`, valid for [`TICKET_TTL`].
    pub fn for_request(device_id: &str, req: &TransferRequest) -> Self {
        Ticket {
            device_id: device_id.to_string(),
            kind: req.kind.clone(),
            projects: req.projects.iter().map(|p| p.id.clone()).collect(),
            meta: req.meta.iter().map(|m| m.key.clone()).collect(),
            expires: Instant::now() + TICKET_TTL,
        }
    }

    /// Whether this ticket lets `device_id` do a `kind` transfer right now.
    pub fn valid_for(&self, device_id: &str, kind: &str) -> bool {
        self.device_id == device_id && self.kind == kind && Instant::now() < self.expires
    }
}

/// Items of a pull request that the device isn't allowed to see. Empty = OK.
pub fn unshared_items(req: &TransferRequest, share: &super::Share) -> Vec<String> {
    let mut out: Vec<String> = req
        .projects
        .iter()
        .filter(|p| !share.has_project(&p.id))
        .map(|p| if p.name.is_empty() { p.id.clone() } else { p.name.clone() })
        .collect();
    out.extend(
        req.meta
            .iter()
            .filter(|m| !share.has_meta(&m.key))
            .map(|m| m.key.clone()),
    );
    out
}

/// Key marking a payload that was replaced by its content hash.
pub const SUMMARY_KEY: &str = "__rustic_sha";

/// Content hash of a metadata payload.
fn payload_hash(v: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(serde_json::to_string(v).unwrap_or_default().as_bytes());
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Same bundle with every payload replaced by its hash — lets the other side
/// show new / changed / same without receiving any actual content.
pub fn summarize(bundle: &crate::meta_sync::MetaBundle) -> crate::meta_sync::MetaBundle {
    crate::meta_sync::MetaBundle {
        items: bundle
            .items
            .iter()
            .map(|i| crate::meta_sync::MetaItem {
                category: i.category.clone(),
                name: i.name.clone(),
                payload: serde_json::json!({ SUMMARY_KEY: payload_hash(&i.payload) }),
            })
            .collect(),
    }
}

/// Whether `bundle` came from [`summarize`] (so the local side must be summarized too before diffing).
pub fn is_summary(bundle: &crate::meta_sync::MetaBundle) -> bool {
    bundle
        .items
        .first()
        .and_then(|i| i.payload.as_object())
        .is_some_and(|o| o.len() == 1 && o.contains_key(SUMMARY_KEY))
}

/// Validate a request's shape before prompting the user.
pub fn validate(req: &TransferRequest) -> Result<(), String> {
    if req.kind != "push" && req.kind != "pull" {
        return Err("kind must be \"push\" or \"pull\"".into());
    }
    if req.projects.is_empty() && req.meta.is_empty() {
        return Err("nothing selected to transfer".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::Share;

    fn req(kind: &str) -> TransferRequest {
        TransferRequest {
            kind: kind.into(),
            projects: vec![RequestedProject { id: "p1".into(), name: "Alpha".into(), exists: false }],
            meta: vec![RequestedMeta { key: "skill/x".into(), category: "skill".into(), name: "x".into() }],
        }
    }

    #[test]
    fn ticket_matches_device_kind_and_items() {
        let t = Ticket::for_request("dev1", &req("pull"));
        assert!(t.valid_for("dev1", "pull"));
        assert!(!t.valid_for("dev1", "push"));
        assert!(!t.valid_for("dev2", "pull"));
        assert!(t.projects.contains("p1"));
        assert!(t.meta.contains("skill/x"));
    }

    #[test]
    fn unshared_items_respects_allowlist() {
        let none = Share::default();
        assert_eq!(unshared_items(&req("pull"), &none), vec!["Alpha", "skill/x"]);
        let all = Share { projects: vec!["p1".into()], meta: vec!["skill/x".into()] };
        assert!(unshared_items(&req("pull"), &all).is_empty());
    }

    #[test]
    fn summarize_hides_content_but_keeps_identity() {
        use crate::meta_sync::{diff_bundles, MetaBundle, MetaItem, MetaStatus};
        let item = |p: &str| MetaItem { category: "skill".into(), name: "x".into(), payload: serde_json::json!({ "body": p }) };
        let remote = MetaBundle { items: vec![item("secret")] };
        let s = summarize(&remote);
        assert!(is_summary(&s));
        assert!(!s.items[0].payload.to_string().contains("secret"));
        let same = summarize(&MetaBundle { items: vec![item("secret")] });
        let diff = summarize(&MetaBundle { items: vec![item("other")] });
        assert_eq!(diff_bundles(&s, &same)[0].status, MetaStatus::Same);
        assert_eq!(diff_bundles(&s, &diff)[0].status, MetaStatus::Conflict);
    }

    #[test]
    fn validate_rejects_bad_requests() {
        assert!(validate(&req("pull")).is_ok());
        assert!(validate(&req("steal")).is_err());
        let empty = TransferRequest { kind: "push".into(), projects: vec![], meta: vec![] };
        assert!(validate(&empty).is_err());
    }
}
