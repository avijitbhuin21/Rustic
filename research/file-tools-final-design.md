# File Tools — Final Design
> Terminal-first for reads, locked tools for writes
> Date: 2026-03-30

---

## The Core Principle

**Read/Navigate → Terminal (`run_command`)**
**Write/Mutate → Locked tools (small set)**

The model already knows grep, cat, sed, awk, PowerShell. No need to teach it new tools for things the shell does perfectly. This keeps the tool list short and context cost minimal.

---

## Final Tool Set (6 tools total)

| Tool | Purpose | Context cost |
|---|---|---|
| `run_command` | ALL reading, searching, navigation | Already in context |
| `create_file` | New or empty files only | 1 definition |
| `edit_file` | String-anchored replacement | 1 definition |
| `apply_patch` | Multiple changes in one atomic op | 1 definition |
| `insert_lines` | Line-number insertion (lock-safe) | 1 definition |
| `delete_lines` | Line-number deletion (lock-safe) | 1 definition |

That's it. No `get_file_info`, no `list_symbols`, no `search_in_file`, no `read_file` as a separate tool. The terminal handles all of that.

---

## Reading and Navigation via Terminal

### Line count / file info
```bash
# Unix/Mac
wc -l src/auth.rs

# PowerShell
(Get-Content src/auth.rs).Count
```

### Symbol/structure map (what list_symbols would have done)
```bash
# Unix — find all function definitions with line numbers
grep -n "^pub fn\|^fn\|^impl\|^struct\|^enum\|^trait" src/auth.rs

# PowerShell
Select-String -Path src/auth.rs -Pattern "^pub fn|^fn|^impl|^struct|^enum|^trait"
```

### Find a specific pattern with line numbers
```bash
# Unix
grep -n "fn login" src/auth.rs

# PowerShell
Select-String -Path src/auth.rs -Pattern "fn login"
```

### Read a specific line range WITH CORRECT LINE NUMBERS
This is critical — the model must always know the actual line numbers it's reading.

```bash
# Unix — reads lines 1840-1865, shows real line numbers (not 1-25)
awk 'NR>=1840 && NR<=1865 {print NR": "$0}' src/auth.rs

# Alternative — cat -n first (preserves real line numbers), then filter
cat -n src/auth.rs | sed -n '1840,1865p'

# PowerShell
$lines = Get-Content src/auth.rs
$start = 1839  # 0-indexed
$end = 1864
$lines[$start..$end] | ForEach-Object -Begin {$i=1840} -Process {"${i}: $_"; $i++}
```

Output the model sees:
```
1840: impl AuthService {
1841:
1842:     /// Authenticates a user
1843:     /// Returns a session token
1844:     pub fn login(user: &str) -> Token {
1845:         let session = Session::new();
1846:         Token::from_session(session)
1847:     }
1848:
```

**The model always knows it's at line 1844, not "line 5 of some buffer".**

### Read with context around a grep hit
```bash
# Show 10 lines before and after "fn login" with line numbers
grep -n "fn login" src/auth.rs
# → src/auth.rs:1844:    pub fn login(user: &str) -> Token {

# Now read ±15 lines around line 1844
awk 'NR>=1829 && NR<=1859 {print NR": "$0}' src/auth.rs
```

### Read with grep context flags (quick, less precise on line numbers)
```bash
grep -n -A 10 -B 5 "fn login" src/auth.rs
# -n = line numbers, -A = lines after, -B = lines before
```

---

## System Prompt Instructions for File Navigation

```
## File Navigation Rules

Always include line numbers when reading files:
- Use: awk 'NR>=X && NR<=Y {print NR": "$0}' file
- Use: grep -n pattern file
- Use: cat -n file | sed -n 'X,Yp'
- Never use plain: cat file (no line numbers)

Workflow for large files:
1. grep -n "^fn\|^pub fn\|^impl\|^struct" file  →  find relevant symbol
2. awk 'NR>=START && NR<=END {print NR": "$0}' file  →  read ±30 lines around it
3. edit_file / insert_lines / delete_lines  →  make the change

Never read more than 300 lines at once.
Never read a whole file larger than 300 lines.

When writing, always note the line number of what you're changing.
If parallel agents may have modified the file, re-grep for your target
before editing to confirm it's still at the expected line.
```

---

## Line Number Drift in Parallel Execution

This is the key problem the user raised: Agent B knows the `login` function is at line 1,844. Agent A inserts 50 lines above line 1,000. Now `login` is at line 1,894. Agent B edits line 1,844 — wrong place.

### Solution: String-anchored edits don't drift

`edit_file` uses `old_string` matching, not line numbers. The string `"pub fn login(user: &str) -> Token {"` is at line 1,894 after Agent A's insert — but `edit_file` finds it regardless of line number. No drift issue.

**This is why `edit_file` (string anchor) is always preferred over `replace_lines` (line anchor) for content changes.**

`replace_lines` and `insert_lines` are only for cases where the model is confident no other agent is touching nearby lines (e.g., the main model working alone, or sub-agents on clearly separate file sections).

### For `replace_lines` when drift might occur

Before calling `replace_lines(1844, 1847, ...)`, first verify:
```bash
awk 'NR>=1844 && NR<=1847 {print NR": "$0}' src/auth.rs
```
If the output doesn't match what you expected → re-grep for the function:
```bash
grep -n "fn login" src/auth.rs
```
Use the new line number. Then `replace_lines` with the corrected range.

### Conflict recovery workflow

```
1. Model knows target is around line 1,844 (from earlier grep)
2. Model calls edit_file(old="pub fn login(user: &str)", new="...")
3. edit_file FAILS — old_string not found (Agent A changed the signature)
4. Error response: "Not found. Searching ±150 lines around line 1,844..."
   Shows lines 1,694–1,994 with line numbers
5. Model sees current state, re-reads relevant section if needed
6. Model generates corrected edit_file call
7. Success
```

**The ±150 line window around the last known line number is the conflict recovery area — never dump the whole file.**

---

## Conflict Error Response Design (Bounded)

When `edit_file` fails, the tool:

1. Takes the first non-empty line of `old_string` as a search key
2. Fuzzy-searches the fresh file for the closest match
3. Returns ±150 lines around that location **with line numbers**
4. Hard cap: 300 lines maximum, 8KB maximum

```
edit_file failed: 'pub fn login(user: &str) -> Token {' not found.

The file may have been modified by another agent.
Showing lines 1,694–1,894 (±100 around estimated location):

1694: // ===== Auth Module =====
1695:
...
1844:     /// Signature changed by another agent:
1845:     pub fn login(user: &User) -> Result<Token> {   ← was &str, now &User
1846:         let session = Session::new(user.id);
1847:         Token::from_session(session)
1848:     }
...
1894:

Retry your edit using the current content above.
```

Model sees exactly what happened at exactly which line. Context cost: ~300 lines ≈ 1,500 tokens. Not 25,000 lines ≈ 125,000 tokens.

---

## Why `insert_lines` and `delete_lines` Stay as Locked Tools

Even though the model could do these via terminal (`sed -i`, `awk` rewrite), we keep them as locked tools because:

1. `sed -i 'X,Yd' file` on a 25,000-line file is not atomic with our lock system — another agent could write between the read and the write inside sed
2. `sed -i` behavior differs between GNU sed (Linux) and BSD sed (macOS) — portability issues
3. The lock ensures the entire read-modify-write cycle is atomic

The model uses terminal to **find** the line numbers, then uses the locked tool to **execute** the insert/delete.

```bash
# Step 1 — find line via terminal
grep -n "fn login" src/auth.rs
# → 1844: pub fn login...

# Step 2 — insert after line 1848 using locked tool
insert_lines("src/auth.rs", after_line=1848, content="    // TODO: add rate limiting")
```

---

## PowerShell Equivalents

For Windows users, the model uses PowerShell equivalents. Include both in system prompt:

| Operation | Unix | PowerShell |
|---|---|---|
| Line count | `wc -l file` | `(Get-Content file).Count` |
| Find symbol | `grep -n "^fn" file` | `Select-String -Path file -Pattern "^fn"` |
| Read range | `awk 'NR>=X&&NR<=Y{print NR": "$0}' file` | `(Get-Content file)[X-1..Y-1] \| %{..}` |
| Find + context | `grep -n -A10 -B5 "fn login" file` | `Select-String -Context 5,10 "fn login" file` |
| File info | `wc -l file && ls -lh file` | `(Get-Content file).Count; Get-Item file` |

The model detects the OS at session start (`run_command("uname -s || echo Windows")`) and uses appropriate syntax throughout. This detection is one-time at task creation, stored in the system context.

---

## Summary

**What the model does for a 25,000-line file:**

```
# 1. Find the function (~2 tokens of output)
grep -n "fn login" src/auth.rs
→ 1844: pub fn login(user: &str) -> Token {

# 2. Read just what's needed (~300 tokens of output)
awk 'NR>=1840 && NR<=1860 {print NR": "$0}' src/auth.rs

# 3. Make the change (~5 tokens tool call)
edit_file("src/auth.rs",
  old="pub fn login(user: &str) -> Token {",
  new="pub fn login(user: &User) -> Result<Token> {")
```

Total context used for the entire operation: ~500 tokens.
The 25,000-line file never enters the context window.

**Write tools in context at all times: 5 small definitions ≈ 200 tokens.**
No read tools needed — the terminal is the read tool.
