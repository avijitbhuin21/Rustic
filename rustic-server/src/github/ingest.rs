//! Pulls a GitHub issue into the project working tree:
//! `issues/issue-<n>/issue.md` (body + metadata, attachment links rewritten
//! to local files) plus the auto-maintained `issues/index.md` checklist.

use std::path::{Path, PathBuf};

use serde_json::Value;

use rustic_app::sync_ext::MutexExt;
use rustic_db::GithubIssueRow;

use crate::context::ServerContext;
use crate::github::client::GithubClient;

/// Folder for one issue, relative to the project root.
pub fn issue_dir(root: &Path, number: i64) -> PathBuf {
    root.join("issues").join(format!("issue-{number}"))
}

/// Hosts we recognise as GitHub-issue attachments worth mirroring locally.
fn is_attachment_url(url: &str) -> bool {
    url.starts_with("https://github.com/user-attachments/")
        || url.starts_with("https://user-images.githubusercontent.com/")
        || url.starts_with("https://private-user-images.githubusercontent.com/")
}

/// Extract candidate attachment URLs from a markdown body. Hand-rolled scan
/// (no regex dep): every `https://` run up to a markdown/url delimiter.
fn extract_attachment_urls(body: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = body;
    while let Some(pos) = rest.find("https://") {
        let candidate = &rest[pos..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | ']' | '<'))
            .unwrap_or(candidate.len());
        let url = &candidate[..end];
        if is_attachment_url(url) && !urls.iter().any(|u| u == url) {
            urls.push(url.to_string());
        }
        rest = &candidate[end.max(1)..];
    }
    urls
}

fn ext_for_content_type(ct: &str, url: &str) -> &'static str {
    match ct {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "application/zip" => "zip",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => {
            // Fall back to a recognisable extension already on the URL.
            let tail = url.rsplit('.').next().unwrap_or("");
            match tail {
                "png" => "png",
                "jpg" | "jpeg" => "jpg",
                "gif" => "gif",
                "mp4" => "mp4",
                "zip" => "zip",
                "pdf" => "pdf",
                "txt" | "log" => "txt",
                _ => "bin",
            }
        }
    }
}

/// Write `issues/issue-<n>/issue.md` from the (fresh) issue JSON, mirroring
/// attachments next to it. Returns the body that was written (with local
/// attachment paths), for inlining into the fixer task's first message.
pub async fn write_issue_folder(
    ctx: &ServerContext,
    root: &Path,
    repo: &str,
    issue: &Value,
) -> Result<String, String> {
    let number = issue["number"]
        .as_i64()
        .ok_or("issue payload missing number")?;
    let dir = issue_dir(root, number);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    let mut body = issue["body"]
        .as_str()
        .unwrap_or("(no description)")
        .to_string();

    // Mirror attachments locally and rewrite the links. Best-effort: a failed
    // download keeps the original URL in place.
    if let Ok(client) = GithubClient::from_ctx(ctx) {
        for (i, url) in extract_attachment_urls(&body).into_iter().enumerate() {
            match client.download(&url).await {
                Ok((bytes, content_type)) => {
                    let name = format!(
                        "attachment-{}.{}",
                        i + 1,
                        ext_for_content_type(&content_type, &url)
                    );
                    let path = dir.join(&name);
                    if let Err(e) = std::fs::write(&path, &bytes) {
                        tracing::warn!(url = %url, %e, "attachment write failed; keeping remote link");
                        continue;
                    }
                    body = body.replace(&url, &name);
                }
                Err(e) => {
                    tracing::warn!(url = %url, %e, "attachment download failed; keeping remote link");
                }
            }
        }
    }

    let title = issue["title"].as_str().unwrap_or("(untitled)");
    let author = issue["user"]["login"].as_str().unwrap_or("unknown");
    let html_url = issue["html_url"].as_str().unwrap_or("");
    let created = issue["created_at"].as_str().unwrap_or("");
    let labels = issue["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let md = format!(
        "# Issue #{number}: {title}\n\n\
         - **Repository:** {repo}\n\
         - **Reported by:** @{author}\n\
         - **Created:** {created}\n\
         - **Labels:** {labels}\n\
         - **URL:** {html_url}\n\n\
         ---\n\n{body}\n"
    );
    std::fs::write(dir.join("issue.md"), &md).map_err(|e| format!("write issue.md: {e}"))?;
    Ok(body)
}

/// Regenerate `issues/index.md` for a project from the DB — idempotent, one
/// checklist line per tracked issue.
pub fn rewrite_index(ctx: &ServerContext, root: &Path, project_id: &str) {
    let rows: Vec<GithubIssueRow> = {
        let db = ctx.state.db.lock_safe();
        db.list_github_issues(Some(project_id)).unwrap_or_default()
    };
    if rows.is_empty() {
        return;
    }
    let mut out = String::from(
        "# GitHub Issues\n\n\
         > Maintained automatically by Rustic auto-issue-resolve. Do not edit by hand.\n\n",
    );
    let mut sorted = rows;
    sorted.sort_by_key(|r| r.issue_number);
    for row in &sorted {
        let check = if row.status == "done" { "x" } else { " " };
        let status_note = match row.status.as_str() {
            "done" => "fixed (committed locally)".to_string(),
            "waiting_reply" => "waiting for the reporter's reply".to_string(),
            "working" => "agent working".to_string(),
            "failed" => {
                if row.error.is_empty() {
                    "failed — needs human attention".to_string()
                } else {
                    format!("failed — {}", row.error)
                }
            }
            "manual" => "taken over manually in Rustic".to_string(),
            other => other.to_string(),
        };
        out.push_str(&format!(
            "- [{check}] #{num} — {title} (`issues/issue-{num}/`) — {status_note}\n",
            num = row.issue_number,
            title = row.title,
        ));
    }
    let dir = root.join("issues");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(%e, "issues dir create failed");
        return;
    }
    if let Err(e) = std::fs::write(dir.join("index.md"), out) {
        tracing::warn!(%e, "index.md write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::extract_attachment_urls;

    #[test]
    fn finds_attachment_urls() {
        let body = "Crash here\n\
            ![screen](https://github.com/user-attachments/assets/abc-123)\n\
            see also <https://user-images.githubusercontent.com/1/shot.png> and \
            https://example.com/not-attachment.png";
        let urls = extract_attachment_urls(body);
        assert_eq!(
            urls,
            vec![
                "https://github.com/user-attachments/assets/abc-123".to_string(),
                "https://user-images.githubusercontent.com/1/shot.png".to_string(),
            ]
        );
    }
}
