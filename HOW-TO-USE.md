# Lattice — Integration Guide (HOW TO USE)

This document is written for a developer or coding agent integrating **Lattice** into
another application (e.g. an IDE with multi-agent workflows). It covers installation,
the core concepts, the exact command sequences to implement, the HTTP/JSON APIs, and
the git publishing path.

Lattice is an **agent-first version control system**. Multiple agents edit
concurrently in isolated workspaces; Lattice merges their work *semantically*
(typed operations over the syntax tree, not text lines), refuses to land
semantically broken merges, stores true conflicts as first-class "divergences"
with both sides' intents, and gates every land behind verifiers. Every landed
change is continuously exported to an ordinary **git mirror**, so downstream
tooling (GitHub, CI) keeps working unchanged.

---

## 1. Installation

### Requirements
- Rust toolchain (the repo pins the version via `rust-toolchain.toml` — `rustup` picks it up automatically)
- `git` on PATH (used by the mirror and history import)
- Windows, macOS, or Linux

### Build from source
```powershell
git clone https://github.com/avijitbhuin21/Lattice.git
cd Lattice
cargo build --release -p lat
```
The binary lands at `target/release/lat` (`lat.exe` on Windows).
Put it on PATH, or have your app invoke it by absolute path.

### Embedding options (pick one)
| Option | How | Best for |
|---|---|---|
| **CLI binary** | ship `lat`, shell out to it | any host language (Electron/TS, Python, …) |
| **HTTP API** | run `lat serve --port 7420`, call JSON endpoints | IDE UI panels |
| **Rust library** | `lattice-core = { git = "https://github.com/avijitbhuin21/Lattice.git" }` | Rust hosts; full programmatic API |

### Identity
Every command records provenance. Set these per agent (defaults exist, but set them):
```powershell
$env:LAT_AUTHOR  = "agent-frontend-1"   # who
$env:LAT_SESSION = "task-1234"          # session/task grouping
```

---

## 2. Core concepts (60 seconds)

- **Change** — the commit equivalent. Carries a structured *intent* (goal,
  constraints, machine-checkable claims), typed operations, verification
  evidence, and provenance. Landed via the merge stack + gate.
- **Workspace** — an O(1) copy-on-write working tree per agent
  (`.lat/workspaces/<name>/tree`). Creation copies nothing; files materialize on demand.
- **Land** — integrate work into HEAD. Runs the three-layer merge
  (tree commutation → structural/CST merge with typed-op composition → verification gate).
- **Divergence** — a true conflict, stored as a first-class object with both
  sides' intents. Not a broken working tree. Resolved once, remembered forever
  (identical conflicts replay automatically).
- **Gate** — registered verifiers (build/test/lint) run against the *post-merge*
  state. Red = nothing lands.
- **Mirror** — a real git repo at `.lat/mirror`, auto-exported on every land.
  One change = one commit. One-way: Lattice is the source of truth.

What merges cleanly that would conflict or silently break in git:
- rename of a function vs. edit of the same function's body
- moving a declaration to another file vs. editing its body in place
- adjacent/same-file declaration edits
- a rename on one side automatically rewrites stale callers introduced on the other side
- git's silent semantic breaks (deleted-but-still-referenced symbols) are caught, not landed

---

## 3. Repository lifecycle

```powershell
cd your-project
lat init                     # creates .lat/, lands the initial snapshot
lat status                   # working tree vs HEAD, with inferred typed ops
lat land -m "goal here"      # land the primary working dir as a change
lat log                      # history (use --full for prose + diff)
```

Register verifiers (the gate) once per project:
```powershell
# .lat/config.json — verifiers run against the post-merge state; inputs are
# globs used for evidence caching (unchanged inputs = cache hit, no re-run).
{
  "verifiers": [
    { "name": "build", "cmd": "cargo check", "inputs": ["src/**"] },
    { "name": "tests", "cmd": "cargo test --quiet", "inputs": ["src/**", "tests/**"] }
  ]
}
```

---

## 4. The multi-agent workflow (implement this loop)

This is the canonical loop your IDE should drive for each agent:

```powershell
# 1. one workspace per agent — O(1), no copying
lat ws new agent-a
lat ws open agent-a                  # materializes files into .lat/workspaces/agent-a/tree

# 2. the agent edits files inside its tree dir (normal file I/O)
#    .lat/workspaces/agent-a/tree/src/...

# 3. inspect what the agent changed
lat ws status agent-a

# 4. land through the merge stack + gate
lat ws land agent-a -m "Add retry logic to the uploader"
```

`ws land` outcomes (parse stdout):
| Output contains | Meaning | Next step |
|---|---|---|
| `landed <id>` | merged + gate green. Mirror auto-exported. **Primary working dir auto-syncs** to the new HEAD (locally-edited files are skipped and reported as `workdir: skipped <path>`). | nothing |
| `DIVERGED: stored as divergence <id>` | true conflict, stored first-class | run the resolve flow (§5) |
| `gate red — nothing landed` | a verifier failed on the post-merge state | fix, land again |

Run any number of agents concurrently — each in its own workspace. Lands are
serialized by the store; concurrent edits compose semantically wherever the
typed-op algebra proves they commute.

Cleanup: `lat ws rm agent-a` or `lat ws gc` for idle workspaces.

---

## 5. Divergence resolution flow

When a land diverges (this is the "two agents edited the same context" case):

```powershell
lat div list                          # all divergences + status
lat div show <id>                     # both sides' goals, conflicted paths

lat resolve <id>                      # materializes conflicts into .lat/resolve/<id>/
#   -> each conflicted file contains diff3 markers (<<<<<<< / >>>>>>>)
#   -> INTENTS.txt states BOTH sides' goals — give this to the resolving agent

# the agent edits the files to their merged form (all markers removed), then:
lat resolve <id> --done -m "resolve: combine retry logic with new size limits"
```

The resolution lands as a normal change (through the gate), the divergence is
marked `resolved`, the primary working dir syncs, and every individual conflict
resolution is remembered — the same conflict never has to be resolved twice.

---

## 6. Publishing to git

Every land already exports to the mirror. To publish:

```powershell
# first time — pass the remote URL (stored as `origin` in the mirror):
lat mirror --push https://github.com/you/your-project.git

# after that:
lat mirror --push            # defaults to origin

# CI fidelity check — mirror checkout is byte-identical to Lattice HEAD:
lat mirror --verify
```

Rules:
- **One-way.** Never commit directly into `.lat/mirror`. Lattice is the source of truth.
- Don't `git init` the working directory itself — the mirror **is** your git repo.
- To seed Lattice from an existing git project: `lat import <path-to-git-repo>` (one-time).

---

## 7. HTTP API (for IDE panels)

```powershell
lat serve --port 7420        # web UI + JSON API on 127.0.0.1
```

| Endpoint | Returns |
|---|---|
| `GET /api/changes` | history list (goal, author, evidence badges, retracts/resolves links) |
| `GET /api/change/:id` | full change: intent, provenance, evidence, typed ops, prose + unified diff |
| `GET /api/proposals` · `GET /api/proposal/:id` | PR-like pages; detail includes `ops` (typed-op summary) and `files` (per-file base/new text for side-by-side rendering) |
| `POST /api/proposal/:id/land` `{ "override_review": bool }` | land through gate + policy |
| `POST /api/proposal/:id/reject` `{ "reason": "..." }` | reject |
| `GET /api/divergences` · `GET /api/divergence/:id` | conflicts with both intents + marked files |
| `POST /api/divergence/:id/resolve` `{ "goal": "...", "files": { "path": "resolved content" } }` | land a resolution |
| `GET /api/search?q=...&k=15` | semantic code search over the store's own index |
| `GET /api/knowledge` | durable project facts (see §8) |
| `GET /api/trust` | per-author track records |
| `GET /api/advisory` | metric trends / worsening changes |

Id parameters accept unique prefixes. The bundled web UI at `/` renders all of
this (changes, proposals with side-by-side diffs, divergence resolution forms).

There is also a local daemon (`lat daemon start`) exposing a line-delimited
JSON protocol over TCP for event subscriptions and agent presence — useful for
live IDE updates (`lat events` reads the same stream one-shot).

---

## 8. Useful extras for agents

```powershell
lat outline src/lib.rs               # declaration signatures only (cheap context)
lat def <symbol>   /  lat refs <symbol>
lat why <symbol>                     # which changes touched this symbol and why
lat search "where is retry logic" -k 10
lat know set uploader-limit "Max upload is 50MB, enforced in uploader.rs" --tag infra
lat know list --find upload
lat run -- cargo fmt                 # capture a command's file delta as one atomic change
lat retract <change-id> -m "broke prod"   # first-class undo: lands the inverse
lat query --author agent-a --limit 20
```

Proposals (optional review flow): `lat propose <workspace> -m "goal" --claim "verifier:tests"`
creates a reviewable proposal instead of landing directly; green claims can
auto-land (`lat proposal` subcommands, or the HTTP API above).

---

## 9. Known limitations (current build)

- `lat sync` works between **local** stores only; no network transport yet. Single-machine use is fully supported — publish via the git mirror.
- Capability tokens are not cryptographically signed yet — single-user/trusted-machine assumption.
- Verifiers run un-sandboxed (trusted-repo assumption).
- File-rename vs. content-edit diverges conservatively (safe, but manual) — git's rename detection is better at this one case.
- Not yet published to crates.io; consume via git dependency or the built binary.

## 10. Verifying your integration

```powershell
cargo test --workspace                                             # full suite (94 tests)
cargo run -p agent-bench --bin lat-bench -- run crates/agent-bench/scenarios
```
The benchmark replays scripted multi-agent scenarios through both Lattice and
git and prints auto-integration / safety scores side by side. A useful smoke
test for your integration is scenario-08-style: two agents editing the same
function body → one lands, one diverges → resolve via §5 → both intents in HEAD.
