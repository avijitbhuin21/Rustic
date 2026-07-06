//! Shared GitHub download helpers for the skills/workflows library commands.
//!
//! Both the Tauri desktop shell and `rustic-server` install skills/workflows
//! from GitHub (raw file URLs and whole-repo codeload zips). The HTTP logic —
//! client construction (UA + redirect limit), status checking, and the
//! streaming size cap — is identical across hosts, so it lives here once.
//!
//! Size-cap semantics (deliberately strict):
//! 1. A `Content-Length` header above the cap fails fast, before any body read.
//! 2. The body is then read chunk-by-chunk and the download aborts as soon as
//!    the accumulated size crosses the cap — chunked/lying responses can never
//!    buffer an unbounded body in memory.
//!
//! Errors are returned as a structured [`DownloadError`] rather than a
//! `String`: the two hosts historically produced slightly different
//! user-facing wording for the cap errors, and each maps the variants back to
//! its own strings at the call site.

/// Cap for single-file (text) downloads — a skill/workflow `.md` should be
/// kilobytes.
pub const MAX_TEXT_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;
/// Cap for whole-repo zip archive downloads.
pub const MAX_ZIP_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

/// Structured download failure. Hosts map these to their own error strings.
#[derive(Debug)]
pub enum DownloadError {
    /// Client build, request send, or body-read failure (stringified reqwest
    /// error — the exact text both hosts previously surfaced).
    Transport(String),
    /// Non-success HTTP status.
    Http {
        status: reqwest::StatusCode,
        url: String,
    },
    /// `Content-Length` announced a body larger than the cap (failed fast,
    /// before reading the body).
    TooLarge { len: u64, cap: u64, url: String },
    /// The streamed body crossed the cap mid-read (chunked or lying
    /// `Content-Length`); the download was aborted.
    ExceededCap { cap: u64, url: String },
}

/// GET `url` and return the body, rejecting oversized responses.
///
/// See the module docs for the exact cap semantics.
pub async fn download_capped(url: &str, cap: u64) -> Result<Vec<u8>, DownloadError> {
    let client = reqwest::Client::builder()
        .user_agent("Rustic-Agent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| DownloadError::Transport(e.to_string()))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DownloadError::Http {
            status: resp.status(),
            url: url.to_string(),
        });
    }
    if let Some(len) = resp.content_length() {
        if len > cap {
            return Err(DownloadError::TooLarge {
                len,
                cap,
                url: url.to_string(),
            });
        }
    }
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| DownloadError::Transport(e.to_string()))?
    {
        if out.len() as u64 + chunk.len() as u64 > cap {
            return Err(DownloadError::ExceededCap {
                cap,
                url: url.to_string(),
            });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Download a text file (lossy UTF-8) with the given byte cap.
pub async fn download_text(url: &str, cap: u64) -> Result<String, DownloadError> {
    let bytes = download_capped(url, cap).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Download a GitHub repo as a zip archive via codeload, trying the `main`
/// branch first and falling back to `master` on any failure.
pub async fn download_repo_zip(
    owner: &str,
    repo: &str,
    cap: u64,
) -> Result<Vec<u8>, DownloadError> {
    let main_url = format!(
        "https://codeload.github.com/{}/{}/zip/refs/heads/main",
        owner, repo
    );
    match download_capped(&main_url, cap).await {
        Ok(b) => Ok(b),
        Err(_) => {
            let master_url = format!(
                "https://codeload.github.com/{}/{}/zip/refs/heads/master",
                owner, repo
            );
            download_capped(&master_url, cap).await
        }
    }
}
