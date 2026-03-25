# Rustic — Project Overview

## What is Rustic?

Rustic is a VS Code-inspired **agentic IDE** built with **Rust (Tauri 2)** on the backend and **vanilla JavaScript/CSS/HTML** on the frontend. It combines the familiar VS Code layout and workflow with two core differentiators:

1. **Multi-Project Workspaces** — Open and work on multiple projects simultaneously within a single window. Each project gets its own file explorer section, source control, search scope, terminal, and agent tasks.

2. **Built-in AI Agent** — An integrated AI agent system that can read/write files, run terminal commands, use MCP tools, and work on tasks in parallel — all with a checkpoint/rollback system that snapshots files before every AI edit.

---

## Tech Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| **Backend** | Rust + Tauri 2 | Performance, safety, cross-platform. All heavy lifting (file I/O, parsing, search, git, AI) runs here. |
| **Frontend** | Vanilla JS + CSS + HTML | Zero framework overhead, full design flexibility, direct DOM control. Vite as dev server only. |
| **Database** | SQLite (rusqlite, bundled) | Persistent storage for tasks, checkpoints, settings, project metadata. Battle-tested, fast, single-file. |
| **Editor Engine** | Ropey (rope data structure) + Tree-sitter | Efficient text buffer for large files + incremental syntax highlighting for 100+ languages. |
| **Terminal** | xterm.js + portable-pty | Terminal emulation in the webview (xterm.js) backed by real PTY sessions (portable-pty in Rust). |
| **Git** | git2 (libgit2 Rust bindings) | Per-project git operations — status, staging, committing, diffing, branching. |
| **AI Providers** | reqwest + keyring | HTTP clients for Claude, OpenAI, Gemini APIs + generic OpenAI-compatible. API keys stored in OS keychain. |
| **Search** | grep-regex + ignore crate | Ripgrep-based content search. Fast file walking with .gitignore respect. |
| **Build Tool** | Vite (minimal) | Serves frontend files for Tauri dev server. No transpilation, no bundling of framework code. |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        Tauri Window                             │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   Frontend (Webview)                       │  │
│  │  Vanilla JS + CSS + HTML                                  │  │
│  │  - Thin rendering layer                                   │  │
│  │  - Virtual scrolling (only renders visible items)         │  │
│  │  - ES modules, no framework                               │  │
│  │  - Communicates with backend via Tauri invoke() / events  │  │
│  └──────────────────────┬────────────────────────────────────┘  │
│                         │ IPC (JSON-RPC)                        │
│  ┌──────────────────────┴────────────────────────────────────┐  │
│  │                   Backend (Rust)                           │  │
│  │                                                           │  │
│  │  src-tauri/          ← Tauri app, commands, state, events │  │
│  │  crates/rustic-core/ ← Buffer, syntax, workspace, search │  │
│  │  crates/rustic-db/   ← SQLite layer, migrations, repos   │  │
│  │  crates/rustic-agent/← AI providers, tools, MCP, checkpts│  │
│  │  crates/rustic-git/  ← Git operations (git2)             │  │
│  │  crates/rustic-terminal/ ← PTY management                │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

**Key principle:** The frontend is a thin rendering layer. It receives data from the Rust backend and renders it. All file I/O, parsing, searching, git operations, AI calls, and database operations happen in Rust. The frontend never directly touches the filesystem.

---

## UI Layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ [Logo] File  Edit  View  Agent  Help          [≡] [⊟] [⊠]  ─ □ ✕  │
├────┬─────────────┬──────────────────────────────┬────────────────────┤
│    │ EXPLORER    │  [Demo1] main.rs  ×  │ ...  │  Agent Chat        │
│ 📁 │ + Add Proj  │─────────────────────────────│  (Secondary        │
│    │             │                              │   Sidebar —        │
│ 🔍 │ ▼ Demo1     │   1 │ fn main() {           │   opens on task    │
│    │   ▸ src/    │   2 │     println!("hi");    │   click)           │
│ 🌿 │   ▸ tests/ │   3 │ }                      │                    │
│    │   Cargo.toml│   4 │                        │  [Chat messages]   │
│ 🤖 │ ▼ Demo2     │                              │  [Tool calls]      │
│    │   ▸ src/    │                              │  [Checkpoints]     │
│    │   ▸ lib/    │                              │                    │
│    │             │                              │  [Input bar]       │
│────│─────────────│──────────────────────────────│────────────────────│
│ ⚙  │             │  Terminal  │ Agent Logs      │                    │
│ 👤 │             │  $ cargo build              │                    │
│    │             │  Compiling rustic v0.1.0     │                    │
└────┴─────────────┴──────────────────────────────┴────────────────────┘
```

### Layout Components
- **Top Bar** — Logo, menus (File/Edit/View/Agent/Help), sidebar/panel toggles, window controls (custom titlebar, no native decorations)
- **Activity Bar** (48px, left) — Explorer, Search, Source Control, Agent icons. Bottom: Settings, Accounts.
- **Primary Sidebar** (resizable, left) — Content changes based on active activity bar icon
- **Editor Area** (center) — Tabbed file editing with `[ProjectName] filename` prefix
- **Secondary Sidebar** (right) — Only visible when an agent task is clicked. Shows chat, tool calls, checkpoints.
- **Bottom Panel** (resizable) — Terminal tabs + Agent output logs

---

## Core Features

### Multi-Project Workspace
- Add/remove multiple project folders to the workspace
- Each project is a collapsible section in the Explorer
- Per-project: file tree, search scope, git tracking, terminals, agent tasks
- Global search across all projects simultaneously
- Project-prefixed tabs: `[Demo1] main.rs`, `[Demo2] main.rs`

### AI Agent System
- **Per-project tasks** — Each task is a conversation where the agent can read/write files in that project
- **Multi-provider** — Claude (Anthropic API), OpenAI, Gemini, + any OpenAI-compatible endpoint (OpenRouter, Grok, etc.)
- **Tool use** — File read/write, terminal commands, search, MCP tools
- **Parallel execution** — Multiple tasks across projects and within the same project
- **MCP support** — Add external MCP tool servers (stdio or SSE transport)
- **Permission system** — Global default + per-project override: Admin (bypass all), ReadWrite (read + write + commands with confirmation), ReadOnly (only read)
- **Checkpoint/rollback** — Before every AI file edit, a snapshot is stored in SQLite. User can revert to any chat message's checkpoint. Not in git history.
- **Task management** — Delete tasks with option to revert all changes or keep them

### Editor
- Rope-based text buffer (ropey) for efficient editing of large files
- Tree-sitter syntax highlighting (incremental parsing)
- Virtual scrolling — only renders visible lines, handles 500,000+ line files
- Undo/redo with time-based grouping
- LSP support (Phase 13) — autocomplete, diagnostics, hover, go-to-definition, auto-format on save

### Terminal
- Integrated terminal (xterm.js + portable-pty)
- Default terminal opens at Rustic's working directory
- Per-project "New Terminal" button opens terminal at project root
- Agent-spawned terminals visible and attachable without interrupting
- Agent raw output/input log tab

### Source Control
- Per-project git integration via git2
- File status, staging, unstaging, committing, discarding changes
- Branch management
- Inline diff viewing
- Each project has its own collapsible section in the Source Control panel

### Settings
- **Themes** — Gruvbox Dark (default), Gruvbox Light, custom upload (TOML/JSON)
- **Fonts** — Font family (Google Font URL or custom upload), font size
- **Keybindings** — VS Code JSON format import compatible
- **AI Providers** — API key management (OS keychain), model selection, temperature
- **Accounts** — GitHub OAuth connection
- **Per-project overrides** — Projects can override global settings

---

## Rust Crate Architecture

```
Cargo Workspace
├── src-tauri/              (binary — Tauri app entry point)
│   ├── Depends on all crates below
│   ├── Tauri commands (the IPC bridge)
│   ├── AppState (holds DB, workspace, buffers, terminals, agent)
│   └── Event definitions (backend → frontend)
│
├── crates/rustic-core/     (library — core data structures)
│   ├── buffer/   — Rope text buffer, edits, undo/redo, line cache
│   ├── syntax/   — Tree-sitter highlighting, language registry
│   ├── workspace/— Multi-project workspace model, file tree
│   ├── search/   — Content search (ripgrep-based), file search
│   └── config/   — Theme, keymap, settings types
│
├── crates/rustic-db/       (library — persistence layer)
│   ├── migrations/ — SQL schema files
│   ├── connection  — SQLite setup, WAL mode, migrations
│   └── *_repo      — CRUD for projects, tasks, checkpoints, settings, MCP
│
├── crates/rustic-agent/    (library — AI agent system)
│   ├── provider/ — AiProvider trait + Claude/OpenAI/Gemini/Compatible impls
│   ├── task/     — Task executor (agentic loop), permissions
│   ├── tools/    — Built-in tools (file ops, terminal, search)
│   ├── mcp/      — MCP client (JSON-RPC over stdio/SSE)
│   └── checkpoint/ — Shadow git, file snapshots
│
├── crates/rustic-git/      (library — git integration)
│   ├── repo, status, diff, branch operations via git2
│
└── crates/rustic-terminal/  (library — terminal emulation)
    ├── PTY spawning (portable-pty)
    └── Shell session management
```

---

## Frontend Architecture (Vanilla JS)

```
src/
├── index.html              — Single HTML entry point
├── main.js                 — App initialization, Tauri API setup
├── styles/
│   ├── global.css          — Reset, base styles, CSS variables
│   ├── theme.css           — Gruvbox + theme variable definitions
│   ├── layout.css          — CSS Grid for main shell
│   └── *.css               — Component-specific styles
├── components/
│   ├── top-bar.js          — Logo, menus, toggles, window controls
│   ├── activity-bar.js     — Icon sidebar
│   ├── primary-sidebar.js  — Panel container
│   ├── secondary-sidebar.js— Agent chat panel
│   ├── editor-area.js      — Tab bar + editor viewport
│   ├── bottom-panel.js     — Terminal panel
│   ├── explorer/           — File explorer components
│   ├── editor/             — Editor pane, virtual scroll, line renderer
│   ├── terminal/           — Terminal tabs, pane, agent logs
│   ├── search/             — Search panel, input, results
│   ├── git/                — Source control panel, project SCM, diff
│   ├── agent/              — Agent panel, task list, chat, MCP config
│   └── settings/           — Settings panel, theme editor, AI config
├── state/
│   ├── store.js            — Lightweight reactive store (custom, ~50 lines)
│   ├── workspace.js        — Project/workspace state
│   ├── editor.js           — Buffer/tab state
│   ├── ui.js               — Sidebar/panel visibility
│   ├── terminal.js         — Terminal sessions
│   ├── search.js           — Search state
│   ├── git.js              — Git state per project
│   ├── agent.js            — Agent tasks, active chat
│   └── settings.js         — User preferences
├── lib/
│   ├── tauri-api.js        — Wrappers around invoke() calls
│   ├── events.js           — Tauri event listeners
│   ├── keybindings.js      — Keyboard shortcut handling
│   └── theme.js            — Theme CSS variable application
└── utils/
    ├── dom.js              — DOM helper utilities
    ├── virtual-scroll.js   — Virtual scrolling engine
    └── debounce.js         — Debounce/throttle utilities
```

**Pattern:** Each component is an ES module that exports a `create()` or `render()` function returning a DOM element. State is managed through a lightweight custom reactive store (pub/sub pattern). No framework, no build step beyond Vite's dev server.

---

## Database Schema (SQLite)

| Table | Purpose |
|-------|---------|
| `projects` | Registered project folders (id, name, root_path, settings overrides) |
| `user_settings` | Key-value settings store |
| `tasks` | Agent tasks (id, project_id, title, status, provider, model) |
| `messages` | Chat messages per task (role, content JSON, sort order) |
| `checkpoints` | Snapshot markers per task/message |
| `file_snapshots` | File contents before AI edits (linked to checkpoints) |
| `mcp_servers` | MCP server configurations |

---

## What Makes Rustic Different

1. **Multi-project first** — Not an afterthought like VS Code's multi-root workspaces. Every feature (explorer, search, git, agent, terminal) is designed around parallel project management.

2. **Agent with rollback** — The checkpoint system means AI edits are never destructive. Every change is snapshotted and revertible to any point in the conversation.

3. **Parallel agent tasks** — Run multiple AI tasks across projects (or within the same project) simultaneously.

4. **Pure Rust backend** — All heavy operations in Rust for maximum performance. The frontend is just a thin rendering layer.

5. **No framework tax** — Vanilla JS frontend means zero overhead, instant startup, and full control over every DOM operation.
