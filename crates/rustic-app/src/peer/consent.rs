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

/// One file or folder inside a project named in a transfer request
/// (`path` is project-relative, `/`-separated; empty = the project root).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestedFile {
    pub project_id: String,
    #[serde(default)]
    pub project_name: String,
    pub path: String,
    #[serde(default)]
    pub is_dir: bool,
    /// Upload only: top-level names arriving in the destination folder `path`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// Body of `POST /lan/request`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransferRequest {
    /// `"push"` (caller sends to us), `"pull"` (caller takes from us) or
    /// `"meta_access"` (caller wants to browse our metadata).
    pub kind: String,
    #[serde(default)]
    pub projects: Vec<RequestedProject>,
    #[serde(default)]
    pub meta: Vec<RequestedMeta>,
    /// Individual files / folders (batch pull or upload).
    #[serde(default)]
    pub files: Vec<RequestedFile>,
    /// Set for `kind == "meta_access"`.
    #[serde(default)]
    pub meta_access: bool,
    /// Total bytes the caller expects to move (shown in the prompt).
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

/// What an approved transfer may move, for one device.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub device_id: String,
    pub kind: String,
    pub projects: HashSet<String>,
    pub meta: HashSet<String>,
    /// `project_id/path` of approved files and folders.
    pub files: HashSet<String>,
    pub expires: Instant,
}

/// Ticket key for a file item.
pub fn file_key(project_id: &str, path: &str) -> String {
    format!("{project_id}/{}", path.trim_matches('/'))
}

impl Ticket {
    /// Ticket for `req` from `device_id`, valid for [`TICKET_TTL`].
    pub fn for_request(device_id: &str, req: &TransferRequest) -> Self {
        Ticket {
            device_id: device_id.to_string(),
            kind: req.kind.clone(),
            projects: req.projects.iter().map(|p| p.id.clone()).collect(),
            meta: req.meta.iter().map(|m| m.key.clone()).collect(),
            files: req.files.iter().map(|f| file_key(&f.project_id, &f.path)).collect(),
            expires: Instant::now() + TICKET_TTL,
        }
    }

    /// Whether `project_id/path` is covered: approved itself, inside an
    /// approved folder, or inside a whole approved project.
    pub fn covers_file(&self, project_id: &str, path: &str) -> bool {
        if self.projects.contains(project_id) {
            return true;
        }
        let key = file_key(project_id, path);
        self.files.iter().any(|f| key == *f || key.starts_with(&format!("{f}/")) || f == &format!("{project_id}/"))
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
    out.extend(
        req.files
            .iter()
            .filter(|f| !share.has_project(&f.project_id))
            .map(|f| format!("{}/{}", if f.project_name.is_empty() { &f.project_id } else { &f.project_name }, f.path)),
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
    if req.kind == "meta_access" {
        return Ok(());
    }
    if req.kind != "push" && req.kind != "pull" {
        return Err("kind must be \"push\", \"pull\" or \"meta_access\"".into());
    }
    if req.projects.is_empty() && req.meta.is_empty() && req.files.is_empty() {
        return Err("nothing selected to transfer".into());
    }
    for f in &req.files {
        crate::peer::files::safe_rel(&f.path)?;
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
            ..Default::default()
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
        let empty = TransferRequest { kind: "push".into(), ..Default::default() };
        assert!(validate(&empty).is_err());
        let escape = TransferRequest {
            kind: "pull".into(),
            files: vec![RequestedFile { project_id: "p".into(), project_name: String::new(), path: "../etc".into(), is_dir: false, names: vec![] }],
            ..Default::default()
        };
        assert!(validate(&escape).is_err());
    }

    #[test]
    fn ticket_covers_files_inside_approved_folders() {
        let r = TransferRequest {
            kind: "pull".into(),
            files: vec![RequestedFile { project_id: "p".into(), project_name: String::new(), path: "src/lib".into(), is_dir: true, names: vec![] }],
            ..Default::default()
        };
        let t = Ticket::for_request("d", &r);
        assert!(t.covers_file("p", "src/lib"));
        assert!(t.covers_file("p", "src/lib/a.rs"));
        assert!(!t.covers_file("p", "src/library.rs"));
        assert!(!t.covers_file("q", "src/lib/a.rs"));
    }
}
