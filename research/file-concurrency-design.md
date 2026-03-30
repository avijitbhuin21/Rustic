# File Concurrency Design — Read-Modify-Write Race Conditions
> Date: 2026-03-30

---

## The Problem Illustrated

```
t=0   Agent A reads auth.rs  →  gets version 1: "fn login(user: &str) { ... }"
t=0   Agent B reads auth.rs  →  gets version 1: "fn login(user: &str) { ... }"
                                 (both reading simultaneously — fine so far)

t=5   Agent A writes auth.rs →  version 2: "fn login(user: &User) { ... }"
                                 (changed parameter type)

t=8   Agent B tries to write auth.rs
      Agent B's edit is based on version 1 it read at t=0

      Case 1 — Agent B uses apply_diff/edit_file:
        old_string: "fn login(user: &str)"  ← no longer in file! Agent A changed it.
        → DIFF FAILS (old_string not found)

      Case 2 — Agent B uses write_file (full overwrite):
        Writes entire file based on version 1 with its own changes
        → SILENTLY OVERWRITES Agent A's changes → DATA LOSS
```

**Case 1 is recoverable. Case 2 is silent data corruption.** This is why `apply_diff` is always preferred over full `write_file` for existing files — failures are loud, overwrites are silent.

---

## The Solution: Atomic Read-Modify-Write Lock

The fix: **when an agent wants to write, it re-reads the file at that moment, then writes — all under an exclusive lock.** Nothing can read or write the file between those two steps.

```
t=8   Agent B calls write/edit on auth.rs:
      ┌─ Acquire EXCLUSIVE write lock on auth.rs ─────────────────┐
      │  Re-read auth.rs FRESH  →  gets version 2 (Agent A's ver) │
      │  Apply Agent B's changes ON TOP of version 2              │
      │  Write the result                                          │
      └─ Release lock ────────────────────────────────────────────┘
```

Agent B is now working with the actual current state of the file, not a stale snapshot from t=0.

---

## How Each Tool Handles This

### `apply_diff` / `edit_file` (recommended for modifications)

```
Old behavior:
  1. Agent calls: edit_file(path, old_string="fn login(user: &str)", new_string="...")
  2. Tool searches file for old_string
  3. If not found → error

New behavior (atomic RMW):
  1. Agent calls: edit_file(path, old_string="...", new_string="...")
  2. Tool acquires exclusive write lock on this file
  3. Tool re-reads file FRESH (ignores what agent saw earlier)
  4. Searches fresh content for old_string
  5a. Found → apply replacement → write → release lock → return success
  5b. Not found → release lock → return error WITH fresh file content:
      "Edit failed: the section you wanted to change no longer matches.
       Current file content:
       ---
       fn login(user: &User) { ... }   ← Agent A changed &str to &User
       ---
       Please regenerate your edit based on the current content above."
  6. Agent sees fresh content in the error, regenerates edit, retries
```

**Why this is clean:** The failure is loud and informative. The model immediately gets the current state and can generate a correct edit. No data loss, no silent corruption.

### `write_file` (full overwrite — for new files only)

Full overwrites of existing files should be **rejected if the file was modified since the agent last read it**. Implement using a version hash:

```
When agent reads a file:
  → Return content + file_hash (SHA256 of content, first 8 chars)
  → Agent sees this as part of the read response

When agent calls write_file on existing file:
  → Require file_hash parameter (the hash it got when it read)
  → Re-read file, compute current hash
  → If hashes match: file unchanged, safe to overwrite → write it
  → If hashes differ: stale! Return error with fresh content:
    "File was modified by another agent since you read it.
     Current content: ..."

For NEW files (don't exist yet): no hash needed, just write.
```

This forces agents to always have a fresh read before a full overwrite. It makes the race visible instead of silently eating changes.

### `read_file` (reads — no change needed)

Concurrent reads are always fine. Multiple agents reading simultaneously is not a problem. No lock needed for pure reads.

The issue only arises when an agent **reads, then later writes**. The write step is where we enforce freshness.

---

## Implementation in Rust

### Updated `FileLockRegistry`

The registry now handles full atomic RMW, not just "lock then write separately":

```rust
// In task/file_lock.rs

pub struct FileLockRegistry {
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    // Note: Using Mutex (not RwLock) because we want exclusive access
    // during the entire read-THEN-write sequence, not just the write.
    // Multiple agents can still read without going through this registry —
    // only write operations use the lock.
}

impl FileLockRegistry {
    /// Execute an atomic read-modify-write on a file.
    /// The closure receives the CURRENT file content and returns the new content.
    /// Holds the exclusive lock for the entire duration of the closure.
    pub async fn atomic_rmw<F, E>(
        &self,
        path: &Path,
        rmw_fn: F,
    ) -> Result<(), E>
    where
        F: FnOnce(&str) -> Result<String, E>,  // current_content → new_content
        E: From<std::io::Error>,
    {
        let canonical = canonicalize(path)?;
        let lock = self.get_or_create_lock(&canonical).await;

        // Hold lock for entire duration
        let _guard = tokio::time::timeout(
            Duration::from_secs(30),
            lock.lock()
        ).await.map_err(|_| io_err("Write lock timeout: another agent is using this file"))?;

        // Re-read FRESH under the lock
        let current = std::fs::read_to_string(&canonical)?;

        // Apply the modification
        let new_content = rmw_fn(&current)?;  // caller applies edit/diff/overwrite

        // Write the result
        std::fs::write(&canonical, new_content)?;

        Ok(())
        // _guard dropped here → lock released
    }
}
```

### Updated `edit_file` tool

```rust
pub async fn edit_file(
    path: &str,
    old_string: &str,
    new_string: &str,
    context: &ToolContext,
) -> ToolOutput {
    let abs_path = context.project_root.join(path);

    let result = context.file_lock_registry.atomic_rmw(&abs_path, |current_content| {
        // Try to find old_string in the FRESH content (not what agent saw earlier)
        if let Some(pos) = current_content.find(old_string) {
            let new_content = format!(
                "{}{}{}",
                &current_content[..pos],
                new_string,
                &current_content[pos + old_string.len()..]
            );
            Ok(new_content)
        } else {
            // Not found — return fresh content in the error so model can retry
            Err(EditError::StaleRead {
                fresh_content: current_content.to_string(),
            })
        }
    }).await;

    match result {
        Ok(()) => ToolOutput::success("File updated successfully."),
        Err(EditError::StaleRead { fresh_content }) => ToolOutput::error(format!(
            "Edit failed: the section you wanted to change no longer exists in the file.\n\
             It may have been modified by another agent.\n\n\
             Current file content:\n```\n{}\n```\n\n\
             Please regenerate your edit based on the current content above.",
            fresh_content
        )),
        Err(EditError::Timeout) => ToolOutput::error(
            "File lock timeout: another agent is currently writing to this file. \
             Please retry in a moment."
        ),
        Err(e) => ToolOutput::error(format!("File write error: {}", e)),
    }
}
```

### `write_file` with stale-detection

```rust
pub async fn write_file(
    path: &str,
    content: &str,
    expected_hash: Option<&str>,  // None = new file, Some = existing file
    context: &ToolContext,
) -> ToolOutput {
    let abs_path = context.project_root.join(path);

    // New file — just write it
    if !abs_path.exists() {
        std::fs::write(&abs_path, content)?;
        return ToolOutput::success("File created.");
    }

    // Existing file — hash check to detect stale overwrites
    context.file_lock_registry.atomic_rmw(&abs_path, |current_content| {
        let current_hash = &sha256_short(current_content);

        match expected_hash {
            None => {
                // Agent didn't provide a hash — reject (they must have read it first)
                Err(WriteError::HashRequired { fresh_content: current_content.to_string() })
            }
            Some(expected) if expected != current_hash => {
                // Hash mismatch — file changed since agent read it
                Err(WriteError::StaleRead {
                    fresh_content: current_content.to_string(),
                    current_hash: current_hash.clone(),
                })
            }
            _ => Ok(content.to_string())  // hash matches, safe to overwrite
        }
    }).await
    // ... handle errors same as edit_file
}
```

### `read_file` — returns hash alongside content

```rust
pub async fn read_file(path: &str, context: &ToolContext) -> ToolOutput {
    let content = std::fs::read_to_string(&abs_path)?;
    let hash = sha256_short(&content);

    // Return content WITH hash so agent can use it for write_file later
    ToolOutput::success(format!(
        "[file_hash: {}]\n{}",
        hash, content
    ))
}
```

The model sees the hash in the read output. When it later calls `write_file`, it includes that hash. If another agent modified the file in between, the hashes won't match and it gets a clean error with fresh content.

---

## Concrete Example — Three Agents, Same File

**Scenario:** Three sub-agents all read `config.rs` at t=0, each makes different changes.

```
t=0   Agent A reads config.rs → [file_hash: abc123] version 1
t=0   Agent B reads config.rs → [file_hash: abc123] version 1  (concurrent read, fine)
t=0   Agent C reads config.rs → [file_hash: abc123] version 1  (concurrent read, fine)

t=4   Agent A calls edit_file(old="timeout: 30", new="timeout: 60")
      ┌─ Lock acquired ──────────────────────────────────────────┐
      │  Fresh read → still "timeout: 30" (nobody wrote yet)    │
      │  Apply edit → "timeout: 60"                             │
      │  Write → version 2, hash: def456                        │
      └─ Lock released ──────────────────────────────────────────┘
      → Success

t=6   Agent B calls edit_file(old="max_retries: 3", new="max_retries: 5")
      ┌─ Lock acquired ──────────────────────────────────────────┐
      │  Fresh read → version 2 (Agent A's version)             │
      │  Search for "max_retries: 3" → FOUND (A didn't touch it)│
      │  Apply edit → version 3, hash: ghi789                   │
      └─ Lock released ──────────────────────────────────────────┘
      → Success  ← Agent B's edit lands on top of Agent A's

t=7   Agent C calls edit_file(old="timeout: 30", new="timeout: 90")
      ┌─ Lock acquired ──────────────────────────────────────────┐
      │  Fresh read → version 3 (both A and B's changes)        │
      │  Search for "timeout: 30" → NOT FOUND (Agent A changed  │
      │  it to 60 at t=4)                                       │
      └─ Lock released ──────────────────────────────────────────┘
      → Error returned to Agent C:
        "Edit failed: 'timeout: 30' not found.
         Current file content:
         ...timeout: 60...   ← shows current state
         ...max_retries: 5..."

t=7   Agent C sees error, re-reads fresh content from error message,
      calls edit_file(old="timeout: 60", new="timeout: 90")
      → Success  ← version 4 has all three agents' changes applied cleanly
```

**Result:** All three changes land correctly. No data loss. The conflict was surfaced as an error with enough info to self-correct.

---

## What This Means for the Main Model

**When assigning tasks to sub-agents, the main model should:**
1. Prefer assigning agents to **non-overlapping files** (primary prevention)
2. If agents MUST touch the same file, tell them to use `edit_file` (never `write_file`) and **be specific about which lines/sections to change** so their edits don't overlap
3. If two agents need to make changes to the same section, make them **sequential** (spawn second only after first completes)

**System prompt addition:**
```
When multiple sub-agents might modify the same file:
- Assign agents to different sections where possible
- Always use edit_file (not write_file) for existing files — it self-corrects on conflicts
- If an edit fails due to stale content, re-read the error message which includes the
  current file state, then retry with the correct old_string
- For coordinated changes to the same section, spawn agents sequentially:
  wait for the first to complete before spawning the second
```

---

## Summary of the Full Concurrency Strategy

| Scenario | Behavior |
|---|---|
| Multiple agents reading same file | Allowed, no locks, no problem |
| Agent reads then another writes before first agent writes | Detected via hash / edit mismatch. Fresh content returned in error. Agent self-corrects. |
| Two agents try to write same file simultaneously | Serialized by Mutex. Second waits (max 30s). Gets fresh content to work with. |
| Agent tries to overwrite without reading first | Rejected — must provide file_hash from a prior read |
| Agent's edit section was already changed by another agent | Fails loudly with current content. Agent retries with correct old_string. |
| Two agents writing completely different sections | Both succeed. Lock serializes them but both changes land correctly. |
| Agent assigned files that overlap with another agent | Main model should prevent this. If it happens, the edit/hash system catches it. |
