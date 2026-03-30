# Skills, Workflows & MCP Config — Implementation Research for Rustic
> Date: 2026-03-30

---

## 1. Skills

### What is skills.sh?

`skills.sh` is a **web directory and leaderboard** for AI agent skills — NOT a package registry. Skills live in public GitHub repos. The site pulls metadata from repos and ranks them by install count (90,500+ skills listed as of March 2026).

The real thing to adopt is the **Agent Skills open standard** (`agentskills.io`) — published by Anthropic in October 2025, open-sourced December 2025. Every major tool (Claude Code, Gemini CLI, Roo Code, Cursor, OpenCode) now reads this format. If Rustic adopts it, users can install any skill from the ecosystem.

The install CLI is `npx skills add owner/repo` — it auto-detects installed agents and symlinks skills to all of them. For Rustic we'd ship our own equivalent (`rustic skills add`).

---

### The SKILL.md Format (Open Standard)

A skill is a **directory named after the skill**, containing a `SKILL.md` file:

```
my-skill/
├── SKILL.md          ← required
├── scripts/          ← optional (bash/python scripts)
├── references/       ← optional (extra .md knowledge files)
│   └── api-docs.md
└── assets/           ← optional (templates, data files)
```

#### SKILL.md Structure

```markdown
---
name: code-review
description: Reviews code for bugs, quality and style. Use when asked to review code or before committing.
license: MIT
compatibility: Requires git
allowed-tools: Bash(git *) Read Glob
metadata:
  author: someone
  version: "1.0"
---

# Code Review

Instructions for the agent go here in plain markdown...

## Steps
1. Run `git diff HEAD` to see changes
2. Check for common issues...

See [references/checklist.md](references/checklist.md) for the full checklist.
```

#### Required Frontmatter Fields

| Field | Required | Notes |
|---|---|---|
| `name` | Yes | 1–64 chars. Lowercase, digits, hyphens only. Must match directory name. |
| `description` | Yes | 1–1024 chars. Include WHAT it does AND WHEN to use it (trigger keywords). |

#### Optional Frontmatter Fields (base spec)

| Field | Notes |
|---|---|
| `license` | License name or path |
| `compatibility` | Environment requirements (Python version, tools needed, etc.) |
| `allowed-tools` | Space-separated list of pre-approved tools |
| `metadata` | Arbitrary key-value map (`author`, `version`, etc.) |

#### Claude Code Extensions (worth adopting selectively)

| Field | What it does |
|---|---|
| `disable-model-invocation: true` | Only user can trigger via `/name`, not auto-activated by agent |
| `user-invocable: false` | Only agent can activate, hidden from user's slash command list |
| `context: fork` | Run in isolated subagent context |
| `paths: "src/**/*.ts"` | Auto-activate only when agent touches matching files |

---

### Progressive Loading (Critical for Token Efficiency)

The spec mandates three loading levels:

```
Session start    → load name + description ONLY for ALL skills (~100 tokens per skill)
Skill activated  → load full SKILL.md body (<5000 tokens recommended)
Body references  → load scripts/, references/, assets/ files on demand
```

Keep `SKILL.md` under 500 lines. Move detail to supporting files.

---

### File Locations for Rustic

```
Global:   ~/.rustic/skills/<name>/SKILL.md
Project:  <project>/.rustic/skills/<name>/SKILL.md
Canonical: <project>/.agents/skills/<name>/SKILL.md  ← cross-tool compat
```

Cross-tool compatibility: skills installed to `.agents/skills/` will also work for Claude Code, Roo Code, Gemini CLI etc. Users who already have skills installed elsewhere can symlink.

---

### Skills Installation Flow

**From GitHub (installable skills):**
```
rustic skills add vercel-labs/agent-skills
rustic skills add https://github.com/someone/my-skills --skill code-review
rustic skills add ./local-skills-dir
```

Implementation in Rust:
1. Fetch repo (via `git2` crate — already in Rustic for the Git module)
2. Walk directory tree looking for `SKILL.md` files (same discovery logic as `npx skills`)
3. Copy or symlink to `.rustic/skills/<name>/`
4. Write/update a `skills-lock.json` with the GitHub tree SHA for update tracking

**Manually created skills:**
User creates `.rustic/skills/my-skill/SKILL.md` directly — or Rustic provides a UI to create it with a name + description field. No CLI needed for manual creation.

---

### Skills Discovery Precedence (suggested for Rustic)

```
Project (.rustic/skills/) > Global (~/.rustic/skills/) > .agents/skills/ (canonical)
```

At session start: scan all three locations, deduplicate by name (project wins), build index of `{name, description}`. Load full body only when activated.

---

### Runtime Activation

Two modes:
1. **Auto-activation**: agent decides the skill is relevant based on description match → loads full SKILL.md body into context
2. **User invocation**: user types `/skill-name` or `@skill-name` in chat → always loads full body

For auto-activation: at each turn, check if any skill description semantically matches the current task. Simple approach: BM25 match between user message and skill descriptions. Only activate if score passes threshold.

---

## 2. Workflows

### What Workflows Are (vs Skills)

| | Skills | Workflows |
|---|---|---|
| Triggered by | Agent (auto) or user (manual) | User only (always manual) |
| Installation | From GitHub OR manually created | Manually created only |
| Purpose | Reusable capability bundles | Project-specific step-by-step processes |
| Scope | Global or per-project | Per-project only makes sense |
| Agent auto-activates? | Yes | No |

Workflows are simpler than skills — they're essentially **saved prompts with a title and structured instructions**. Think: "Deploy to staging", "Write a PR description", "Run the test suite and fix failures". The user picks one from a list and it runs as a prompt.

---

### Workflow Format (Proposed for Rustic)

Since workflows are user-created only and simpler, a single markdown file (no directory needed) works well:

```
<project>/.rustic/workflows/deploy-staging.md
<project>/.rustic/workflows/write-pr.md
```

File format:

```markdown
---
name: deploy-staging
description: Deploy the current branch to the staging environment
---

# Deploy to Staging

1. Run the test suite: `npm test`
2. Build the project: `npm run build`
3. Deploy to staging: `./scripts/deploy.sh staging`
4. Verify the deployment at https://staging.example.com
5. Post a message in #deployments Slack channel with the commit hash

If any step fails, stop and report the error. Do not proceed to the next step.
```

Frontmatter fields:
- `name` — becomes the `/workflow-name` command (lowercase, hyphens)
- `description` — shown in the workflow picker UI

The body is the full prompt sent to the agent when the workflow is triggered.

---

### Workflow Discovery and UI

Since workflows are user-triggered only:
- Show a `/workflows` command that lists all available workflows with their descriptions
- Or show them in a picker in the chat UI (dropdown or command palette)
- User selects → full workflow markdown body becomes the user message
- No auto-activation ever

Workflows don't need to be in the agent's context at all until triggered — zero token cost at session start.

---

## 3. MCP Configuration

### The De-Facto Standard

No official spec for config format exists (RFC #2219 was filed and closed). The closest thing to a standard is what Claude Code, Roo Code, Cursor, and Claude Desktop all share:

```json
{
  "mcpServers": {
    "server-name": {
      "command": "executable",
      "args": ["arg1", "arg2"],
      "env": { "KEY": "value" }
    }
  }
}
```

VS Code is the outlier (uses `"servers"` instead of `"mcpServers"`). Codex CLI uses TOML with `[mcp_servers.*]` sections.

---

### Config Format Comparison

| Tool | Format | Root key | Global path | Project path |
|---|---|---|---|---|
| Claude Code | JSON | `mcpServers` | `~/.claude.json` | `.mcp.json` |
| VS Code | JSON | `servers` | `Code/User/mcp.json` | `.vscode/mcp.json` |
| Gemini CLI | JSON | `mcpServers` | `~/.gemini/settings.json` | `.gemini/settings.json` |
| Roo Code | JSON | `mcpServers` | `globalStorage/.../cline_mcp_settings.json` | `.roo/mcp.json` |
| Codex CLI | **TOML** | `[mcp_servers.*]` | `~/.codex/config.toml` | `.codex/config.toml` |

---

### Recommended Format for Rustic

Since Rustic is Rust-native, **TOML** fits best — `serde` + `toml` crate is idiomatic, already likely in the project, no JSON quoting issues, supports comments, cleaner for humans to write.

But for interoperability (users copying configs from Claude Code docs), also support reading `.mcp.json` (the most common format in tutorials).

#### Global config: `~/.rustic/config.toml`

```toml
# ~/.rustic/config.toml

[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
enabled = true

[mcp_servers.github.env]
GITHUB_PERSONAL_ACCESS_TOKEN = "${GITHUB_TOKEN}"

[mcp_servers.postgres]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"]
enabled = true
trust = false                          # require approval per tool call
allowed_tools = ["query"]              # allowlist
disabled_tools = ["execute_ddl"]       # blocklist

[mcp_servers.remote-api]
url = "https://api.example.com/mcp"    # http transport (url field = HTTP)
headers = { Authorization = "Bearer ${MY_TOKEN}" }
enabled = true
timeout_ms = 30000
```

#### Project config: `<project>/.rustic/mcp.toml`

Same structure — project config is merged with global, project wins on conflict.

#### Fields to support

**All transports:**
| Field | Type | Default | Notes |
|---|---|---|---|
| `enabled` | bool | `true` | Disable without removing |
| `trust` | bool | `false` | Skip per-tool approval dialogs |
| `allowed_tools` | `[string]` | all | Allowlist of tool names |
| `disabled_tools` | `[string]` | none | Blocklist of tool names |
| `timeout_ms` | int | 60000 | Tool call timeout |
| `required` | bool | `false` | Fail session if server unavailable |

**Stdio transport (inferred when `command` is present):**
| Field | Type | Notes |
|---|---|---|
| `command` | string | Required |
| `args` | `[string]` | Optional |
| `env` | table | Key-value env vars, `${VAR}` expansion |
| `cwd` | string | Working directory |

**HTTP transport (inferred when `url` is present):**
| Field | Type | Notes |
|---|---|---|
| `url` | string | Required |
| `headers` | table | Static headers, `${VAR}` expansion |

**SSE transport (legacy — keep for compat but don't promote):**
| Field | Type | Notes |
|---|---|---|
| `sse_url` | string | Distinguishes from HTTP |

---

### Also Support `.mcp.json` (Interoperability)

For users who copy-paste configs from Claude Code tutorials. At startup, Rustic checks:
1. `~/.rustic/config.toml` (primary)
2. `<project>/.rustic/mcp.toml` (project override)
3. `<project>/.mcp.json` (compatibility — read-only, same format as Claude Code)

If `.mcp.json` exists alongside `.rustic/mcp.toml`, merge them (`.rustic/mcp.toml` wins on conflict).

---

### Trust / Permission Model

Three levels (simple is better):

```
trust = false (default)  → every tool call shows approval dialog
trust = true             → all tools from this server auto-approve
allowed_tools = [...]    → only these tools auto-approve, rest need approval
```

This matches the `alwaysAllow` pattern from Roo Code and the `trust` flag from Gemini CLI — no need to invent something new.

---

### MCP Server Loading Strategy (Two-Level for 100+ Servers)

As discussed — hierarchical loading:

```
Context at session start:
  → Server registry only: [{name, description, tool_count}]   (~2K tokens)

Agent calls: search_mcp_servers("github pull requests")
  → Returns: [{name: "github", matching_tools: 3}]

Agent calls: get_server_tools("github")
  → Returns: tool names + 1-line descriptions for that server

Agent calls: use_mcp_tool("github", "list_pull_requests", {...})
  → Loads full schema for that tool, executes, returns result
```

Dynamic switching based on total tool count:
- `< 20 tools total` → flat loading (all tool names + descriptions in context at start)
- `20–100 tools` → flat names + BM25 search tool
- `100+ tools` → two-level hierarchy (server names only at start)

This is determined at session start by counting total tools across all connected servers.

---

## 4. Summary: What to Build for Rustic

### Skills

| Feature | Implementation |
|---|---|
| Format | SKILL.md open standard (base spec only, optionally Claude Code extensions) |
| File location | `.rustic/skills/<name>/SKILL.md` (project) + `~/.rustic/skills/<name>/SKILL.md` (global) |
| Cross-tool compat | Also check `.agents/skills/` canonical path |
| Install from GitHub | `rustic skills add owner/repo` — uses `git2` (already in Rustic) |
| Manual creation | UI in Rustic to create a skill with name + description + body |
| Runtime loading | Name+description at startup, full body on activation |
| Auto-activation | BM25 match between user message and skill descriptions |
| User invocation | `/skill-name` or `@skill-name` in chat |

### Workflows

| Feature | Implementation |
|---|---|
| Format | Simple markdown: frontmatter (`name`, `description`) + body |
| File location | `.rustic/workflows/<name>.md` (project-only) |
| Creation | Manual only — UI or direct file creation |
| Trigger | User-only via `/workflow-name` or picker UI |
| Context cost | Zero tokens until triggered |

### MCP Config

| Feature | Implementation |
|---|---|
| Primary format | TOML (`~/.rustic/config.toml` + `<project>/.rustic/mcp.toml`) |
| Compat format | Also read `.mcp.json` (Claude Code format) |
| Transports | stdio (command + args + env) + HTTP (url + headers) + SSE (legacy) |
| Trust model | `trust` flag (server-wide) + `allowed_tools` (per-tool) |
| Tool loading | Dynamic: flat / flat+BM25 / two-level hierarchy based on tool count |

### Key Rust Crates

| Purpose | Crate |
|---|---|
| TOML config parsing | `toml` + `serde` |
| JSON compat (`.mcp.json`) | `serde_json` |
| Frontmatter parsing (YAML in SKILL.md) | `gray_matter` or `serde_yaml` |
| BM25 tool search | `bm25` or build from scratch (~100 lines) |
| GitHub install (`git2` already present) | `git2` |
| HTTP MCP transport | `reqwest` (likely already present) |
| stdio MCP transport | `tokio::process::Command` |
