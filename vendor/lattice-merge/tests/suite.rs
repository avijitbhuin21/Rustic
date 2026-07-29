//! Hand-authored semantic merge suite: scenarios whose correct answer is known
//! from the semantics of the change rather than from what any tool happens to
//! produce, scored against three objective oracles.
//!
//! `safety_property` is the CI gate and only fails on WRONG — a clean result
//! that is broken. Conservative conflicts (MISSED) are reported, never failed:
//! the contract is "may be conservative, may not be silently wrong".
//!
//! `comparison_report` scores Lattice against Mergiraf, git and the `all_ours`
//! control, and writes a markdown table to `target/merge-suite-report.md`.

use lattice_merge::hidden::{
    merge_file_checked, merge_file_checked_multibase, merge_files_checked,
};
use lattice_merge::parsers::parses_clean;
use serde::Deserialize;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const MARKER: &str = "<<<<<<<";

/// Per-merge wall-clock ceiling. Overridable for slower machines, but a
/// regression that blows this budget is a failure, not a note.
fn latency_budget() -> Duration {
    let ms: u64 = std::env::var("LATTICE_MERGE_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    Duration::from_millis(ms)
}

/// One file of a multi-file scenario.
#[derive(Deserialize)]
struct ScenarioFile {
    path: String,
    base: Vec<String>,
    left: Vec<String>,
    right: Vec<String>,
}

#[derive(Deserialize)]
struct Scenario {
    id: String,
    lang: String,
    ext: String,
    category: String,
    expect: String,
    rationale: String,
    #[serde(default)]
    base: Vec<String>,
    #[serde(default)]
    left: Vec<String>,
    #[serde(default)]
    right: Vec<String>,
    /// Multi-file scenarios: every file is merged and the checks are pooled
    /// across the whole set, which is where concurrent agents actually break
    /// each other.
    #[serde(default)]
    files: Vec<ScenarioFile>,
    /// Criss-cross scenarios: more than one candidate merge base. A tool that
    /// silently returns a base-dependent answer is wrong, not clever.
    #[serde(default)]
    extra_bases: Vec<Vec<String>>,
    must_contain: Vec<String>,
    must_not_contain: Vec<String>,
    must_contain_once: Vec<String>,
    #[serde(default)]
    lattice_known_wrong: Option<String>,
}

impl Scenario {
    /// The base revision as source text.
    fn base_text(&self) -> String {
        join(&self.base)
    }

    /// The left revision as source text.
    fn left_text(&self) -> String {
        join(&self.left)
    }

    /// The right revision as source text.
    fn right_text(&self) -> String {
        join(&self.right)
    }

    /// True when a clean merge is the correct outcome.
    fn wants_clean(&self) -> bool {
        self.expect == "clean"
    }

    /// True when the scenario spans more than one file.
    fn is_multi_file(&self) -> bool {
        !self.files.is_empty()
    }

    /// Every revision text the scenario declares, for well-formedness checks.
    fn revisions(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.is_multi_file() {
            for f in &self.files {
                out.extend([join(&f.base), join(&f.left), join(&f.right)]);
            }
        } else {
            out.extend([self.base_text(), self.left_text(), self.right_text()]);
        }
        out.extend(self.extra_bases.iter().map(|b| join(b)));
        out
    }
}

/// Renders a line array as newline-terminated source text.
fn join(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Missed,
    Wrong(String),
    Unavailable,
}

impl Verdict {
    /// Short cell label for the report table.
    fn label(&self) -> &str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Missed => "MISSED",
            Verdict::Wrong(_) => "WRONG",
            Verdict::Unavailable => "n/a",
        }
    }
}

/// What a merge tool produced: its text per file, and whether it escalated.
struct ToolResult {
    texts: Vec<String>,
    conflicted: bool,
    note: String,
    elapsed: Duration,
}

impl ToolResult {
    /// Every merged file as one blob, for content assertions.
    fn joined(&self) -> String {
        self.texts.join("\n")
    }
}

/// Loads every scenario file in filename order.
fn load_scenarios() -> Vec<Scenario> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/suite");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("suite directory is readable")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no scenarios found in {}", dir.display());
    paths
        .iter()
        .map(|p| {
            let raw = fs::read_to_string(p).expect("scenario file is readable");
            serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("{} is not a valid scenario: {e}", p.display()))
        })
        .collect()
}

/// Applies the three oracles to one tool's output for one scenario.
fn judge(sc: &Scenario, result: &ToolResult) -> Verdict {
    if result.conflicted {
        return if sc.wants_clean() {
            Verdict::Missed
        } else {
            Verdict::Pass
        };
    }
    if !sc.wants_clean() {
        return Verdict::Wrong("reported clean; the change is not mechanically resolvable".into());
    }
    let text = result.joined();
    if text.contains(MARKER) {
        return Verdict::Wrong("conflict markers inside a result reported clean".into());
    }
    for file_text in &result.texts {
        if !parses_clean(&sc.lang, file_text) {
            return Verdict::Wrong("merged output does not parse".into());
        }
    }
    for needle in &sc.must_contain {
        if !text.contains(needle) {
            return Verdict::Wrong(format!("lost required content: {needle}"));
        }
    }
    for needle in &sc.must_not_contain {
        if text.contains(needle) {
            return Verdict::Wrong(format!("kept superseded content: {needle}"));
        }
    }
    for needle in &sc.must_contain_once {
        let count = text.matches(needle.as_str()).count();
        if count != 1 {
            return Verdict::Wrong(format!("{needle} appears {count} times, expected once"));
        }
    }
    Verdict::Pass
}

/// Runs Lattice's shipped unattended-merge path: single file, multi-file, or
/// multi-base, whichever the scenario declares.
fn run_lattice(sc: &Scenario) -> ToolResult {
    let started = Instant::now();
    if sc.is_multi_file() {
        let files: Vec<(String, String, String, String)> = sc
            .files
            .iter()
            .map(|f| (f.path.clone(), join(&f.base), join(&f.left), join(&f.right)))
            .collect();
        let (per_file, checked) =
            merge_files_checked(&sc.lang, &files).expect("lattice merge does not error");
        return ToolResult {
            texts: per_file.into_iter().map(|(_, o)| o.text).collect(),
            conflicted: !checked.is_clean(),
            note: lattice_note(&checked),
            elapsed: started.elapsed(),
        };
    }
    let checked = if sc.extra_bases.is_empty() {
        merge_file_checked(
            Some(&sc.lang),
            &sc.base_text(),
            &sc.left_text(),
            &sc.right_text(),
        )
        .expect("lattice merge does not error")
    } else {
        let mut bases = vec![sc.base_text()];
        bases.extend(sc.extra_bases.iter().map(|b| join(b)));
        let refs: Vec<&str> = bases.iter().map(String::as_str).collect();
        merge_file_checked_multibase(Some(&sc.lang), &refs, &sc.left_text(), &sc.right_text())
            .expect("lattice merge does not error")
    };
    ToolResult {
        texts: vec![checked.outcome.text.clone()],
        conflicted: !checked.is_clean(),
        note: lattice_note(&checked),
        elapsed: started.elapsed(),
    }
}

/// One-line status for the report: merge status plus any refused defect.
fn lattice_note(checked: &lattice_merge::CheckedMerge) -> String {
    let defects = checked.defects();
    if defects.is_empty() {
        format!("{:?}", checked.outcome.status)
    } else {
        format!("{:?}+{}", checked.outcome.status, defects.join(","))
    }
}

/// Writes the revisions of one file to a scratch directory and returns paths.
fn stage_file(
    sc: &Scenario,
    tag: &str,
    base: &str,
    left: &str,
    right: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir()
        .join("lattice-merge-suite")
        .join(&sc.id)
        .join(tag);
    fs::create_dir_all(&dir).expect("scratch directory is creatable");
    let base_p = dir.join(format!("base.{}", sc.ext));
    let left_p = dir.join(format!("left.{}", sc.ext));
    let right_p = dir.join(format!("right.{}", sc.ext));
    fs::write(&base_p, base).expect("base is writable");
    fs::write(&left_p, left).expect("left is writable");
    fs::write(&right_p, right).expect("right is writable");
    (base_p, left_p, right_p)
}

/// The base/left/right triples an external tool is asked to merge. Multi-base
/// scenarios hand external tools the first base only — that is exactly the
/// single-base assumption under test.
fn triples(sc: &Scenario) -> Vec<(String, String, String, String)> {
    if sc.is_multi_file() {
        sc.files
            .iter()
            .map(|f| (f.path.clone(), join(&f.base), join(&f.left), join(&f.right)))
            .collect()
    } else {
        vec![(
            "file".into(),
            sc.base_text(),
            sc.left_text(),
            sc.right_text(),
        )]
    }
}

/// Runs Mergiraf, or reports it unavailable.
fn run_mergiraf(sc: &Scenario) -> Option<ToolResult> {
    let started = Instant::now();
    let mut texts = Vec::new();
    let mut conflicted = false;
    let mut codes = Vec::new();
    for (i, (tag, base, left, right)) in triples(sc).into_iter().enumerate() {
        let (base_p, left_p, right_p) =
            stage_file(sc, &format!("mergiraf{i}"), &base, &left, &right);
        let out = base_p.with_file_name(format!("mergiraf.{}", sc.ext));
        let status = Command::new("mergiraf")
            .arg("merge")
            .arg(&base_p)
            .arg(&left_p)
            .arg(&right_p)
            .arg("-o")
            .arg(&out)
            .arg("-p")
            .arg(format!("{tag}.{}", sc.ext))
            .output()
            .ok()?;
        let text = fs::read_to_string(&out).unwrap_or_default();
        conflicted |= !status.status.success() || text.contains(MARKER);
        codes.push(status.status.code().unwrap_or(-1).to_string());
        texts.push(text);
    }
    Some(ToolResult {
        texts,
        conflicted,
        note: format!("exit {}", codes.join("/")),
        elapsed: started.elapsed(),
    })
}

/// Runs `git merge-file` as the line-merge floor, or reports it unavailable.
fn run_git(sc: &Scenario) -> Option<ToolResult> {
    let started = Instant::now();
    let mut texts = Vec::new();
    let mut conflicted = false;
    let mut codes = Vec::new();
    for (i, (_, base, left, right)) in triples(sc).into_iter().enumerate() {
        let (base_p, left_p, right_p) = stage_file(sc, &format!("git{i}"), &base, &left, &right);
        let out = Command::new("git")
            .arg("merge-file")
            .arg("-p")
            .arg(&left_p)
            .arg(&base_p)
            .arg(&right_p)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        conflicted |= !out.status.success() || text.contains(MARKER);
        codes.push(out.status.code().unwrap_or(-1).to_string());
        texts.push(text);
    }
    Some(ToolResult {
        texts,
        conflicted,
        note: format!("exit {}", codes.join("/")),
        elapsed: started.elapsed(),
    })
}

/// The degenerate control: always take our own side and never escalate.
fn run_all_ours(sc: &Scenario) -> ToolResult {
    ToolResult {
        texts: triples(sc)
            .into_iter()
            .map(|(_, _, left, _)| left)
            .collect(),
        conflicted: false,
        note: "control".into(),
        elapsed: Duration::ZERO,
    }
}

/// The opposite control: escalate everything, which is never wrong but never useful.
fn run_always_conflict(_sc: &Scenario) -> ToolResult {
    ToolResult {
        texts: vec![String::new()],
        conflicted: true,
        note: "control".into(),
        elapsed: Duration::ZERO,
    }
}

#[test]
fn safety_property() {
    let scenarios = load_scenarios();
    let mut regressions = Vec::new();
    let mut stale = Vec::new();
    let mut known = Vec::new();
    let mut missed = Vec::new();
    let mut slow = Vec::new();
    let mut worst = Duration::ZERO;
    for sc in &scenarios {
        let result = run_lattice(sc);
        if result.elapsed > worst {
            worst = result.elapsed;
        }
        if result.elapsed > latency_budget() {
            slow.push(format!("  {} — {:?}", sc.id, result.elapsed));
        }
        let verdict = judge(sc, &result);
        match (&verdict, &sc.lattice_known_wrong) {
            (Verdict::Wrong(reason), None) => regressions.push(format!("  {} — {reason}", sc.id)),
            (Verdict::Wrong(_), Some(_)) => known.push(sc.id.clone()),
            (Verdict::Pass, Some(_)) => stale.push(sc.id.clone()),
            (Verdict::Missed, _) => missed.push(sc.id.clone()),
            _ => {}
        }
    }
    println!(
        "lattice: {} scenarios | {} conservative misses ({}) | {} known gaps ({}) | slowest merge {:?} (budget {:?})",
        scenarios.len(),
        missed.len(),
        missed.join(", "),
        known.len(),
        known.join(", "),
        worst,
        latency_budget(),
    );
    assert!(
        regressions.is_empty(),
        "merges reported clean but semantically broken:\n{}",
        regressions.join("\n")
    );
    assert!(
        stale.is_empty(),
        "these scenarios now pass — drop their lattice_known_wrong marker: {}",
        stale.join(", ")
    );
    assert!(
        slow.is_empty(),
        "per-merge latency budget exceeded:\n{}",
        slow.join("\n")
    );
}

#[test]
fn scenarios_are_well_formed() {
    let scenarios = load_scenarios();
    for sc in &scenarios {
        assert!(
            sc.expect == "clean" || sc.expect == "conflict",
            "{}: expect must be clean or conflict",
            sc.id
        );
        assert!(
            sc.category == "should-merge" || sc.category == "must-conflict",
            "{}: unknown category {}",
            sc.id,
            sc.category
        );
        assert_eq!(
            sc.wants_clean(),
            sc.category == "should-merge",
            "{}: category and expect disagree",
            sc.id
        );
        assert!(
            sc.rationale.len() > 40,
            "{}: rationale must justify why this answer is correct",
            sc.id
        );
        assert_eq!(
            sc.is_multi_file(),
            sc.base.is_empty(),
            "{}: declare either base/left/right or files, not both or neither",
            sc.id
        );
        assert!(
            !sc.is_multi_file() || sc.extra_bases.is_empty(),
            "{}: multi-file and multi-base are not combined by the runner",
            sc.id
        );
        for text in sc.revisions() {
            assert!(
                parses_clean(&sc.lang, &text),
                "{}: every declared revision must parse",
                sc.id
            );
        }
        if sc.wants_clean() {
            assert!(
                !sc.must_contain.is_empty() || !sc.must_contain_once.is_empty(),
                "{}: a should-merge scenario needs at least one content assertion",
                sc.id
            );
        }
    }
    for lang in ["rust", "typescript", "tsx", "python", "javascript"] {
        assert!(
            scenarios.iter().any(|s| s.lang == lang),
            "every supported grammar needs coverage; {lang} has none"
        );
    }
    assert!(
        scenarios.iter().any(|s| s.is_multi_file()),
        "the suite must contain at least one multi-file scenario"
    );
    assert!(
        scenarios.iter().any(|s| !s.extra_bases.is_empty()),
        "the suite must contain at least one criss-cross scenario"
    );
}

#[test]
fn comparison_report() {
    let scenarios = load_scenarios();
    let tools = ["lattice", "mergiraf", "git", "all_ours", "always_conflict"];
    let mut rows = Vec::new();
    let mut tally = vec![[0usize; 4]; tools.len()];

    for sc in &scenarios {
        let results = [
            Some(run_lattice(sc)),
            run_mergiraf(sc),
            run_git(sc),
            Some(run_all_ours(sc)),
            Some(run_always_conflict(sc)),
        ];
        let verdicts: Vec<Verdict> = results
            .iter()
            .map(|r| match r {
                Some(result) => judge(sc, result),
                None => Verdict::Unavailable,
            })
            .collect();
        for (i, verdict) in verdicts.iter().enumerate() {
            match verdict {
                Verdict::Pass => {
                    tally[i][0] += 1;
                    if sc.wants_clean() {
                        tally[i][3] += 1;
                    }
                }
                Verdict::Missed => tally[i][1] += 1,
                Verdict::Wrong(_) => tally[i][2] += 1,
                Verdict::Unavailable => {}
            }
        }
        let (lattice_note, lattice_text) = results[0]
            .as_ref()
            .map(|r| (r.note.clone(), r.joined()))
            .unwrap_or_default();
        rows.push((sc, verdicts, lattice_note, lattice_text));
    }

    let mut md = String::new();
    let should_merge = scenarios.iter().filter(|s| s.wants_clean()).count();
    let must_conflict = scenarios.len() - should_merge;
    md.push_str(
        "| # | scenario | expected | lattice | mergiraf | git | all_ours | always_conflict |\n",
    );
    md.push_str(
        "|---|----------|----------|---------|----------|-----|----------|-----------------|\n",
    );
    for (i, (sc, verdicts, _, _)) in rows.iter().enumerate() {
        let _ = writeln!(
            md,
            "| {} | `{}` | {} | {} | {} | {} | {} | {} |",
            i + 1,
            sc.id,
            sc.expect,
            verdicts[0].label(),
            verdicts[1].label(),
            verdicts[2].label(),
            verdicts[3].label(),
            verdicts[4].label()
        );
    }
    let _ = write!(
        md,
        "\n| tool | solved /{should_merge} | escalated /{must_conflict} | MISSED | WRONG |\n\
         |------|------------|---------------|--------|-------|\n"
    );
    for (i, name) in tools.iter().enumerate() {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} |",
            name,
            tally[i][3],
            tally[i][0] - tally[i][3],
            tally[i][1],
            tally[i][2]
        );
    }
    md.push_str("\nWRONG detail:\n");
    for (sc, verdicts, _, _) in &rows {
        for (i, verdict) in verdicts.iter().enumerate() {
            if let Verdict::Wrong(reason) = verdict {
                let _ = writeln!(md, "- {} / {}: {}", sc.id, tools[i], reason);
            }
        }
    }
    md.push_str("\nKnown Lattice gaps, with the output we actually emit:\n");
    for (sc, _, _, text) in &rows {
        if let Some(reason) = &sc.lattice_known_wrong {
            let _ = writeln!(md, "\n**{}** — {}\n\n```\n{}```", sc.id, reason, text);
        }
    }
    md.push_str("\nLattice status per scenario:\n");
    for (sc, _, note, _) in &rows {
        let _ = writeln!(md, "- {}: {}", sc.id, note);
    }

    let mergiraf_missing = rows
        .iter()
        .all(|(_, verdicts, _, _)| verdicts[1] == Verdict::Unavailable);
    if mergiraf_missing {
        md.push_str(
            "\n> `mergiraf` was not on PATH for this run, so its column reads `n/a`.\n\
             > Install it (`cargo install mergiraf`) to restore the comparison — the\n\
             > numbers above are Lattice-versus-git only.\n",
        );
    }

    println!("{md}");
    let dest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/merge-suite-report.md");
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&dest, &md);

    // The controls are what make the metric trustworthy: publish them beside
    // every number, and fail if either stops collapsing, because then the
    // suite has become one a trivial policy can win.
    let ours = tools.iter().position(|t| *t == "all_ours").unwrap();
    let never = tools.iter().position(|t| *t == "always_conflict").unwrap();
    assert!(
        tally[ours][2] * 2 > scenarios.len(),
        "all_ours must be silently wrong on most scenarios, was {} of {}",
        tally[ours][2],
        scenarios.len()
    );
    assert_eq!(
        tally[never][3], 0,
        "always_conflict must solve nothing; it solved {}",
        tally[never][3]
    );
    assert_eq!(
        tally[never][2], 0,
        "always_conflict cannot be WRONG; a zero there is purchasable by refusing to merge"
    );
    let lattice = tools.iter().position(|t| *t == "lattice").unwrap();
    assert!(
        tally[lattice][3] > tally[ours][3] && tally[lattice][2] < tally[ours][2],
        "lattice must beat the all_ours control on both axes"
    );
}
