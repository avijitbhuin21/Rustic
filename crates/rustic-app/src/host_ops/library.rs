//! Helpers shared by the markdown-library host ops (skills, workflows, rules):
//! name sanitising, description summaries, GitHub source parsing and
//! archive access.

use std::io::Read;

use crate::github_download::{self, DownloadError, MAX_TEXT_DOWNLOAD_BYTES, MAX_ZIP_DOWNLOAD_BYTES};

pub fn sanitize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// One-line frontmatter description from the first 150 chars of the body.
pub fn summarize(body: &str) -> String {
    const MAX: usize = 150;
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX {
        flat
    } else {
        let truncated: String = flat.chars().take(MAX).collect();
        format!("{}...", truncated.trim_end())
    }
}

/// Canonical `---name/description---` frontmatter + body.
pub fn with_frontmatter(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {}\ndescription: {}\n---\n\n{}", name, description, body)
}

/// Picks entry `i` of an optional parallel name-override list (trimmed, non-empty).
pub fn name_override(names: Option<&[String]>, i: usize) -> Option<String> {
    names
        .and_then(|v| v.get(i))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
pub enum GithubSource {
    /// Download the whole repo as a zip and scan it (optionally filtered by subpath).
    RepoZip {
        owner: String,
        repo: String,
        subpath: Option<String>,
    },
    /// Fetch a single file (raw.githubusercontent.com, or converted from a blob URL).
    RawFile { url: String },
}

/// Accepts `owner/repo`, `https://github.com/owner/repo[.git]`,
/// `.../blob/<branch>/path` (a `.md` blob is rewritten to its raw URL) and
/// `https://raw.githubusercontent.com/...`.
pub fn parse_github_source(source: &str) -> Result<GithubSource, String> {
    let source = source.trim();
    if source.starts_with("https://raw.githubusercontent.com/")
        || source.starts_with("http://raw.githubusercontent.com/")
    {
        return Ok(GithubSource::RawFile { url: source.to_string() });
    }

    let path = source
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/")
        .trim_start_matches("github.com/")
        .trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() < 2 || segments[0].is_empty() || segments[1].is_empty() {
        return Err(format!(
            "Invalid GitHub source: \"{}\" — expected owner/repo, a github.com URL, or a raw.githubusercontent.com URL",
            source
        ));
    }
    let owner = segments[0].to_string();
    let repo = segments[1].trim_end_matches(".git").to_string();

    if segments.len() > 4 && segments[2] == "blob" {
        let branch = segments[3];
        let subpath = segments[4..].join("/");
        if subpath.ends_with(".md") {
            let url = format!("https://raw.githubusercontent.com/{}/{}/{}/{}", owner, repo, branch, subpath);
            return Ok(GithubSource::RawFile { url });
        }
        return Ok(GithubSource::RepoZip { owner, repo, subpath: Some(subpath) });
    }
    Ok(GithubSource::RepoZip { owner, repo, subpath: None })
}

fn map_download_err(e: DownloadError) -> String {
    match e {
        DownloadError::Transport(msg) => msg,
        DownloadError::Http { status, url } => format!("HTTP {} downloading {}", status, url),
        DownloadError::TooLarge { len, cap, url } => {
            format!("Download too large ({} bytes, cap {}) for {}", len, cap, url)
        }
        DownloadError::ExceededCap { cap, url } => format!("Download exceeded {} byte cap for {}", cap, url),
    }
}

pub async fn download_text(url: &str) -> Result<String, String> {
    github_download::download_text(url, MAX_TEXT_DOWNLOAD_BYTES)
        .await
        .map_err(map_download_err)
}

pub async fn download_repo_zip(owner: &str, repo: &str) -> Result<Vec<u8>, String> {
    github_download::download_repo_zip(owner, repo, MAX_ZIP_DOWNLOAD_BYTES)
        .await
        .map_err(map_download_err)
}

/// Raw content of one entry from a GitHub source, for preview before install.
/// `path` is a zip entry (repo sources) or a raw URL (single-file sources).
pub async fn preview_entry(source: &str, path: &str) -> Result<String, String> {
    match parse_github_source(source)? {
        GithubSource::RawFile { .. } => download_text(path).await,
        GithubSource::RepoZip { owner, repo, .. } => {
            let zip_bytes = download_repo_zip(&owner, &repo).await?;
            read_zip_text(&zip_bytes, path)
        }
    }
}

pub fn open_zip(zip_bytes: &[u8]) -> Result<zip::ZipArchive<std::io::Cursor<&[u8]>>, String> {
    zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())
}

pub fn read_zip_text(zip_bytes: &[u8], entry: &str) -> Result<String, String> {
    let mut archive = open_zip(zip_bytes)?;
    let mut file = archive.by_name(entry).map_err(|e| e.to_string())?;
    let mut s = String::new();
    file.read_to_string(&mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

/// Strip the `<repo>-<branch>/` prefix GitHub zips put on every entry.
pub fn strip_zip_top_dir(path: &str) -> &str {
    match path.find('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Whether an in-archive path (top dir stripped) matches a user subpath filter.
pub fn matches_subpath(inside: &str, filter: &str) -> bool {
    let filter = filter.trim_end_matches('/');
    inside == filter || inside.ends_with(&format!("/{}", filter)) || filter.ends_with(inside)
}

/// Friendly default name from a URL or file path: the filename minus extension.
pub fn filename_stem(url_or_path: &str) -> String {
    let last = url_or_path.rsplit(['/', '\\']).next().unwrap_or(url_or_path);
    last.rsplit_once('.').map(|(a, _)| a).unwrap_or(last).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_sources() {
        assert_eq!(
            parse_github_source("owner/repo.git").unwrap(),
            GithubSource::RepoZip { owner: "owner".into(), repo: "repo".into(), subpath: None }
        );
        assert_eq!(
            parse_github_source("https://github.com/o/r/blob/main/skills/x/SKILL.md").unwrap(),
            GithubSource::RawFile { url: "https://raw.githubusercontent.com/o/r/main/skills/x/SKILL.md".into() }
        );
        assert_eq!(
            parse_github_source("https://github.com/o/r/blob/main/skills/x/").unwrap(),
            GithubSource::RepoZip { owner: "o".into(), repo: "r".into(), subpath: Some("skills/x".into()) }
        );
        assert!(matches!(
            parse_github_source(" https://raw.githubusercontent.com/o/r/main/a.md ").unwrap(),
            GithubSource::RawFile { .. }
        ));
        assert!(parse_github_source("justone").is_err());
    }

    #[test]
    fn stems_and_subpaths() {
        assert_eq!(filename_stem("https://x/y/my-flow.md"), "my-flow");
        assert_eq!(filename_stem("C:\\a\\b.md"), "b");
        assert_eq!(filename_stem("noext"), "noext");
        assert_eq!(strip_zip_top_dir("repo-main/skills/a/SKILL.md"), "skills/a/SKILL.md");
        assert!(matches_subpath("skills/a/SKILL.md", "skills/a/SKILL.md/"));
        assert!(matches_subpath("x/skills/a/SKILL.md", "skills/a/SKILL.md"));
        assert!(!matches_subpath("skills/b/SKILL.md", "skills/a"));
    }

    #[test]
    fn overrides_and_frontmatter() {
        let names = vec!["  ".to_string(), " New ".to_string()];
        assert_eq!(name_override(Some(&names), 0), None);
        assert_eq!(name_override(Some(&names), 1), Some("New".into()));
        assert_eq!(name_override(None, 0), None);
        assert_eq!(with_frontmatter("a", "d", "b"), "---\nname: a\ndescription: d\n---\n\nb");
        assert_eq!(sanitize_name("My Skill!"), "my-skill");
        assert_eq!(summarize("a\n b"), "a b");
    }
}
