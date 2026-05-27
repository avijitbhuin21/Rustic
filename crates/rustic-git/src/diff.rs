use crate::repo::GitRepo;
use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub origin: char, // '+', '-', ' '
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub file_path: String,
    pub hunks: Vec<DiffHunk>,
    pub additions: usize,
    pub deletions: usize,
}

impl GitRepo {
    /// Get diff for a specific file (worktree vs index, then staged vs HEAD
    /// as a fallback, then untracked-as-all-additions as a final fallback).
    /// Mirrors the libgit2-era behaviour where calling this on a staged-only
    /// modification returned the staged diff, and extends it so newly-created
    /// (untracked) files render as a full additions diff rather than an
    /// empty "no changes to display" pane — `git diff` ignores untracked
    /// paths by design, so the CLI port needs explicit handling.
    pub fn diff_file(&self, path: &str) -> Result<FileDiff> {
        let work_dir = self.work_dir()?;

        // Worktree vs index first.
        let unstaged = crate::git_cli::run(
            &work_dir,
            &["diff", "--no-color", "-U3", "--", path],
        )?;
        let mut diffs = parse_unified_text(&unstaged);
        if let Some(d) = diffs.pop() {
            if !d.hunks.is_empty() {
                return Ok(d);
            }
        }

        // Fall back to staged diff (HEAD vs index).
        let staged = crate::git_cli::run(
            &work_dir,
            &["diff", "--cached", "--no-color", "-U3", "--", path],
        )?;
        let mut diffs = parse_unified_text(&staged);
        if let Some(d) = diffs.pop() {
            if !d.hunks.is_empty() {
                return Ok(d);
            }
        }

        // Final fallback: untracked file. `git diff` (with or without
        // --cached) returns nothing for paths git has never seen, but the
        // SCM panel still wants to display the contents as a series of
        // additions. `git diff --no-index /dev/null <path>` synthesises
        // exactly that shape and exits with status 1 (because there ARE
        // differences) — so we bypass the strict run_silent wrapper and
        // accept the non-zero exit as "diff produced output".
        let abs = work_dir.join(path);
        if abs.is_file() {
            if let Ok(text) = untracked_diff(&work_dir, path) {
                let mut diffs = parse_unified_text(&text);
                if let Some(d) = diffs.pop() {
                    if !d.hunks.is_empty() {
                        return Ok(d);
                    }
                }
            }
        }

        // Nothing to show — return an empty diff for the requested path so
        // the UI has a stable shape.
        Ok(FileDiff {
            file_path: path.to_string(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        })
    }

    /// Get diff for all staged changes (HEAD vs index).
    pub fn diff_staged(&self) -> Result<Vec<FileDiff>> {
        let work_dir = self.work_dir()?;
        let text = crate::git_cli::run(
            &work_dir,
            &["diff", "--cached", "--no-color", "-U3"],
        )?;
        Ok(parse_unified_text(&text))
    }
}

/// Parse the output of `git diff --no-color -U3` into `Vec<FileDiff>`.
///
/// The format we consume:
///
/// ```text
/// diff --git a/path/to/file b/path/to/file
/// index abc..def 100644
/// --- a/path/to/file
/// +++ b/path/to/file
/// @@ -1,3 +1,4 @@
///  context line
/// -removed
/// +added
///  context line
/// ```
///
/// We extract: the b-path (preferred over a-path so renames land on the
/// new location), each `@@` hunk header, and per-line origin/content with
/// the line numbers tracked from the hunk header. Binary diffs (where
/// "Binary files ... differ" replaces the unified text) yield a FileDiff
/// with zero hunks.
pub(crate) fn parse_unified_text(text: &str) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut old_lineno: u32 = 0;
    let mut new_lineno: u32 = 0;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // New file header. The b-path is what we care about (post-rename).
            let new_path = parse_diff_git_header(rest);
            current_path = Some(new_path.clone());
            out.push(FileDiff {
                file_path: new_path,
                hunks: Vec::new(),
                additions: 0,
                deletions: 0,
            });
            continue;
        }

        if line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity index")
            || line.starts_with("rename from")
            || line.starts_with("rename to")
            || line.starts_with("Binary files")
        {
            continue;
        }

        if line.starts_with("@@ ") {
            // Hunk header: @@ -A,B +C,D @@ optional_section_heading
            if current_path.is_none() {
                continue;
            }
            if let Some((old_start, new_start)) = parse_hunk_header(line) {
                old_lineno = old_start;
                new_lineno = new_start;
            }
            if let Some(fd) = out.last_mut() {
                fd.hunks.push(DiffHunk {
                    header: line.to_string(),
                    lines: Vec::new(),
                });
            }
            continue;
        }

        // Diff body line. First char is origin (' ', '+', '-' or '\').
        let origin = line.chars().next().unwrap_or(' ');
        let content = line.get(1..).unwrap_or("").to_string();

        match origin {
            '+' => {
                if let Some(fd) = out.last_mut() {
                    fd.additions += 1;
                    let lineno = new_lineno;
                    new_lineno += 1;
                    if let Some(hunk) = fd.hunks.last_mut() {
                        hunk.lines.push(DiffLine {
                            origin: '+',
                            content,
                            old_lineno: None,
                            new_lineno: Some(lineno),
                        });
                    }
                }
            }
            '-' => {
                if let Some(fd) = out.last_mut() {
                    fd.deletions += 1;
                    let lineno = old_lineno;
                    old_lineno += 1;
                    if let Some(hunk) = fd.hunks.last_mut() {
                        hunk.lines.push(DiffLine {
                            origin: '-',
                            content,
                            old_lineno: Some(lineno),
                            new_lineno: None,
                        });
                    }
                }
            }
            ' ' => {
                if let Some(fd) = out.last_mut() {
                    let o = old_lineno;
                    let n = new_lineno;
                    old_lineno += 1;
                    new_lineno += 1;
                    if let Some(hunk) = fd.hunks.last_mut() {
                        hunk.lines.push(DiffLine {
                            origin: ' ',
                            content,
                            old_lineno: Some(o),
                            new_lineno: Some(n),
                        });
                    }
                }
            }
            '\\' => {
                // "\ No newline at end of file" — informational, skip.
            }
            _ => {} // unknown — skip rather than crash
        }
    }

    out
}

/// Parse the path from a `diff --git a/path b/path` header line. Quoting
/// rules: paths may be quoted in `"..."` if they contain spaces or special
/// chars. We do a best-effort parse — falling back to the raw split — since
/// the worst case is a slightly weird file_path string in the UI.
fn parse_diff_git_header(rest: &str) -> String {
    // Find " b/" as the split point between a-path and b-path. This is
    // reliable because the a-path always starts with "a/".
    if let Some(idx) = rest.find(" b/") {
        let b_part = &rest[idx + 3..];
        return b_part.trim_matches('"').to_string();
    }
    rest.trim().to_string()
}

/// Synthesise a `git diff`-style "new file" unified diff for an untracked
/// path. `git diff -- path` is silent on untracked files (that's by design;
/// they aren't in git's index), so calling `git diff --no-index NUL path`
/// would be the canonical cross-platform invocation — but the `/dev/null`
/// vs `NUL` split is fiddly and the exit code is 1 even on success. Far
/// simpler: read the file contents directly and emit the unified diff text
/// ourselves. The output matches what `git diff --no-index` would produce
/// for a new file, so `parse_unified_text` consumes it without changes.
fn untracked_diff(work_dir: &std::path::Path, rel_path: &str) -> Result<String> {
    let abs = work_dir.join(rel_path);
    let content = std::fs::read_to_string(&abs)
        .or_else(|_| {
            // Best-effort UTF-8 lossy read for files with mixed encoding.
            // For genuinely binary files this gives back something the
            // diff renderer can show even if it's noisy; the SCM panel
            // gates binary handling elsewhere.
            std::fs::read(&abs).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
        .with_context(|| format!("read untracked file {}", abs.display()))?;

    if content.is_empty() {
        // Empty new file — still emit a header so the UI can render
        // "new file mode" and a 0-line additions block.
        let mut s = String::new();
        s.push_str(&format!("diff --git a/{rel} b/{rel}\n", rel = rel_path));
        s.push_str("new file mode 100644\n");
        s.push_str("--- /dev/null\n");
        s.push_str(&format!("+++ b/{rel}\n", rel = rel_path));
        s.push_str("@@ -0,0 +0,0 @@\n");
        return Ok(s);
    }

    let lines: Vec<&str> = content.split('\n').collect();
    // `split('\n')` on a trailing newline gives a final empty element; drop
    // it so we don't emit a phantom blank addition.
    let drop_trailing = content.ends_with('\n');
    let effective_len = if drop_trailing { lines.len() - 1 } else { lines.len() };

    let mut s = String::new();
    s.push_str(&format!("diff --git a/{rel} b/{rel}\n", rel = rel_path));
    s.push_str("new file mode 100644\n");
    s.push_str("--- /dev/null\n");
    s.push_str(&format!("+++ b/{rel}\n", rel = rel_path));
    s.push_str(&format!("@@ -0,0 +1,{} @@\n", effective_len));
    for (i, line) in lines.iter().enumerate() {
        if i >= effective_len {
            break;
        }
        s.push('+');
        s.push_str(line);
        s.push('\n');
    }
    if !drop_trailing {
        // Match git's marker for a file without a trailing newline.
        s.push_str("\\ No newline at end of file\n");
    }
    Ok(s)
}

/// Parse the `@@ -A,B +C,D @@` header. Returns (old_start_line, new_start_line).
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    // Strip the leading "@@ " and trailing " @@ ..." section.
    let inner = line.strip_prefix("@@ ")?;
    let close_idx = inner.find(" @@")?;
    let inner = &inner[..close_idx];
    // inner is now "-A,B +C,D" or "-A +C" etc.
    let mut parts = inner.split_whitespace();
    let old_part = parts.next()?.trim_start_matches('-');
    let new_part = parts.next()?.trim_start_matches('+');
    let old_start: u32 = old_part.split(',').next()?.parse().ok()?;
    let new_start: u32 = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_file_modify() {
        let text = "\
diff --git a/foo.rs b/foo.rs
index abc..def 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,3 @@
 line1
-line2
+line2-new
 line3
";
        let diffs = parse_unified_text(text);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].file_path, "foo.rs");
        assert_eq!(diffs[0].hunks.len(), 1);
        assert_eq!(diffs[0].additions, 1);
        assert_eq!(diffs[0].deletions, 1);
        let lines = &diffs[0].hunks[0].lines;
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[1].origin, '-');
        assert_eq!(lines[1].old_lineno, Some(2));
        assert_eq!(lines[2].origin, '+');
        assert_eq!(lines[2].new_lineno, Some(2));
    }

    #[test]
    fn parse_two_files() {
        let text = "\
diff --git a/a.rs b/a.rs
index 1..2 100644
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old a
+new a
diff --git a/b.rs b/b.rs
index 3..4 100644
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-old b
+new b
";
        let diffs = parse_unified_text(text);
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].file_path, "a.rs");
        assert_eq!(diffs[1].file_path, "b.rs");
        for d in &diffs {
            assert_eq!(d.additions, 1);
            assert_eq!(d.deletions, 1);
        }
    }

    #[test]
    fn parse_binary_diff_yields_empty_hunks() {
        let text = "\
diff --git a/bin b/bin
index abc..def 100644
Binary files a/bin and b/bin differ
";
        let diffs = parse_unified_text(text);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].file_path, "bin");
        assert!(diffs[0].hunks.is_empty());
        assert_eq!(diffs[0].additions, 0);
        assert_eq!(diffs[0].deletions, 0);
    }

    #[test]
    fn parse_hunk_header_basic() {
        assert_eq!(parse_hunk_header("@@ -10,5 +12,7 @@"), Some((10, 12)));
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(
            parse_hunk_header("@@ -1,3 +1,4 @@ fn main() {"),
            Some((1, 1))
        );
    }
}
