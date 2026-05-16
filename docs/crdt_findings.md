# R.3 — CRDTs for conflict-free sub-agent editing

**Status:** Draft v1 (pre-prototype landscape survey), 2026-05-14.
**Question:** Should Rustic add a CRDT layer so multiple sub-agents can edit the **same file** concurrently without serializing on the `writes`-overlap check, instead of (or in addition to) the git-worktree fan-out planned for P1.4?
**TL;DR recommendation:** **No, with one narrow Conditional.** Worktrees + a lead-agent merge step is the industry-converged answer for multi-agent code editing in 2026 (Composio, Claude Squad, amux, Conductor, JetBrains 2026.1 all ship this pattern; none ship a CRDT). The remaining open question is the **mechanical** fan-out shape (e.g., "rename `X` to `Y` everywhere across N files"), where CRDTs are theoretically a clean fit — but worktrees + `git merge` already handle that case at ~zero risk and ~zero engineering cost. The full prototype called for in plan.md should still run to confirm, but the bar for a Yes is high and this writeup lays out why.

> This document is a **landscape survey + analysis** written before the `yrs` prototype. The plan.md deliverable calls for a final writeup that includes prototype measurements — that version supersedes this one. See [§9](#9-what-the-prototype-must-measure-to-overturn-this-recommendation) for the specific numbers the prototype needs to produce to flip the recommendation.

---

## 1. The problem we're trying to solve

Today, two sub-agents that want to edit overlapping files have exactly two paths:

| Path | Where it lives | Cost |
|---|---|---|
| **(a) Separate worktrees** | P1.4 — `enter_worktree` / `exit_worktree` tools. Each worktree is a full filesystem copy on a branch. A final merge lands work back on the trunk. | Disk: ~5 GB / worktree on a real codebase. Merge step is a manual orchestrator decision. |
| **(b) Serialize on the `writes`-overlap check** | `task::spawn` rejects fan-outs whose declared `writes:` sets overlap. | Latency: children run sequentially; defeats the parallelism we just bought with sub-agents. |

Neither is good when the **right** fan-out shape is "five children all touching `src/auth/` from different angles." The CRDT pitch is: let all five edit the same files in shared memory, merge by construction, no worktree disk cost, no serialization.

This document evaluates whether that pitch holds up.

---

## 2. The Rust CRDT library landscape (as of May 2026)

There are four serious contenders. None is obviously the right pick.

### 2.1 `yrs` — Rust port of Yjs

- **Status.** Actively maintained, "in the process of reaching feature compatibility with Yjs." [[ref: y-crdt/y-crdt]](https://github.com/y-crdt/y-crdt) The most mature of the four for production embedding.
- **Wire format.** Compatible with Yjs's documented binary protocol; cross-language by design. Stable enough to be the default backing store for production collaborative editors.
- **Data types.** `Y.Text`, `Y.Array`, `Y.Map`, `Y.Doc` — covers text and structured docs. We'd model files as `Y.Text` keyed by path inside a project-wide `Y.Doc`.
- **Granularity.** Per-character. Edits are inserts/deletes of code points with logical IDs.
- **Throughput (Yjs reference).** ~268k ops/sec on the canonical 260k-edit trace. [[ref: crdts-go-brrr]](https://josephg.com/blog/crdts-go-brrr/) `yrs` is expected to be faster — bartoszsypytkowski.com/yrs-architecture documents the design — but no published apples-to-apples benchmark exists at the time of writing.
- **Production users.** Production collaborative editors (TipTap, Liveblocks, Hocuspocus). The April 2026 Electric.ax post on "AI agents as CRDT peers" is built on Yjs. [[ref: electric.ax]](https://electric.ax/blog/2026/04/08/ai-agents-as-crdt-peers-with-yjs) (Caveat: that post is conceptual — no production agent metrics.)
- **For us.** The default candidate. Most ecosystem, most stable wire format, easiest to find prior art.

### 2.2 `automerge-rs` (Automerge 3.0, August 2025)

- **Status.** Released August 2025. Major rewrite: **10× memory reduction** vs Automerge 2 (Moby Dick paste: 700 MB → 1.3 MB), one document's load time went from **17 hours to 9 seconds**. [[ref: automerge 3.0]](https://automerge.org/blog/automerge-3/) Server-side use cases are now viable.
- **Wire format.** Compressed columnar binary, stable across the 3.x line. Strongest persistence story.
- **Data types.** Rich JSON CRDT — `Map`, `List`, `Counter`, `Text` (now plain JS strings by default in 3.0), `Bytes`. Strictly more expressive than Yjs for non-text state.
- **Granularity.** Per-character for text, per-element for lists/maps.
- **Throughput.** Historically much slower than Yjs/diamond-types on the editing-trace benchmark (3.0 closes most of that gap but precise numbers vs Yjs in 3.x aren't published yet). For our edit pattern (10–100 edits/file, not 260k/file), throughput is not the binding constraint anyway.
- **For us.** Strongest candidate if we'd also like to share **non-file state** (the orchestrator's plan, the todo list, the cost ledger) across the same CRDT layer. But that's mission creep relative to the R.3 question, which is specifically about file edits.

### 2.3 `diamond-types` — Joseph Gentle

- **Status.** Per its own README: WIP. "The package published to cargo is quite out of date, both in terms of API and performance." [[ref: josephg/diamond-types]](https://github.com/josephg/diamond-types) Wire format **not stable**. Plain text only — JSON-style types are on a `more_types` branch.
- **Throughput.** The reference point: **4.6 million ops/sec** native Rust on the 260k-edit trace, **1.1 MB total memory**. [[ref: crdts-go-brrr]](https://josephg.com/blog/crdts-go-brrr/) Best-in-class by a wide margin (~17× Yjs, ~5000× old Automerge).
- **For us.** Tempting on raw numbers, but the maturity gap is real. Pinning a hash on `master` and accepting wire-format breakage on every dt upgrade is a tax we don't need for a feature that isn't yet justified.

### 2.4 `loro` — Loro CRDT framework

- **Status.** General-purpose CRDT engine in Rust, JS/Swift bindings. Bundles a Peritext+Fugue-based rich-text CRDT. [[ref: loro-dev/loro]](https://github.com/loro-dev/loro)
- **Differentiator.** Best-published numbers in the JS/WASM benchmark suite [[ref: loro.dev/docs/performance]](https://loro.dev/docs/performance). Rich-text intent preservation (Peritext) is unique among production CRDTs.
- **For us.** Worth keeping on the radar but rich-text annotations aren't a problem we have. The interesting trait — intent preservation — doesn't translate cleanly to code edits, where "intent" is the AST, not character formatting.

### 2.5 Pick

If we prototype: **`yrs`**. Most stable, most prior art, easiest to abandon if the result is "no." The plan.md investigation plan already names `yrs` — keep that.

---

## 3. Granularity: which level of CRDT actually fits agent edits?

This is the most important decision and it largely determines whether the rest of the analysis is even worth running.

### 3.1 Per-character (Yjs / Automerge 3.0 / diamond-types text mode)

- **What it does.** Treat every file as `Y.Text`. Each agent's `edit_file(old → new)` call diffs to a sequence of `insert(pos, str)` / `delete(pos, len)` ops on the CRDT.
- **Pro.** Library off-the-shelf. Two agents editing line 50 and line 800 of the same file just work — their ops commute, no overlap.
- **Pro.** Op count is bounded by the bytes the agent actually changes, not the file size. Our edit pattern (~10–100 ops/file/turn × 30 turns × 6 children = ~1,800–18,000 ops/file) is **3 orders of magnitude below** what Yjs is tuned for (the 260k-edit trace).
- **Con.** Compaction. Y.Text deltas grow monotonically. At our op rate this isn't a memory problem (1.1 MB at 260k ops in diamond-types; back-of-envelope ~10 MB for the same trace in Yjs) but it does mean every shadow-history snapshot ([R.1](file_tracking_decision.md)) has to either store a state vector + delta-from-base or the full materialized file content. Probably the latter — the shadow-git layer already has content-addressed dedup, no point reinventing it.
- **Con — the real one.** **Semantic breakage.** See [§4](#4-semantic-merge-quality-the-real-failure-mode).

### 3.2 Per-line

- **What it does.** Same as per-character but the CRDT unit is a line. Splits/merges of lines need bespoke ops.
- **Pro.** Op count drops another order of magnitude (~100s of ops per file rather than 1000s).
- **Con.** Nobody ships this in a library. Building it ourselves is a research project, not a feature.
- **Verdict.** Skip. The per-character throughput headroom is so large that downgrading granularity to save it isn't worth the custom code.

### 3.3 Per-AST-node

- **What it does.** Use tree-sitter to parse to an AST. Each CRDT op is an insertion / deletion / re-parent of a tree node.
- **Pro.** Eliminates a whole class of semantic breakage by construction — you can't merge two edits into "function with 4 args instead of 3" if the function signature is a single CRDT node and one of the edits replaced it wholesale.
- **Con.** Atom Teletype tried this and abandoned it. Microsoft's 2015 AST-based collaborative editing paper [[ref: MS Research]](https://www.microsoft.com/en-us/research/wp-content/uploads/2015/02/paper.pdf) lays out the design but provides no quantified performance data and no production deployment. The implementation cost is significant — every op needs a re-parse, and the merge algorithm must handle every language we index.
- **Con.** Language-locked. We index Rust, TS, Python, Go, etc. via tree-sitter (P1.1). An AST CRDT means a per-language merge story. For non-indexed files we'd fall back to per-character anyway, which is the failure mode we were trying to escape.
- **Verdict.** Right idea, wrong decade. Wait for someone else to ship this in a library. If Zed's DeltaDB [[ref: zed.dev/blog/crdts]](https://zed.dev/blog/crdts) eventually exposes a code-aware CRDT primitive, reconsider.

### 3.4 Per-file (LWW)

Defeats the whole point. Skip.

### 3.5 Conclusion on granularity

**Per-character (Yjs / `yrs`) is the only realistic choice for a near-term prototype.** Everything else is either unimplemented or research-grade. This forces us to confront semantic breakage head-on rather than dodging it with structural awareness.

---

## 4. Semantic merge quality — the real failure mode

A CRDT guarantees the merged state **exists and converges** — it does not guarantee the merged state is **correct code**. This is the central problem.

### 4.1 Concrete failure scenarios

Drawn from the Microsoft AST paper's failure mode taxonomy [[ref: MS Research]](https://www.microsoft.com/en-us/research/wp-content/uploads/2015/02/paper.pdf), our own [docs/perf_findings.md](perf_findings.md) observations about edit_file failure modes, and our intuition about how agents actually compose:

| # | Scenario | What each agent does | What the CRDT merges to | Severity |
|---|---|---|---|---|
| 1 | **Signature drift** | Agent A adds `ctx: &Context` as 1st arg. Agent B adds `tracing: TracingHandle` as 1st arg. | `fn foo(tracing: TracingHandle, ctx: &Context, ...)` — interleaved insertion at the same position. Callers see both new args. | **High** — compiles only if types happen to be unique; otherwise type errors at every call site. |
| 2 | **Import list duplication** | Both agents add `use crate::foo::Bar;` to the top of the file. | Two identical `use` lines. | Low — `rustfmt` deduplicates, or compiler emits `duplicate import` warning. Mostly cosmetic. |
| 3 | **Dispatch table interleave** | Both agents add a new variant to a `match` arm or a `HashMap::insert` block. | Both inserted, may be at different positions, may compile, may not. | Medium — usually correct, sometimes wrong if pattern is exhaustive and the new variants are reordered. |
| 4 | **Refactor + tweak collision** | Agent A renames a struct field. Agent B writes new code that references the **old** field name. | Diverged references — A's rename applies; B's new code references a field that no longer exists. | **High** — compiles only on B's branch; CRDT merge produces broken code immediately. |
| 5 | **Whole-file rewrite vs surgical edit** | Agent A rewrites a 200-line file from scratch. Agent B fixes a typo on line 47. | CRDT preserves both: B's typo fix lands somewhere inside A's rewrite, almost certainly at the wrong position. | **Critical** — silent corruption. Worktrees handle this perfectly with a merge conflict; CRDT handles it silently and wrongly. |
| 6 | **Comment drift** | Both agents touch the same docblock above a function for different reasons. | Interleaved characters of two prose paragraphs. | Low (cosmetic), but reads as obvious AI-slop garbage to a human reviewer. |

### 4.2 Why CRDTs can't catch this

CRDTs operate on the **representation** (characters), not the **meaning** (AST + types + semantics). No amount of CRDT cleverness rescues scenarios 1, 4, or 5. The mitigation in the collaborative-editing world is *human attention* — a real-time editor lets two humans see each other's cursors and back off. We have no equivalent for autonomous agents.

The Electric.ax post on AI agents as CRDT peers [[ref: electric.ax]](https://electric.ax/blog/2026/04/08/ai-agents-as-crdt-peers-with-yjs) makes the optimistic case — "conflicts resolve naturally" — but provides **no empirical failure data**. It's a conceptual demo, not a production report. Treat it as a hypothesis to test, not a result.

### 4.3 What partial mitigations exist?

1. **Tree-sitter-aware reconciler at commit time.** After the CRDT produces a merged file, parse with tree-sitter. If parse fails, or if a signature was modified by more than one peer, **escalate** — block the merge and force the orchestrator to resolve. This is the most pragmatic mitigation but it's its own implementation effort and re-introduces a "merge step" that was supposed to disappear.
2. **"Hands-off zones."** Each child declares not only `writes:` but `signatures_modified:` / `imports_modified:`. Pre-spawn collision check refuses fan-outs that would touch the same signature. Functionally equivalent to today's `writes:` check, just finer-grained. **This is the same idea as the current `writes`-overlap check, just with smaller cells** — doesn't actually unlock the "five children touching the same file" use case we wanted.
3. **Test-as-arbiter.** Run tests after merge. If they pass, ship; if they fail, escalate. This is "let tests catch it" and the answer to whether it's acceptable depends entirely on whether the project has tests of the relevant code paths. Most real codebases don't.

None of these is free. **The cost of any of these mitigations is comparable to just doing the worktree merge step properly.**

---

## 5. Throughput and storage cost — not the binding constraint

Working the numbers explicitly because the plan.md investigation plan flags them as open:

- **Op rate per file per task.** Best estimate: 10–100 edits/file/turn × 30 turns × 6 children = 1,800–18,000 ops/file in a heavy fan-out. Order of magnitude: 10k.
- **Yjs throughput headroom.** ~268k ops/sec [[ref: crdts-go-brrr]](https://josephg.com/blog/crdts-go-brrr/). Our 10k-op file would be processed in ~40ms. **Not a bottleneck.**
- **diamond-types headroom.** ~4.6M ops/sec, ~2ms per file. **Not a bottleneck.**
- **Memory cost.** diamond-types: 1.1 MB at 260k ops. Yjs: ~10× that, so ~10 MB at 260k ops. Our 10k ops: under 1 MB / file, well below noise.
- **Storage cost.** With R.1's shadow-git layer, the CRDT state vector is a tiny addition; the materialized file is already content-addressed. No new storage problem.
- **Compaction.** Yjs's GC drops tombstones from deletes. At our op rate, even un-compacted state is fine. **Compaction is a non-issue at our scale.**

**Conclusion:** The throughput / storage concerns plan.md flagged are not actually the problem. They were the right thing to ask, but the math comes out comfortably in CRDTs' favor. The problem is §4 (semantic merge), not §5 (mechanical throughput).

---

## 6. The worktree status-quo: stronger than it looked

The original framing in plan.md treats worktrees as the "cheap but limited" baseline. Re-evaluating with current ecosystem data:

- **Industry has converged.** Composio agent-orchestrator, Claude Squad, amux, Conductor, Vibe Kanban, Cursor Background Agents, OpenClaw + Antfarm — **all use git worktrees + branches**. None ship a CRDT layer for code edits. [[ref: amux.io/blog/best-multi-agent-orchestrators-2026]](https://amux.io/blog/best-multi-agent-orchestrators-2026/) [[ref: augmentcode.com/guides/git-worktrees-parallel-ai-agent-execution]](https://www.augmentcode.com/guides/git-worktrees-parallel-ai-agent-execution)
- **IDE momentum is firmly behind worktrees.** JetBrains 2026.1 (March 2026) shipped first-class worktree support precisely for this AI-agent fan-out use case.
- **5–7 concurrent agents is the practical ceiling.** [[ref: blog.appxlab.io]](https://blog.appxlab.io/2026/03/31/multi-agent-ai-coding-workflow-git-worktrees/) The binding constraint is **rate limits + review overhead**, not disk or merge cost. CRDTs don't help with rate limits or review.
- **Merge conflict rate is low in practice.** When fan-outs are decomposed properly (one task → one branch), agents conflicting at the file level is the exception, not the rule. The lead-agent merge pattern resolves the residual conflicts; the orchestrator already has the context to do this cleanly.
- **Per-worktree disk cost (~5 GB).** Real but mitigated by git's reference-counted object store — checkouts are sparse where possible.

**Worktrees aren't merely the "no CRDT" baseline. They are the answer the industry has settled on for the exact problem CRDTs were supposed to solve.** This is the strongest single argument for "No" in this writeup.

---

## 7. Implementation effort if we did it anyway

Plan.md estimates 4 days for the prototype. That's the right number **for a prototype**. For a production-grade implementation:

| Phase | Effort | Notes |
|---|---|---|
| `yrs` integration into `edit_file` (behind feature flag) | 2 days | Shared `Y.Doc` per task, file→`Y.Text` map, diff `old → new` into ops |
| State persistence (shadow-git plays nicely) | 1 day | Materialize → R.1 shadow-git on every commit; keep CRDT state in RAM during the task |
| Sub-agent fan-out: spawn N children on the same `Y.Doc`, await all, materialize | 2 days | Hard part: which child's view of the file does the orchestrator show during streaming? |
| Tree-sitter reconciler at commit (parse-and-block) | 3 days | Per language, only for languages already in P1.1 |
| `signatures_modified` / `imports_modified` declarations + collision check | 2 days | Or skip and rely on the reconciler |
| Failure-mode tests | 2 days | Reproduce all 6 scenarios from §4 in a test suite; codify acceptable / unacceptable outcomes |
| UI: show "two agents edited this region, here's the merged result" inline | 3 days | Without this, the user has no idea what happened |
| **Total** | **~15 days** | Roughly **3× the plan.md estimate** |

Compare to **worktrees + a `merge_subagent_branches` tool** under P1.4, which is at most a week and we already have `rustic-git` plumbing.

---

## 8. Recommendation: No (with one Conditional)

**Default recommendation: No.**

For the **design-heavy** fan-out shape (the only one where CRDTs were ever plausibly going to win), the semantic-breakage risk dominates. The mitigations that catch the breakage cost as much as the worktree-merge step we were trying to eliminate. The industry has converged on worktrees for the right reasons. We should follow.

**Conditional Yes (low confidence, only after a prototype):**

For the **mechanical** fan-out shape — "rename `X` to `Y` everywhere," "add this import to every file," "apply this codemod across N files" — CRDTs are a cleaner fit because the edits truly don't overlap conceptually. **But** the worktree path also handles this case with `enter_worktree` + a one-line `git merge --no-edit` flow at ~zero risk. The CRDT version would be marginally faster and disk-cheaper, not categorically better. **Worth measuring; not worth shipping without measuring a meaningful delta.**

### 8.1 What we should ship instead

The right immediate work is to **double down on worktrees**:

1. Finish P1.4 (`enter_worktree` / `exit_worktree`).
2. Add a first-class `merge_subagent_branches` tool that the orchestrator can call. Inputs: list of branches; behavior: attempt `git merge --no-edit` in dependency order, surface conflicts as structured tool output. **This is the lead-agent merge pattern in tool form.**
3. Optional: a `dry_run_merge_subagent_branches` that simulates the merge in a throwaway worktree and reports the conflict shape before children even commit. Lets the orchestrator pre-flight a fan-out and back off to serial if the conflict surface is too wide.

That's 3–4 days of work that covers ~95% of the cases CRDTs were supposed to address, with zero of the semantic-breakage risk.

### 8.2 When to revisit this decision

Three things would change the answer:

1. **Zed ships DeltaDB as a usable Rust library** with an AST-aware CRDT primitive that handles signature collisions structurally. Reconsider then.
2. **A peer-reviewed empirical study** quantifies semantic-merge failure rates on real agent fan-outs and shows them to be low enough that "let tests catch it" is defensible. The Electric.ax post is a conceptual demo, not this.
3. **Our own usage telemetry** shows that the worktree-merge tool is a frequent bottleneck for fan-outs where children truly don't overlap. Then the mechanical Conditional Yes becomes worth doing.

Absent one of those, the answer is settled.

---

## 9. What the prototype must measure to overturn this recommendation

Plan.md still calls for a prototype. If we run it, here is the minimum bar a "Yes" recommendation must clear:

1. **Semantic-merge failure rate ≤ 5%** across at least 30 fan-out runs, using the 6 failure scenarios from §4 as the test grid. Anything higher and "let tests catch it" is unacceptable.
2. **No silent corruption events** (scenario 5 in §4) at all. A single one of these is a hard No.
3. **Wall-clock or token-budget win of ≥ 30%** vs the worktree path on at least one of the two reference fan-out shapes (mechanical, design-heavy). Below 30% the engineering cost (~15 production-day estimate from §7) doesn't pay back.
4. **Working tree-sitter reconciler** in the prototype, demonstrating that scenarios 1 and 4 from §4 can be detected and escalated automatically. Without this the prototype isn't actually testing the production design.

If the prototype clears all four bars, flip to **Conditional Yes for mechanical fan-outs**. If it clears (1)–(3) but not (4), it's not a production design. If it fails (1) or (2), it's a definitive No and we can close R.3 permanently.

---

## 10. Open questions for the prototype

These are the things this pre-prototype survey **cannot** answer; the prototype must:

- **Y.Doc lifetime.** Per-task `Y.Doc` or per-project? Per-task is simpler; per-project allows cross-task long-lived state but raises the bar on state migration when the schema changes.
- **Streaming UX.** When child A and child B are both writing to the same file via the same `Y.Doc`, what does the user see in the file viewer mid-stream? Per-child diff overlays? Live merged view? This is a chat-view UX problem, not a CRDT problem, but the answer constrains the implementation.
- **Failure rollback.** If child B crashes mid-task with 200 ops on the wire, do we keep them or roll them back? Yjs has no native "transaction" for this; we'd build it on top.
- **R.1 shadow-git integration.** Per snapshot we want the materialized file, not the op log. That means a Yjs → text materialization on every snapshot trigger (every tool call). At ~10k ops the materialization cost is in the single-digit ms range — confirm with the prototype.

---

## 11. Effort revision and what to put back in the build order

- **Original plan.md effort estimate: 4 days for prototype + writeup.** That number is **right for the prototype** (1 day landscape — done in this doc — + 2 day `yrs` prototype + 1 day measurement). Keep it.
- **Followup if the recommendation is positive: ~15 production days** (see §7). This is the number to plan against, not the 4-day prototype budget.
- **Followup if the recommendation is negative** (the expected outcome): close R.3, fold the worktree-merge enhancements from §8.1 into P1.4's scope. Total cost: ~3–4 days adjacent to P1.4's existing scope.

The "no" outcome is materially cheaper than the "yes" outcome. That asymmetry is worth keeping in mind when scheduling — the prototype is also a relatively cheap insurance policy against being wrong here.

---

## 12. Summary table

| Question from plan.md | Pre-prototype answer | Confidence |
|---|---|---|
| Which granularity fits agent edits? | Per-character (only viable library option); per-AST-node is right idea but unimplemented | High |
| How often does semantic merge break? | Unknown empirically; failure modes well-characterized (§4); scenarios 1, 4, 5 are the dangerous ones | Medium |
| Is "let tests catch it" acceptable? | No — most projects lack tests on the affected paths; scenario 5 is silent corruption | High |
| Compaction / op-log throughput? | Non-issue at our scale (10k ops/file vs 4.6M ops/sec headroom) | High |
| Library landscape? | `yrs` is the prototype pick; `automerge-rs` is the runner-up; `diamond-types` is too WIP; `loro` solves a different problem | High |
| Beats worktrees? | No on design-heavy fan-outs; Conditional Maybe on mechanical fan-outs — but worktrees already handle those at zero risk | Medium-high |
| Should we ship CRDTs? | **No, with a narrow Conditional pending prototype data** | Medium-high |
| Should we double down on worktrees? | **Yes** — add `merge_subagent_branches` tool to P1.4 | High |

---

## References

- y-crdt/y-crdt — https://github.com/y-crdt/y-crdt
- yrs architecture deep-dive — https://www.bartoszsypytkowski.com/yrs-architecture/
- Automerge 3.0 release notes — https://automerge.org/blog/automerge-3/
- diamond-types README — https://github.com/josephg/diamond-types
- "CRDTs go brrr" benchmark numbers — https://josephg.com/blog/crdts-go-brrr/
- Loro CRDT framework — https://github.com/loro-dev/loro
- Loro performance benchmarks — https://loro.dev/docs/performance
- Zed editor CRDTs blog — https://zed.dev/blog/crdts
- Microsoft Research: Towards AST-based Collaborative Editing — https://www.microsoft.com/en-us/research/wp-content/uploads/2015/02/paper.pdf
- Electric.ax: AI agents as CRDT peers (Apr 2026) — https://electric.ax/blog/2026/04/08/ai-agents-as-crdt-peers-with-yjs
- Composio agent-orchestrator — https://github.com/ComposioHQ/agent-orchestrator
- Best multi-agent orchestrators 2026 (amux) — https://amux.io/blog/best-multi-agent-orchestrators-2026/
- Git worktrees for parallel AI agents (Augment Code) — https://www.augmentcode.com/guides/git-worktrees-parallel-ai-agent-execution
- Multi-agent AI coding workflow with worktrees (appxlab) — https://blog.appxlab.io/2026/03/31/multi-agent-ai-coding-workflow-git-worktrees/
- Sibling deliverables: [docs/file_tracking_decision.md](file_tracking_decision.md) (R.1), [docs/perf_findings.md](perf_findings.md) (R.2)
