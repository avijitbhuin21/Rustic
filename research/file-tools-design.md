# File Tools Design — Large File Safety
> Date: 2026-03-30
> Addresses: no full overwrites, no full-file dumps in context, large file navigation

---

## The Two Problems

### Problem 1: `write_file` on large files
A 25,000-line file is ~700KB. Sending it entirely to the model to rewrite:
- Fills most of a 131K context window
- Is completely unnecessary — the model only wants to change 5 lines
- Risks losing Agent A's changes when Agent B overwrites based on stale content
- Is simply the wrong tool for the job

### Problem 2: Dumping full file content on conflict errors
When edit fails (stale old_string), returning the entire 25,000-line file in the error:
- Would blow the context window entirely
- The model only needs to see the ~200 lines around the area it was trying to change
- Most of the file is irrelevant noise

---

## Decision: Remove `write_file` for Existing Files

`write_file` → `create_file` + all the surgical edit tools below.

| Old tool | Replacement | When |
|---|---|---|
| `write_file` (existing) | REMOVED | Never — too dangerous on large files |
| `write_file` (new/empty) | `create_file` | Creating new files or overwriting empty files |
| All edits to existing files | Surgical tools below | Always |

`create_file` rejects the call if the file already has content:
```
Error: File already exists with content (1,847 lines).
Use edit_file, replace_lines, or insert_lines to modify it.
```

---

## Complete Surgical Tool Set for Existing Files

### Tool 1: `edit_file` — context-anchored string replacement

The workhorse. Model provides a unique string to find and what to replace it with. Never needs to see the whole file.

```json
{
  "name": "edit_file",
  "parameters": {
    "path": "src/auth.rs",
    "old_string": "fn login(user: &str) -> Token {",
    "new_string": "fn login(user: &User) -> Result<Token> {",
    "context_lines": 5
  }
}
```

Internally:
1. Acquire write lock
2. Re-read file fresh
3. Search for `old_string`
4. If found → replace → write → return success
5. If NOT found → return **only the ±`context_lines` (default 150) lines around the closest fuzzy match**, not the whole file

Fuzzy match on conflict: use a sliding window similarity to find where `old_string` would have been. Return that section so the model can see what changed there.

### Tool 2: `replace_lines` — line-number range replacement

For when the model knows exactly which lines to replace (from a prior `read_file` or `list_symbols`).

```json
{
  "name": "replace_lines",
  "parameters": {
    "path": "src/auth.rs",
    "start_line": 45,
    "end_line": 52,
    "new_content": "fn login(user: &User) -> Result<Token> {\n    // new body\n}"
  }
}
```

Internally:
1. Acquire write lock
2. Re-read file fresh
3. Extract current lines 45–52 from fresh content
4. If they differ significantly from what model expected → return ONLY those lines ±50 lines of context, not the whole file
5. Otherwise → replace and write

**No verification of expected content needed** — the line numbers are the anchor, not string matching. Simpler and works even when adjacent code changed.

### Tool 3: `insert_lines` — line-number based insertion

For adding new code without replacing existing code.

```json
{
  "name": "insert_lines",
  "parameters": {
    "path": "src/auth.rs",
    "after_line": 52,
    "content": "\npub fn logout(token: Token) {\n    // implementation\n}"
  }
}
```

Internally:
1. Acquire write lock
2. Re-read fresh
3. Split at line 52 boundary
4. Insert content between line 52 and 53
5. Write

Almost never fails — inserting after a line number doesn't depend on content matching. Only edge case: file has fewer lines than `after_line` → return file line count.

### Tool 4: `delete_lines` — remove a range of lines

```json
{
  "name": "delete_lines",
  "parameters": {
    "path": "src/auth.rs",
    "start_line": 78,
    "end_line": 82
  }
}
```

Internally: same pattern — lock, re-read, remove lines, write.

### Tool 5: `apply_patch` — multiple hunks in one atomic operation

For when the model needs to make several changes to one file atomically. Grouped as a single lock operation — no other agent can interleave.

```json
{
  "name": "apply_patch",
  "parameters": {
    "path": "src/auth.rs",
    "hunks": [
      {
        "old_string": "use std::str;",
        "new_string": "use std::str;\nuse crate::types::User;"
      },
      {
        "old_string": "fn login(user: &str)",
        "new_string": "fn login(user: &User)"
      },
      {
        "start_line": 120,
        "end_line": 125,
        "new_content": "    let token = Token::new(user.id);"
      }
    ]
  }
}
```

Hunks can be either old_string/new_string style OR line-number style — mixed is fine. Applied in order. If any hunk fails → roll back ALL (write original content back) → return which hunk failed and why.

---

## Complete Tool Set for Reading Large Files

### Tool: `read_file` — with mandatory line range for large files

```json
{
  "name": "read_file",
  "parameters": {
    "path": "src/auth.rs",
    "start_line": 40,
    "end_line": 80
  }
}
```

Behavior:
- `start_line` and `end_line` are optional
- If omitted AND file is under threshold (e.g., 300 lines) → return whole file
- If omitted AND file is OVER threshold → return first 100 lines + warning:
  ```
  [File: src/auth.rs — 18,432 lines total. Showing lines 1-100.]
  [Use list_symbols to navigate, or read_file with start_line/end_line for a specific section.]
  ...first 100 lines...
  ```
- Model is never silently given a truncated file — it always knows the file is large

### Tool: `list_symbols` — tree-sitter structural map

Returns function/class/struct names with line numbers. Rustic already has tree-sitter — this is nearly free to implement.

```json
{ "name": "list_symbols", "parameters": { "path": "src/auth.rs" } }
```

Response:
```
src/auth.rs (18,432 lines)

Structs:
  AuthConfig         line 12
  TokenStore         line 34
  Session            line 67

Enums:
  AuthError          line 89

Functions:
  login              line 102
  logout             line 134
  refresh_token      line 158
  validate_session   line 201
  ...
```

**This is the primary navigation tool for large files.** The model calls this first to understand the file structure, then uses `read_file(start, end)` to zoom into specific functions.

### Tool: `search_in_file` — grep within a single file

```json
{
  "name": "search_in_file",
  "parameters": {
    "path": "src/auth.rs",
    "pattern": "fn login",
    "context_lines": 10
  }
}
```

Response (bounded output):
```
src/auth.rs:102:  fn login(user: &str) -> Token {
src/auth.rs:103:      let token = Token::generate();
...10 lines of context...

src/auth.rs:445:  fn login_with_oauth(provider: &str) -> Result<Token> {
...10 lines of context...
```

Model uses this to locate the exact line number of what it wants, then uses `read_file(start, end)` for a precise window.

### Tool: `get_file_info` — metadata without content

```json
{ "name": "get_file_info", "parameters": { "path": "src/auth.rs" } }
```

Response:
```
path: src/auth.rs
lines: 18,432
size: 524 KB
last_modified: 2026-03-30 14:22
language: Rust
symbols: 47 functions, 8 structs, 3 enums, 2 traits
```

Costs essentially zero tokens. Model uses this first to decide its navigation strategy.

---

## Bounded Conflict Error Responses

When `edit_file` fails because old_string is stale, the response is bounded:

### Finding where the edit would have been

Use the leading/trailing lines of `old_string` to locate the approximate area:
1. Take first non-empty line of `old_string` as a search key
2. Fuzzy-search for the closest match in the fresh file content
3. Return ±150 lines around that location

```
Edit failed: 'fn login(user: &str)' not found in src/auth.rs

This section was likely modified by another agent. Showing the relevant area (lines 95-115):

 95:  use crate::types::{User, Token};
 96:
 97:  impl AuthService {
 98:
 99:      /// Authenticates a user and returns a session token.
100:      /// Updated to accept User struct instead of raw string.  ← Agent A's change
101:      pub fn login(user: &User) -> Result<Token> {             ← old_string was here
102:          let session = Session::new(user.id);
103:          Token::from_session(session)
104:      }
105:
...

Retry your edit using the current content above.
```

The model sees exactly what it needs — the current state of that specific section — without being flooded with 25,000 lines.

### Max error response size

Cap all error responses at **300 lines or 8KB, whichever is smaller**. The surrounding context window is too valuable to waste on large error payloads.

---

## Recommended Navigation Pattern for Large Files

**System prompt instructions for the model:**

```
When working with files, use this workflow:

1. ORIENT:     get_file_info(path)      → check size, symbol count
2. MAP:        list_symbols(path)       → see all functions/classes with line numbers
3. LOCATE:     search_in_file(path, pattern) → find exact lines of interest
4. READ:       read_file(path, start, end)   → read only the relevant section (200-300 lines max)
5. EDIT:       edit_file / replace_lines / insert_lines / delete_lines

Never read an entire file larger than 500 lines in one call.
Never write an entire file to make a small change — use the surgical edit tools.
If you need to understand the structure of a large file, use list_symbols first.
```

---

## Example: Working on a 25,000-Line File

**Task:** "Change the `login` function in `src/auth.rs` to accept `&User` instead of `&str`"

```
Step 1:  get_file_info("src/auth.rs")
         → 25,432 lines, Rust, 63 functions

Step 2:  list_symbols("src/auth.rs")
         → login: line 1,847
           logout: line 1,892
           ...

Step 3:  read_file("src/auth.rs", start=1840, end=1865)
         → reads only 25 lines around the function
         → sees: fn login(user: &str) -> Token {

Step 4:  edit_file("src/auth.rs",
           old="fn login(user: &str) -> Token {",
           new="fn login(user: &User) -> Result<Token> {")
         → Locks file, re-reads fresh, finds old_string at line 1,847, replaces, writes
         → Done. Touched 1 line of a 25,000-line file.
         → Context used: ~50 tokens for the edit, not 700KB of file content.
```

---

## Tool Summary

### File Reading (no side effects)

| Tool | When to use | Context cost |
|---|---|---|
| `get_file_info` | First step on any large file | ~5 tokens |
| `list_symbols` | Navigating large files, finding function locations | ~50-200 tokens |
| `search_in_file` | Finding specific patterns with context | ~50-500 tokens |
| `read_file(start, end)` | Reading specific section | proportional to range |

### File Writing (all atomic, all through lock)

| Tool | When to use | Conflict behavior |
|---|---|---|
| `create_file` | New files or empty files ONLY | Rejects if file has content |
| `edit_file` | Replacing a specific string | Returns ±150 lines around failure area |
| `replace_lines` | Replacing known line range | Returns ±50 lines around that range |
| `insert_lines` | Adding new content | Almost never fails |
| `delete_lines` | Removing line range | Almost never fails |
| `apply_patch` | Multiple changes atomically | Returns which hunk failed, rolls back |

### Removed

| Tool | Why removed |
|---|---|
| `write_file` (existing file) | Dangerous on large files, causes data loss in parallel scenarios |

---

## Implementation Notes for Rustic

- `list_symbols` is nearly free — Rustic already has tree-sitter integrated
- `read_file` with line ranges: Rustic can use `ropey` crate's line-indexed access (already present) — reading lines 1840-1865 from a 25,000-line rope is O(log n), not O(n)
- File lock granularity: one `tokio::sync::Mutex` per canonical file path, created on demand
- Conflict error responses: use the `old_string`'s first line as fuzzy search key to locate the relevant section in the fresh file content — never return more than 300 lines in an error
- `apply_patch` rollback: read original content before any hunks, keep in memory, write back on failure
