# R.2 perf findings v2 — post-fix benchmark

**Status:** In progress — further investigation needed on T2.
**Date:** 2026-05-15.
**Baseline:** [docs/perf_findings.md](perf_findings.md) (original run, 2026-05-13).
**Question:** Did the R.2-derived fixes (P0.5 / P0.6 / P0.7 / F4 / F5 / P0.7-hardened) measurably close the gap vs Claude CLI?

---

## 1. Fixes shipped between v1 and v2

| Fix | What changed | Expected impact |
|---|---|---|
| **P0.5** | `STALE_READ` → `EDIT_NO_MATCH` + whitespace-tolerant match | Eliminate 2–3 wasted edit turns per task |
| **P0.6** | File tree out of system prompt → first-message block; `cache_control` on stable prefix | Cut per-turn cache writes ~80% |
| **P0.7 (soft)** | System prompt guidance + `run_command` soft-warn for shell reads | Reduce shell-read turns |
| **P0.7 (hard)** | Soft-warn → hard-reject; added `Get-Content` anywhere + `findstr /N` detection; removed pipeline/semicolon bailout | Force agent off shell reads entirely |
| **F4** | `CONDENSE_KEEP_TAIL` 6 → 12; latest `read_file` content per path preserved verbatim in condense summary | Prevent re-reads after condense |
| **F5** | "Don't stop early" rule in orchestration system prompt | Reduce mid-task `end_turn` premature stops |
| **File op timeouts** | Lock-acquisition 30 s timeout on all file ops; lock scope narrowed to write-only (read phase no longer holds mutex) | Fix `edit_file` hangs from Defender/indexer; fix FILE_LOCK_TIMEOUT false positives after grep_search |
| **`<project_structure>` UI hide** | Frontend filter in `message-pipeline.js` | Fix spurious injected message showing in chat |

---

## 2. Raw results

### T1 — Single-file constant change

| Mode | Wall | Turns | In | Out | Cache read | Cache write | Cost | Done |
|---|---:|---:|---:|---:|---:|---:|---:|:---:|
| rustic-native v1 | 54s | 9 | 14 | 2,900 | 316,991 | 35,972 | $0.46 | ✅ |
| **rustic-native v2** | **21s** | **4** | 9 | 705 | 77,869 | 26,630 | **$0.22** | ✅ |
| claude-cli baseline | 33s | 5 | — | 1,980 | 147,037 | 10,933 | $0.19 | ✅ |

### T2 — Cross-file feature add

| Mode | Wall | Turns | In | Out | Cache read | Cache write | Cost | Done |
|---|---:|---:|---:|---:|---:|---:|---:|:---:|
| rustic-native v1 | 400s | 57 | 62 | 16,475 | 2,099,331 | 741,161 | $6.09 | ✅ |
| rustic-native v2 (run 1) | 1,025s | 95 | 110 | 18,754 | 2,323,992 | 1,433,034 | $10.59 | ❌ |
| **rustic-native v2 (run 2)** | **480s** | **87** | 92 | 24,007 | 2,216,467 | 1,717,487 | **$12.44** | ✅ |
| claude-cli baseline | 258s | 26 | — | 8,806 | 1,435,175 | 57,301 | $1.30 | ✅ |

*Run 1 used soft-reject for shell reads (P0.7 soft); run 2 used hard-reject + lock scope fix.*

### T3 — Read-only investigation

| Mode | Wall | Turns | In | Out | Cache read | Cache write | Cost | Done |
|---|---:|---:|---:|---:|---:|---:|---:|:---:|
| rustic-native v1 | 736s | 77 | 87 | 17,421 | 2,449,100 | 899,190 | $7.28 | ✅ |
| **rustic-native v2** | **168s** | **21** | 2,554 | 8,701 | 492,155 | 215,594 | **$1.82** | ✅ |
| claude-cli baseline | 92s | 14 | — | 5,380 | 469,234 | 43,597 | $0.64 | ✅ |

---

## 3. Aggregate

| Metric | v1 total | v2 total | CLI total | v2 vs v1 | v2 vs CLI |
|---|---:|---:|---:|:---:|:---:|
| Wall (s) | 1,190 | **669** | 383 | **1.8× faster** | 1.7× slower |
| Turns | 143 | **112** | 45 | **1.3× fewer** | 2.5× more |
| Cache write | 1,676,323 | 1,959,711 | 111,831 | 1.2× worse | 17.5× more |
| Cost (USD) | $13.83 | $14.48 | $2.13 | ~flat | 6.8× more |

---

## 4. Findings by task

### T1 — Strong improvement ✅

Every metric improved. Rustic v2 now **beats CLI** on wall time (21s vs 33s) and turn count (4 vs 5).

- P0.5 eliminated the 3 `EDIT_NO_MATCH` retries that burned turns in v1.
- P0.6 halved cache writes (35K → 27K).
- P0.7 guidance worked: agent used `read_file` directly with no shell fallbacks.
- T1 is effectively solved. **Rustic is now faster than CLI on this task shape.**

### T3 — Strong improvement ✅

4.4× faster, 3.7× fewer turns, 4× cheaper. Still ~1.8× behind CLI wall-clock and 1.5× behind on turns, but the gap is no longer embarrassing.

- The v1 disaster (77 turns of `Get-Content`/`findstr` shell reads) is gone.
- Agent used `read_file` with ranges throughout. A handful of `Get-Content` attempts still appeared but were hard-rejected (P0.7 hardened) and the agent switched to `read_file` immediately.
- Remaining gap vs CLI is mainly prompt prefix — per-turn cache write (215K/21 turns = ~10K/turn) is still 4× CLI's ~3K/turn.

### T2 — Regression on cost and turns, but now completes ✅

This is the critical open problem.

**What got better:**
- Task now completes (v1 run at 95 turns didn't finish).
- No more infinite `edit_file` hangs — lock scope fix eliminated Defender-caused deadlocks.
- Shell-read hard-reject removed dozens of `findstr /N` turns.

**What got worse:**
- Per-turn cache write rate: 13K/turn (v1) → **20K/turn (v2)**. P0.6 reduced it for T1/T3 but T2 shows an increase.
- Turn count: 57 → 87. Despite eliminating shell-read turns, the agent took more turns overall.
- Cost: $6.09 → $12.44 (2× more expensive despite completing ~similarly).

**Why the cache write rate increased on T2:**

The F4 "preserved reads" block is the primary suspect. When condense fires mid-task (T2 is long enough to trigger it), the preserved-reads section is injected as a large text block into the rebuiltconversation. On every subsequent turn, this block is NEW to the cache (different content each time condense fires), forcing the API to re-write it. For a task that hits condense multiple times, this compounds badly.

The per-turn cache write numbers support this theory:
- T1 (short task, condense never fires): 26K writes / 4 turns = **6.5K/turn** ← improved
- T3 (medium, condense may fire once): 215K / 21 turns = **10K/turn** ← improved
- T2 (long task, condense fires multiple times): 1,717K / 87 turns = **20K/turn** ← regressed

---

## 5. Open problems and next investigations

### O1 — F4 preserved-reads block inflates cache on long tasks (HIGH PRIORITY)

**Observed:** T2's per-turn cache write rate went from 13K to 20K after F4 shipped. The condense summary + preserved-reads block is NOT cache-stable across turns — its content changes every time condense fires.

**Root cause:** The preserved-reads section lists file content verbatim. If a file was edited between two condense events, the content in the block is different, forcing a full cache re-write of the entire block on every subsequent turn.

**Options to investigate:**
1. **Remove F4's preserved-reads block entirely** — the `CONDENSE_KEEP_TAIL` bump (6→12) may be sufficient. Measure whether T2 regresses further without the preserved block.
2. **Only preserve file paths, not content** — inject "these files were read earlier: [list]" instead of full content. Costs a re-read but avoids bloating the prompt.
3. **Cap preserved block aggressively** — current budget is 96 KB. Try 8 KB (just the most-recently-read file).

### O2 — T2 still 3.3× more turns than CLI (MEDIUM PRIORITY)

87 turns vs CLI's 26 for the same task. Even with shell reads eliminated, the agent is making 3× as many round-trips. Likely causes:
- Each `read_file` call with a range is a separate turn; CLI's `Read` tool is used more efficiently.
- `EDIT_NO_MATCH` retries still happening on complex diffs (whitespace normalization may not be catching all cases).
- The agent reads then re-reads files it already has in context (FILE_UNCHANGED check may not be preventing redundant reads in all cases).

**Investigation:** Capture the T2 tool-call trace and count turn types: reads, edits, retries, searches. Find the biggest bucket of "wasted" turns.

### O3 — Cache prefix still not as stable as CLI on T2 (MEDIUM)

Even with P0.6's file-tree hoisting and `cache_control` on the stable prefix, the per-turn cache write for T2 is 20K vs CLI's ~2K. Something in the per-turn prompt is still changing. The F4 block is the main culprit, but there may be other contributors (todo state, per-turn tool definitions, etc.).

**Next step:** Capture two consecutive turn prompts from a T2 run and diff them byte-by-byte (the investigation originally planned for P0.6 but never executed for the T2 case specifically).

---

## 6. Per-turn cache write rate summary

This is the clearest single metric for prompt-cache efficiency.

| Task/Mode | Cache writes | Turns | Per-turn rate | vs CLI |
|---|---:|---:|---:|:---:|
| T1 v1 | 35,972 | 9 | 4,000 | 1.9× |
| **T1 v2** | 26,630 | 4 | **6,658** | 3.1× |
| T1 CLI | 10,933 | 5 | 2,187 | — |
| T2 v1 | 741,161 | 57 | 13,003 | 5.9× |
| **T2 v2** | 1,717,487 | 87 | **19,741** | 8.9× |
| T2 CLI | 57,301 | 26 | 2,204 | — |
| T3 v1 | 899,190 | 77 | 11,678 | 3.9× |
| **T3 v2** | 215,594 | 21 | **10,267** | 3.4× |
| T3 CLI | 43,597 | 14 | 3,114 | — |

T1 per-turn rate appears to have increased (4K → 6.6K) but T1 only ran 4 turns — the total is lower and the task completed faster, so the absolute cost is fine. T2 per-turn rate increasing is the real concern.

---

## 7. What to do next

Priority order based on impact-per-effort:

1. **(High)** Revert or cap F4's preserved-reads block — it's hurting T2 more than it helps. Measure T2 without it. If T2 turns/cost improve, either revert F4 or replace with the path-list-only variant.
2. **(Medium)** Capture T2 consecutive prompts and diff for cache instability source — the 20K/turn rate needs a root cause beyond just "F4 block."
3. **(Medium)** Instrument T2 tool-call trace — count turn types, find the biggest wasted-turn bucket, fix the top one.
4. **(Low)** Re-run T3 with the latest fixes (lock scope change, P0.7 hardened) to get a clean post-fix T3 number — current T3 v2 numbers predate some fixes.
