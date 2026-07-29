//! Performance guards for the merge path: parses are memoized per revision,
//! and a >1 MiB file still merges inside a wall-clock budget.

use lattice_merge::parsers::parse_count;
use lattice_merge::{merge_file, MergeStatus};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `parse_count` is process-global, so the tests that read it must not overlap.
static SERIAL: Mutex<()> = Mutex::new(());

/// A file with `n` trivial functions, the `mark`th one carrying `value`.
fn generated(n: usize, mark: usize, value: &str) -> String {
    let mut out = String::new();
    for i in 0..n {
        let body = if i == mark { value } else { "V0" };
        out.push_str(&format!("fn f{i}() -> u32 {{\n    {body}\n}}\n\n"));
    }
    out
}

#[test]
fn a_merge_parses_a_bounded_number_of_times() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = generated(40, 0, "V0");
    let left = generated(40, 3, "LEFT_V");
    let right = generated(40, 30, "RIGHT_V");

    let before = parse_count();
    let outcome = merge_file(Some("rust"), &base, &left, &right).unwrap();
    let clean_parses = parse_count() - before;
    assert_eq!(outcome.status, MergeStatus::Clean, "{:?}", outcome.strategies);

    // A conflicting merge exercises every strategy tier, which is where a
    // per-hunk or per-strategy re-parse would show up.
    let conflict_left = generated(40, 5, "LEFT_V");
    let conflict_right = generated(40, 5, "RIGHT_V");
    let before = parse_count();
    let outcome = merge_file(Some("rust"), &base, &conflict_left, &conflict_right).unwrap();
    let conflict_parses = parse_count() - before;
    println!(
        "parses: {clean_parses} clean, {conflict_parses} conflicting ({:?})",
        outcome.strategies
    );

    // Budget, not an exact count: the tiers may legitimately be reordered, but
    // parsing must stay a small constant and never scale with file size or
    // hunk count. 40 declarations would blow any per-hunk re-parse budget.
    assert!(
        clean_parses <= 8,
        "clean merge parsed {clean_parses} times; the revisions should be parsed once each"
    );
    assert!(
        conflict_parses <= 16,
        "conflicting merge parsed {conflict_parses} times; strategies must reuse parsed trees"
    );
}

#[test]
fn a_large_file_merges_inside_the_latency_budget() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // ~1.4 MiB, past the 1 MiB chunked-storage threshold.
    let n = 40_000;
    let base = generated(n, 0, "V0");
    assert!(base.len() > 1024 * 1024, "fixture must exceed 1 MiB, was {}", base.len());
    let left = generated(n, 7, "LEFT_V");
    let right = generated(n, n - 7, "RIGHT_V");

    let started = Instant::now();
    let outcome = merge_file(Some("rust"), &base, &left, &right).unwrap();
    let elapsed = started.elapsed();
    println!("large-file merge: {} bytes in {elapsed:?}", base.len());

    assert_eq!(outcome.status, MergeStatus::Clean, "{:?}", outcome.strategies);
    assert!(outcome.text.contains("LEFT_V") && outcome.text.contains("RIGHT_V"));
    let budget = Duration::from_secs(20);
    assert!(elapsed < budget, "large-file merge took {elapsed:?}, budget {budget:?}");
}
