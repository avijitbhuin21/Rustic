// Throwaway spike: validates the walk-time assumption for the changed-files
// tracker design. Walks `cwd` (or a path arg) gitignore-aware, stat-only, and
// reports cold + warm timings plus file/dir counts.
//
// Run from project root:
//   cargo run -p rustic-agent --example walk_timing_spike --release
//   cargo run -p rustic-agent --example walk_timing_spike --release -- "C:\path\to\some\repo"
//
// We measure two passes back-to-back. First = cold-ish OS cache, second = warm.
// The design budget is: warm walk under ~500ms on a real-world repo of ~5,000
// files post-gitignore. If this spike clears that, proceed to schema. If not,
// revisit before writing real code.

use std::path::PathBuf;
use std::time::Instant;

use ignore::WalkBuilder;

const HARD_DENY_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".next",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
];

fn is_hard_denied(name: &str) -> bool {
    HARD_DENY_DIRS.iter().any(|d| *d == name)
}

#[derive(Default, Debug)]
struct WalkStats {
    files: u64,
    dirs: u64,
    bytes: u64,
    skipped_denied: u64,
    errors: u64,
}

fn walk_once(root: &PathBuf) -> WalkStats {
    let mut stats = WalkStats::default();

    let walker = WalkBuilder::new(root)
        .hidden(false)              // we want to see .env etc.; gitignore handles excludes
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(true)
        .filter_entry(|entry| {
            // Hard-deny well-known noise dirs even if not gitignored.
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    if is_hard_denied(name) {
                        return false;
                    }
                }
            }
            true
        })
        .build();

    for result in walker {
        match result {
            Ok(entry) => {
                let Some(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    stats.dirs += 1;
                    if let Some(name) = entry.file_name().to_str() {
                        if is_hard_denied(name) {
                            stats.skipped_denied += 1;
                        }
                    }
                } else if ft.is_file() {
                    // Stat-only: we already have file_type from the walker, but
                    // the design needs mtime + size for the mtime filter, so we
                    // call metadata() to mirror real cost.
                    match entry.metadata() {
                        Ok(md) => {
                            stats.files += 1;
                            stats.bytes += md.len();
                        }
                        Err(_) => stats.errors += 1,
                    }
                }
            }
            Err(_) => stats.errors += 1,
        }
    }

    stats
}

fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    println!("Walk timing spike");
    println!("root: {}", root.display());
    println!();

    // Pass 1 — cold-ish (OS cache state depends on what was just touched)
    let t0 = Instant::now();
    let cold = walk_once(&root);
    let cold_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Pass 2 — warm (just-walked tree, OS cache hot)
    let t1 = Instant::now();
    let warm = walk_once(&root);
    let warm_ms = t1.elapsed().as_secs_f64() * 1000.0;

    // Pass 3 — warm again, sanity check
    let t2 = Instant::now();
    let warm2 = walk_once(&root);
    let warm2_ms = t2.elapsed().as_secs_f64() * 1000.0;

    println!("Pass 1 (cold-ish):  {:>7.1} ms  files={} dirs={} bytes={}",
        cold_ms, cold.files, cold.dirs, cold.bytes);
    println!("Pass 2 (warm):      {:>7.1} ms  files={} dirs={} bytes={}",
        warm_ms, warm.files, warm.dirs, warm.bytes);
    println!("Pass 3 (warm):      {:>7.1} ms  files={} dirs={} bytes={}",
        warm2_ms, warm2.files, warm2.dirs, warm2.bytes);
    println!();
    println!("Skipped (hard-denied): {}", warm.skipped_denied);
    println!("Errors: {}", warm.errors);
    println!();

    // Verdict
    let warm_avg = (warm_ms + warm2_ms) / 2.0;
    let budget_ms = 500.0;
    if warm_avg <= budget_ms {
        println!("VERDICT: warm walk avg = {:.1} ms <= {} ms budget. PROCEED.", warm_avg, budget_ms);
    } else {
        println!("VERDICT: warm walk avg = {:.1} ms > {} ms budget. REVISIT design before coding.", warm_avg, budget_ms);
    }
}
