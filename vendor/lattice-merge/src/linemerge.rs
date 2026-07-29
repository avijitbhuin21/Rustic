//! In-process line-level 3-way merge plus conflict-marker parsing (port of
//! `phase0/structural/linemerge.py`).

use anyhow::{bail, Result};
use diffy::{ConflictStyle, MergeOptions};
use std::borrow::Cow;

const MARKER_LEN: usize = 7;
const OURS_MARKER: &str = "<<<<<<<";
/// diffy labels the diff3 base section `original`; `git merge-file -L base` wrote
/// `base`, and marked text is persisted in divergence records, so keep git's spelling.
const DIFFY_BASE_LINE: &str = "\n||||||| original\n";
const GIT_BASE_LINE: &str = "\n||||||| base\n";

/// One conflict hunk with all three sides' lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    pub ours: Vec<String>,
    pub base: Vec<String>,
    pub theirs: Vec<String>,
}

/// A merged document: plain context segments interleaved with hunks.
#[derive(Clone, Debug)]
pub enum Segment {
    Context(Vec<String>),
    Conflict(Hunk),
}

/// Three-way merge in diff3 style: `Ok` clean text, `Err` conflict-marked text.
fn merge_diff3(base: &str, left: &str, right: &str) -> std::result::Result<String, String> {
    MergeOptions::new()
        .set_conflict_style(ConflictStyle::Diff3)
        .set_conflict_marker_length(MARKER_LEN)
        .merge(base, left, right)
}

/// The text with a final newline, so a last line lacking one cannot have the
/// following conflict marker appended to it.
fn with_final_newline(text: &str) -> Cow<'_, str> {
    if text.is_empty() || text.ends_with('\n') {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(format!("{text}\n"))
    }
}

/// Run an in-process 3-way line merge; returns (conflict_count, diff3-marked output).
pub fn line_merge(base: &str, left: &str, right: &str) -> Result<(i32, String)> {
    let marked = match merge_diff3(base, left, right) {
        Ok(clean) => return Ok((0, clean)),
        Err(conflicted) => {
            let unterminated = [base, left, right]
                .iter()
                .any(|t| !t.is_empty() && !t.ends_with('\n'));
            if unterminated {
                match merge_diff3(
                    &with_final_newline(base),
                    &with_final_newline(left),
                    &with_final_newline(right),
                ) {
                    Ok(clean) => return Ok((0, clean)),
                    Err(reconflicted) => reconflicted,
                }
            } else {
                conflicted
            }
        }
    };
    let conflicts = marked
        .lines()
        .filter(|l| l.starts_with(OURS_MARKER))
        .count();
    Ok((
        i32::try_from(conflicts).unwrap_or(i32::MAX).max(1),
        marked.replace(DIFFY_BASE_LINE, GIT_BASE_LINE),
    ))
}

/// Split diff3-marked merge output into context segments and hunks.
pub fn parse_markers(marked: &str) -> Vec<Segment> {
    let mut doc = Vec::new();
    let mut context: Vec<String> = Vec::new();
    let lines: Vec<&str> = marked.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("<<<<<<<") {
            if !context.is_empty() {
                doc.push(Segment::Context(std::mem::take(&mut context)));
            }
            let mut hunk = Hunk {
                ours: Vec::new(),
                base: Vec::new(),
                theirs: Vec::new(),
            };
            let mut bucket = 0;
            i += 1;
            while i < lines.len() && !lines[i].starts_with(">>>>>>>") {
                if lines[i].starts_with("|||||||") {
                    bucket = 1;
                } else if lines[i].starts_with("=======") {
                    bucket = 2;
                } else {
                    match bucket {
                        0 => hunk.ours.push(lines[i].to_string()),
                        1 => hunk.base.push(lines[i].to_string()),
                        _ => hunk.theirs.push(lines[i].to_string()),
                    }
                }
                i += 1;
            }
            doc.push(Segment::Conflict(hunk));
        } else {
            context.push(lines[i].to_string());
        }
        i += 1;
    }
    if !context.is_empty() {
        doc.push(Segment::Context(context));
    }
    doc
}

/// Render a document whose hunks were all replaced by context back to text.
pub fn render(doc: &[Segment], trailing_newline: bool) -> Result<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in doc {
        match seg {
            Segment::Context(lines) => out.extend(lines.iter().map(String::as_str)),
            Segment::Conflict(_) => bail!("unresolved hunk"),
        }
    }
    let text = out.join("\n");
    Ok(if trailing_newline && !text.is_empty() {
        text + "\n"
    } else {
        text
    })
}
