# AI Agent Comparison Report
> Research for Rustic's Agent Implementation
> Date: 2026-03-30

---

## Tools Compared

1. **Claude Code** (Anthropic) — TypeScript CLI
2. **OpenAI Codex CLI** (OpenAI) — Rust CLI (~94.7% Rust)
3. **Gemini CLI** (Google) — TypeScript monorepo
4. **Roo Code** (Community) — TypeScript VS Code extension
5. **Kilo Code** (Kilo-Org) — Roo Code fork with minor additions

---

## 1. Tool Sets (What the LLM Can Call)

### Claude Code

| Tool | Category | Notes |
|---|---|---|
| `Read` | File | Line-number format, 2000-line default limit, supports images/PDFs/notebooks |
| `Write` | File | Full overwrite, requires prior Read on existing files |
| `Edit` | File | Exact `old_string → new_string` replacement. Sends diff only, not full file |
| `Glob` | Search | Pattern matching, results sorted by modification time |
| `Grep` | Search | Ripgrep-backed. Supports regex, context lines, file type filters |
| `Bash` | Shell | Persistent shell session across calls. Working dir persists, env vars do NOT |
| `WebFetch` | Web | URL → markdown conversion. 15-min cache |
| `WebSearch` | Web | External search |
| `Agent` | Orchestration | Spawn subagent with own isolated context window |
| `LSP` | Code Intel | Type errors, jump-to-def, find-refs (requires plugin) |
| `NotebookEdit` | Notebook | JSON-aware Jupyter cell editing |

**Key constraints:** Bash explicitly forbidden for file operations — must use Read/Grep/Glob. This keeps all file ops auditable through the permission system.

### OpenAI Codex CLI

| Tool | Category | Notes |
|---|---|---|
| `shell_command` | Shell | String command via bash/zsh/PowerShell |
| `shell_tool` (LocalShellCall) | Shell | Command as array, direct execvp/CreateProcessW |
| `exec_command_tool` | Shell | PTY-based, interactive, supports `write_stdin_tool` follow-ups |
| `apply_patch` | File | **V4A diff format** — structured patch, not full file rewrites |
| `list_dir` | File | Recursive listing, 2-level depth default, offset/limit pagination |
| `view_image_tool` | Multimodal | Image display |
| `js_repl_tool` | Execution | V8 JS REPL, stateful across calls |
| `update_plan` | Tracking | Structured task list, one step in_progress at a time |
| Web search | Web | Built-in, cached OpenAI index by default; live fetch with `--search` flag |
| Multi-agent tools | Orchestration | `create_spawn_agent`, `send_message`, `assign_task`, `list_agents`, etc. |
| `tool_search_tool` | Discovery | **BM25 search** across all registered tools — avoids loading all schemas |
| MCP tools | MCP | `read_mcp_resource`, `list_mcp_resources` |

**No dedicated read_file or grep tool** — file ops are done through shell commands or `apply_patch`. The apply_patch approach is significantly more token-efficient than sending full file content back and forth.

### Gemini CLI

| Tool | Category | Notes |
|---|---|---|
| `read_file` | File | Line range support (`start_line`, `end_line`) |
| `write_file` | File | Full overwrite with diff preview, preserves line endings (CRLF/LF) |
| `edit` | File | **4-strategy cascade**: exact → flexible whitespace → regex → fuzzy (Levenshtein). LLM self-correction on failure |
| `read_many_files` | File | Parallel glob-based multi-file read |
| `list_directory` | File | Standard listing with gitignore respect |
| `glob` | Search | 20,000 file limit, sorted by mtime |
| `grep` / `SearchText` | Search | 3-tier fallback: git grep → system grep → pure JS |
| `ripGrep` | Search | Separate `rg` binary invocation with `--json` output |
| `run_shell_command` | Shell | Platform dispatch: Windows→PowerShell, else→bash |
| `web_fetch` | Web | Two modes: standard (Gemini backend + grounding) or direct |
| `web_search` | Web | **Native Google Search Grounding** — not external API |
| `save_memory` | Memory | Appends facts to `~/.gemini/GEMINI.md` |
| `write_todos` | Tracking | Task tracking |
| `enter_plan_mode` / `exit_plan_mode` | Control | Read-only enforcement |
| `activate_skill` | Skills | Loads skill bundle with instructions + resources |

**Standout:** The 4-strategy edit with LLM self-correction is the most robust file editing approach of any tool studied.

### Roo Code

| Tool | Category | Notes |
|---|---|---|
| `read_file` | File | Line range support, file size limits enforced |
| `write_to_file` | File | Full file replacement |
| `apply_diff` | File | Unified diff format — token-efficient for small changes |
| `insert_content` | File | Line-number-based insertion without full rewrite |
| `search_and_replace` | File | Pattern-based replacement |
| `list_files` | File | Recursive listing with .gitignore respect |
| `list_code_definition_names` | Code | Tree-sitter-based symbol extraction |
| `search_files` | Search | Regex search across files |
| `execute_command` | Shell | Shell command execution |
| `browser_action` | Browser | Puppeteer browser control (navigate, click, type, screenshot) |
| `ask_followup_question` | Interaction | Pause and ask the user |
| `attempt_completion` | Control | Signal task complete |
| `use_mcp_tool` | MCP | Direct MCP tool invocation |
| `access_mcp_resource` | MCP | Direct MCP resource access |
| `web_search` | Web | Via search-plus MCP integration |

**Unique:** `list_code_definition_names` for tree-sitter symbol extraction — gives the LLM a structural view of a file without reading the whole thing.

---

## 2. MCP Server Integration

| Feature | Claude Code | Codex | Gemini CLI | Roo Code |
|---|---|---|---|---|
| Transports | stdio, SSE, HTTP, WebSocket | stdio, HTTP streaming | stdio, SSE, HTTP streaming | stdio, SSE |
| Auth | OAuth (RFC 9728), tokens, env vars | Token/env vars | OAuth 2.0 with dynamic client reg, env vars | Token/env vars |
| Tool namespace | `mcp__plugin_<srv>__<tool>` | `mcp__<server>__<tool>` | `mcp_<server>_<tool>` | `use_mcp_tool` with server param |
| Deferred loading | Yes — ToolSearch fetches schemas on demand | Yes — BM25 tool_search_tool | No — all schemas loaded at startup | No — all schemas loaded |
| Output token limit | 25,000 (configurable `MAX_MCP_OUTPUT_TOKENS`) | Not specified | Not specified | Not specified |
| Schema sanitization | Yes | Yes | Yes (removes $schema, additionalProperties) | Yes |
| Act as MCP server | No | Yes — `codex mcp-server` subcommand | No | No |
| MCP prompts as commands | No | No | Yes — exposed as slash commands | No |
| MCP resources | Yes — `@server://resource` | Yes | Yes — `@server://resource` syntax | Yes |

**Key insight for Rustic:** Claude Code's deferred loading approach (load tool names only, fetch full schemas with ToolSearch when needed) is the most token-efficient for large MCP server libraries. Codex's BM25 search approach is equivalent but more semantic.

---

## 3. Skills / Workflows / Slash Commands

### Claude Code
- Skills live in `.claude/skills/<name>/SKILL.md` (project) or `~/.claude/skills/` (global)
- Each skill description capped at **250 chars** for context budget
- Total skill description budget: **1% of context window**
- Lazy: full skill content only loaded when invoked
- Custom subagents in `.claude/agents/<name>.md` with optional worktree isolation
- Built-in slash commands: `/clear`, `/compact`, `/context`, `/model`, `/cost`, `/mcp`

### OpenAI Codex
- Skills in `~/.codex/skills/` (global) or `.codex/skills/` (repo-scoped)
- Remote skill marketplace support
- `AGENTS.md` files provide coding conventions (deeper-nested = higher precedence)
- Hooks: `userpromptsubmit`, pre/post-turn callbacks, notification scripts
- OpenTelemetry export for observability

### Gemini CLI
- Skills activated via `activate_skill` tool — returns `<instructions>` + `<available_resources>` XML
- **Extensions system** via `gemini-extension.json` manifest — bundles MCP servers + skills + hooks + themes + custom commands
- Custom slash commands via **TOML files** in `~/.gemini/commands/` or `.gemini/commands/`
- TOML template syntax: `{{args}}`, `!{shell command}`, `@{filepath}` for embedding files
- Subdirectories create namespaced commands: `git/commit.toml` → `/git:commit`

### Roo Code
- `.roo/skills/` directory (or `.kilo/skills/` in Kilo Code)
- Skills activated via `@skill-name` mention or rule-based auto-activation
- **Modes system**: Code, Architect, Ask, Debug, Orchestrator — each mode has its own:
  - Allowed tool groups
  - File restriction regex (Architect can only read code files, not write them)
  - Custom system prompt section
  - Custom model configuration
- Mode config in `.roomodes` / `.kilocodemodes`

---

## 4. Token Consumption & Context Management (Critical for Rustic)

This is the most important dimension. Here's how each handles the context window filling up:

### Claude Code
- **Window:** 200K tokens (claude-sonnet-4-6 / claude-opus-4-6)
- **Compaction trigger:** ~98% of effective window
- **Process:**
  1. Strip images/PDFs/empty blocks
  2. Summarize conversation via separate API call (33K–45K token buffer reserved)
  3. Compression ratio: **60–80%** (150K → 30–50K tokens)
- **Optimizations:**
  - **Cache warm-up:** dummy `max_tokens=1` request before first real call to pre-fill server-side prompt cache on tool definitions
  - Deferred MCP tool schemas (only names in context)
  - Skill descriptions capped at 250 chars
  - Subagent results: only summary returned to parent (intermediate work never enters parent context)
  - CLAUDE.md: first 200 lines only

### OpenAI Codex
- **Window:** Model-dependent (gpt-5.x series)
- **Token counting:** Byte-based heuristic `ceil(bytes/4)` — fast but approximate
- **Per-section budgets:**
  - Current thread history: 1,200 tokens
  - Recent work: 2,200 tokens
  - Workspace structure: 1,600 tokens
  - Notes: 300 tokens
- **Truncation (no summarization in this path):**
  1. Output truncation first
  2. Image stripping
  3. History normalization (orphaned tool results removed)
  4. Turn-based removal, oldest first
- **Auto-compact** (separate from truncation): Uses `gpt-5.1-codex-mini` (Stage 1) + `gpt-5.3-codex` (Stage 2) — dual model summarization
- **Optimizations:**
  - WebSocket reuse across tool iterations (no reconnect per call)
  - BM25 tool search (don't load all schemas)
  - apply_patch format (diffs, not full files — huge token savings)
  - Session prewarm before model call
  - Image placeholders (base64 → byte estimate)
  - `EXEC_OUTPUT_MAX_BYTES` = 8 KiB shell output cap (1 MiB full-buffer mode)

### Gemini CLI
- **Window:** **1,048,576 tokens flat** for all models (gemini-2.5-pro, flash, flash-lite)
- **Compression trigger:** When `remainingTokenCount < estimatedRequestTokenCount`
- **Process:** Summarize history, reinitialize chat, re-send full IDE context next turn
- **Tool output truncation:** `truncateToolOutputThreshold` = **40,000 tokens** (much larger than Codex's 8 KiB!)
- **JIT context loading:** When high-intent tools access a path, auto-discover and inject the GEMINI.md for that directory — deferred until needed
- **Token caching:** Google context caching for system instructions (not available to OAuth users)
- **Optimizations:**
  - Large context window reduces compaction frequency
  - JIT GEMINI.md means project context loaded on demand, not upfront
  - Dual `llmContent`/`returnDisplay` fields — model sees compact version, user sees full version

### Roo Code (Most Sophisticated Context Management)
- **Window:** Per-provider (supports 200K for Claude, 1M+ for Gemini, etc.)
- **Three-layer strategy:**
  1. **Predictive condensation (80% threshold):** Before sending, calculates if message will fit. If not, starts condensing proactively. "Will I fit?" check BEFORE the API call, not reactively after.
  2. **LLM-based summarization:** Uses a separate summarization call when condensation needed. Summarizes older parts of conversation.
  3. **Sliding window fallback:** If summarization fails, drops oldest messages while keeping recent context.
- **Non-destructive tagging:** Messages are tagged (condensed/dropped) but never deleted from the internal store. If the context relaxes, previously condensed messages can be restored.
- **Context budget distribution:**
  - System prompt: fixed portion
  - Conversation history: dynamic, fills remaining budget
  - Tool result truncation: per-tool limits
- **Migration note:** Roo Code moved from XML-based tool parsing to native LLM tool calling — eliminated ~10% tool call failure rate from brittle parsing

---

## 5. Architecture Overview

| Dimension | Claude Code | Codex | Gemini CLI | Roo Code |
|---|---|---|---|---|
| Language | TypeScript | **Rust** (94.7%) | TypeScript | TypeScript |
| API format | Anthropic Messages API | OpenAI **Responses API** (not Chat Completions) | Gemini generateContent | Provider-agnostic |
| Streaming | Yes | Yes (WebSocket) | Yes | Yes |
| Dual model | Haiku (metadata) + Opus/Sonnet (main) | mini (compact) + full (main) | flash-lite/flash/pro cascade | Single model per session |
| Parallel tools | Yes (Scheduler groups independent calls) | Yes (tokio RwLock pattern) | Yes (Promise.allSettled) | Yes |
| Subagents | Yes — full isolated context | Yes — multi-agent tools | Limited | Orchestrator mode |
| Session storage | Per-directory | `~/.codex/sessions/` + history.jsonl | Shadow git repo | VS Code workspace state |
| Checkpointing | Yes (before edits) | Shadow git repo (`~/.gemini/tmp`) | Yes (before file writes) | No built-in |
| Sandbox | No | Bubblewrap (Linux) / Seatbelt (macOS) / Windows sandbox | macOS Seatbelt / Docker | No (VS Code handles it) |

---

## 6. Edit Strategy Comparison

One of the highest-impact decisions for token consumption:

| Strategy | Used By | Token Cost | Failure Risk |
|---|---|---|---|
| Full file write | Gemini `write_file`, Roo `write_to_file` | **High** — sends entire file content | Low (always succeeds) |
| Exact string replacement | Claude Code `Edit` | **Low** — sends only old+new strings | Medium — fails if exact match not found |
| Unified diff (`apply_diff`) | Roo Code, Gemini V2 (planned) | Low — standard diff format | Low |
| V4A patch format (`apply_patch`) | Codex | **Very Low** — structured patch | Low |
| 4-strategy cascade + LLM self-correction | Gemini `edit` | Low + retry overhead | **Very Low** — fallback to fuzzy match |
| Line-number insertion (`insert_content`) | Roo Code | Very Low | Low |

**Recommendation for Rustic:** The combination of `apply_diff` (for existing file modifications) + `write_to_file` (for new files) + `insert_content` (for targeted insertions) gives the best balance of token efficiency and reliability. The exact-match-only approach (Claude Code `Edit`) is risky without the 4-strategy fallback.

---

## 7. Fit Assessment for Rustic

**Rustic's requirements:** Least token consumption, pure performance, already has multi-provider + MCP + checkpoints.

### What to Take From Each

#### From Claude Code
- **Deferred MCP tool loading:** Only tool names in context; `ToolSearch`-style fetch when needed. Critical for large MCP libraries.
- **Skill description budget:** Cap skill descriptions at ~250 chars. Total skill budget = ~1% of context window.
- **Cache warm-up pattern:** Send a dummy 1-token request before the first real call to pre-populate the provider's prompt cache on tool definitions.
- **Subagent context isolation:** Subagent summaries only returned to parent — intermediate work never bloats parent context.
- **Parallel tool batching:** System prompt should explicitly instruct model to batch independent tool calls in a single turn.

#### From OpenAI Codex
- **apply_patch / apply_diff as primary edit tool:** Far more token-efficient than full file rewrites. Send structured patches, not entire file contents.
- **Per-section context budgets:** Give explicit token budgets to each context section (workspace tree, recent files, conversation history) — prevents any one section from dominating.
- **BM25 tool discovery:** For large tool sets, let the model search for the tool it needs rather than loading all schemas upfront.
- **Shell output caps:** Hard cap shell command output at 8–16 KiB by default. Let the model request more if needed.
- **Byte-based token estimation:** `ceil(bytes/4)` is fast enough and good enough. Don't need a full tokenizer for budget management.

#### From Gemini CLI
- **JIT project context:** Load RUSTIC.md / project context files on demand when the model first touches a directory — not all upfront.
- **Dual llmContent/returnDisplay:** Tool results can have a compact version for the LLM and a rich version for the UI. Reduces tokens while keeping UI informative.
- **4-strategy edit cascade:** If implementing exact-string edit, fall back to whitespace-flexible → regex → fuzzy rather than hard-failing. Huge reliability improvement.
- **Per-directory context files:** Auto-inject directory-level instructions when the agent works in a subdirectory.

#### From Roo Code
- **Predictive condensation (most important):** Check "will this message fit?" BEFORE making the API call, not after it fails. At 80% threshold, start condensing proactively.
- **Non-destructive message tagging:** Don't delete condensed messages from internal store. Tag them as condensed. If context budget relaxes, they can be restored.
- **Modes system:** Code / Architect / Ask modes with explicit tool group access control is a clean pattern that reduces tool noise per mode.
- **`list_code_definition_names`:** Tree-sitter symbol extraction gives the LLM a structural view without full file reads — Rustic already has tree-sitter!

---

## 8. Recommended Tool Set for Rustic

Based on the comparison, the minimal high-performance tool set for Rustic:

### Core File Tools
| Tool | Basis | Notes |
|---|---|---|
| `read_file` | Gemini/Roo | With line range support — avoid sending 5000-line files when the model only needs lines 100-200 |
| `write_file` | All | For new files only |
| `apply_diff` / `apply_patch` | Codex/Roo | Primary edit method — unified diff or V4A format |
| `insert_content` | Roo | Line-number insertion without full rewrite |
| `list_files` | All | With .gitignore respect, depth limit |
| `list_code_definitions` | Roo | Tree-sitter symbol index — Rustic already has this! |

### Search Tools
| Tool | Basis | Notes |
|---|---|---|
| `search_files` | Roo/Claude | Ripgrep-backed regex search |
| `glob_files` | Claude/Gemini | Pattern matching sorted by mtime |

### Execution
| Tool | Basis | Notes |
|---|---|---|
| `run_command` | All | Shell execution with output cap (8–16 KiB default) |

### MCP
| Tool | Basis | Notes |
|---|---|---|
| `use_mcp_tool` | Roo | With deferred schema loading (Claude Code approach) |
| `list_mcp_resources` | All | Resource discovery |

### Control
| Tool | Basis | Notes |
|---|---|---|
| `ask_user` | All | Pause for clarification |
| `task_complete` | All | Signal completion |

**Omit for now:** Browser/Puppeteer (computer use territory), JS REPL (too niche), voice (not relevant).

---

## 9. Recommended Context Management Strategy for Rustic

```
Context Budget (example for 200K window):
├── System prompt:          ~8,000 tokens (fixed)
├── Tool definitions:       ~3,000 tokens (built-in tools, fixed)
├── MCP tool names:         ~500 tokens (deferred schemas via search)
├── Active skill:           ~1,000 tokens (only when invoked)
├── Project context (RUSTIC.md): ~2,000 tokens (JIT per directory)
├── Workspace tree:         ~1,500 tokens (2-level, filtered)
├── Conversation history:   ~remaining (dynamic)
└── Buffer for next turn:   ~10,000 tokens (reserved, never filled)

Condensation triggers:
1. At 75-80%: predictive check — will next message fit?
2. If not: LLM-based summarization of oldest 30% of history
3. Fallback: sliding window — drop oldest turns
4. Messages tagged (not deleted) in internal store
```

---

## 10. Summary Comparison Table

| Dimension | Claude Code | Codex | Gemini CLI | Roo Code |
|---|---|---|---|---|
| Token efficiency (edit) | Good (exact replace) | **Best** (apply_patch) | Good (cascade) | Good (apply_diff) |
| Context management | Good (compact) | Good (budgets + pure removal) | Good (large window) | **Best** (predictive) |
| MCP integration | **Best** (deferred loading) | Very good (BM25 search) | Good | Good |
| Skills/workflows | Good | Good | **Best** (extensions system) | Good (modes) |
| Sandbox/security | Basic | **Best** (Bubblewrap/Seatbelt) | Good (Seatbelt/Docker) | None |
| Subagents | **Best** (isolated context) | Very good | Limited | Basic (Orchestrator mode) |
| Code intelligence | Good (LSP) | None | None | Good (tree-sitter) |
| Reliability (edits) | Medium (exact match) | High | **Best** (4-strategy) | High (multi-strategy) |
| Implementation language | TypeScript | **Rust** | TypeScript | TypeScript |
| Fit for Rustic | High | **Very High** | High | High |

---

## Final Recommendation

**No single tool is a perfect fit — take the best ideas from each:**

1. **Primary inspiration: Codex + Roo Code** — Codex's apply_patch efficiency and Roo Code's predictive context management are the two most impactful patterns for Rustic's "least token, pure performance" goal.

2. **Adopt Claude Code's deferred MCP loading** — Critical once Rustic users start connecting large MCP servers.

3. **Adopt Gemini's 4-strategy edit cascade** — Don't hard-fail on exact match misses. The LLM often generates slightly-off old_string values; fuzzy fallback prevents frustrating failures.

4. **Adopt Gemini's JIT context loading** — Rustic already has a project settings concept; load directory-level context on demand rather than upfront.

5. **Adopt Roo Code's dual-representation tool results** — Compact for LLM context, rich for the Rustic UI. This alone can cut tool result tokens by 30-50%.

6. **Build modes from day one** — Code / Architect / Ask / Debug modes with per-mode tool access control. Clean, simple, and prevents the LLM from reaching for browser/destructive tools when in Ask mode.

7. **Rustic advantage:** Already has tree-sitter (→ `list_code_definitions`), checkpoint system (already better than most), MCP support, and multi-provider. The agent loop itself is the missing piece.
