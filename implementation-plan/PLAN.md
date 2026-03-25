# Rustic — Implementation Plan

> **Frontend:** Vanilla JS + CSS + HTML (no framework, no TypeScript)
> **Backend:** Rust + Tauri 2
> **Database:** SQLite (rusqlite)
> **Editor Engine:** Ropey + Tree-sitter

---

## Table of Contents

- [Project Structure](#project-structure)
- [Phase 1: Project Scaffold and Shell UI](#phase-1-project-scaffold-and-shell-ui)
- [Phase 2: File Explorer (Multi-Project)](#phase-2-file-explorer-multi-project)
- [Phase 3: Editor Core](#phase-3-editor-core)
- [Phase 4: Tabs and Multi-File Editing](#phase-4-tabs-and-multi-file-editing)
- [Phase 5: Terminal Integration](#phase-5-terminal-integration)
- [Phase 6: Search](#phase-6-search)
- [Phase 7: Source Control](#phase-7-source-control)
- [Phase 8: Agent System](#phase-8-agent-system)
- [Phase 9: MCP Integration](#phase-9-mcp-integration)
- [Phase 10: Shadow Git / Checkpoint System](#phase-10-shadow-git--checkpoint-system)
- [Phase 11: Settings Panel](#phase-11-settings-panel)
- [Phase 12: SQLite Database Integration](#phase-12-sqlite-database-integration)
- [Phase 13: LSP Client](#phase-13-lsp-client)
- [Phase 14: Polish, Packaging, Logo/Branding](#phase-14-polish-packaging-logobranding)
- [Dependency Graph](#dependency-graph)

---

## Project Structure

```
d:\Programming\Projects\Personal\Rustic\
├── Cargo.toml                          # Workspace root
├── package.json                        # Frontend dependencies (xterm, vite, @tauri-apps/api)
├── vite.config.js                      # Vite config for Tauri dev server
├── rustic_icon.svg                     # Logo asset
├── implementation-plan/
│   ├── overview.md                     # Project overview
│   ├── prerequisites.md               # Setup requirements
│   └── PLAN.md                         # This file
│
├── crates/
│   ├── rustic-core/                    # Core data structures & business logic
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── buffer/
│   │       │   ├── mod.rs              # Rope-based text buffer
│   │       │   ├── rope.rs             # Rope operations (using ropey crate)
│   │       │   ├── edit.rs             # Edit operations, undo/redo
│   │       │   └── line_cache.rs       # Line-indexed access for virtual scroll
│   │       ├── syntax/
│   │       │   ├── mod.rs              # Tree-sitter integration
│   │       │   ├── highlight.rs        # Syntax highlighting queries
│   │       │   └── languages.rs        # Language registry & grammar loading
│   │       ├── workspace/
│   │       │   ├── mod.rs              # Multi-project workspace model
│   │       │   ├── project.rs          # Single project representation
│   │       │   └── file_tree.rs        # File tree data model
│   │       ├── search/
│   │       │   ├── mod.rs              # Search engine
│   │       │   ├── file_search.rs      # Per-project file search
│   │       │   └── content_search.rs   # Grep-like content search (ripgrep-based)
│   │       └── config/
│   │           ├── mod.rs              # Configuration types
│   │           ├── theme.rs            # Theme data model (Gruvbox, custom)
│   │           ├── keymap.rs           # Keybinding model (VS Code JSON compat)
│   │           └── settings.rs         # All settings types
│   │
│   ├── rustic-db/                      # SQLite database layer
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── migrations/             # SQL migration files
│   │       │   ├── 001_initial.sql
│   │       │   ├── 002_agent_tasks.sql
│   │       │   └── 003_checkpoints.sql
│   │       ├── connection.rs           # Connection pool & init
│   │       ├── models.rs              # DB row types
│   │       ├── project_repo.rs        # Project metadata CRUD
│   │       ├── task_repo.rs           # Agent task & conversation CRUD
│   │       ├── checkpoint_repo.rs     # Snapshot/checkpoint CRUD
│   │       ├── settings_repo.rs       # User preferences CRUD
│   │       └── mcp_repo.rs           # MCP configuration CRUD
│   │
│   ├── rustic-agent/                   # AI agent system
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider/
│   │       │   ├── mod.rs              # AiProvider trait
│   │       │   ├── claude.rs           # Anthropic Claude API
│   │       │   ├── openai.rs           # OpenAI API
│   │       │   ├── gemini.rs           # Google Gemini API
│   │       │   └── compatible.rs       # Generic OpenAI-compatible (OpenRouter, Grok, etc.)
│   │       ├── task/
│   │       │   ├── mod.rs              # Task orchestration
│   │       │   ├── executor.rs         # Task execution loop (agentic loop)
│   │       │   └── permissions.rs      # Permission system (global + per-project)
│   │       ├── tools/
│   │       │   ├── mod.rs              # Tool definitions for agent
│   │       │   ├── file_ops.rs         # Read/write/create file tools
│   │       │   ├── terminal.rs         # Run command tool
│   │       │   └── search.rs           # Search tool
│   │       ├── mcp/
│   │       │   ├── mod.rs              # MCP client implementation
│   │       │   ├── client.rs           # MCP protocol client (JSON-RPC)
│   │       │   └── config.rs           # MCP server configuration
│   │       ├── checkpoint/
│   │       │   ├── mod.rs              # Shadow git / checkpoint manager
│   │       │   └── snapshot.rs         # File snapshot operations
│   │       └── config.rs              # AiConfig, provider config types
│   │
│   ├── rustic-git/                     # Git integration
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── repo.rs                # Repository operations (via git2)
│   │       ├── status.rs              # File status tracking
│   │       ├── diff.rs                # Diff computation
│   │       └── branch.rs             # Branch operations
│   │
│   └── rustic-terminal/               # Terminal emulation
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── pty.rs                  # PTY spawning (portable-pty)
│           ├── shell.rs               # Shell session management
│           └── ansi.rs                # ANSI escape code parsing
│
├── src-tauri/                          # Tauri application (the binary)
│   ├── Cargo.toml                     # Depends on all crates above
│   ├── tauri.conf.json
│   ├── capabilities/
│   │   └── default.json
│   ├── icons/                         # App icons generated from SVG
│   ├── src/
│   │   ├── main.rs                    # Tauri entry point
│   │   ├── lib.rs                     # App setup, plugin registration
│   │   ├── state.rs                   # AppState (holds DB, workspace, etc.)
│   │   ├── commands/
│   │   │   ├── mod.rs                 # Re-exports all command modules
│   │   │   ├── workspace.rs           # #[tauri::command] for project/workspace ops
│   │   │   ├── editor.rs             # #[tauri::command] for buffer/edit ops
│   │   │   ├── file_tree.rs          # #[tauri::command] for file tree ops
│   │   │   ├── search.rs             # #[tauri::command] for search ops
│   │   │   ├── git.rs                # #[tauri::command] for git ops
│   │   │   ├── terminal.rs           # #[tauri::command] for terminal ops
│   │   │   ├── agent.rs              # #[tauri::command] for agent ops
│   │   │   ├── settings.rs           # #[tauri::command] for settings ops
│   │   │   └── checkpoint.rs         # #[tauri::command] for checkpoint ops
│   │   └── events.rs                 # Tauri event definitions (backend -> frontend)
│   └── build.rs
│
├── src/                                # Vanilla JS + CSS + HTML frontend
│   ├── index.html                     # Single entry point
│   ├── main.js                        # App initialization
│   ├── styles/
│   │   ├── global.css                 # CSS reset, base variables
│   │   ├── theme.css                  # Gruvbox + theme definitions
│   │   ├── layout.css                 # CSS Grid for main shell
│   │   ├── activity-bar.css
│   │   ├── sidebar.css
│   │   ├── editor.css
│   │   ├── terminal.css
│   │   ├── tabs.css
│   │   └── settings.css
│   ├── components/
│   │   ├── top-bar.js                 # Logo, menus, toggles, window controls
│   │   ├── activity-bar.js            # Narrow icon bar
│   │   ├── primary-sidebar.js         # Panel container that switches content
│   │   ├── secondary-sidebar.js       # Agent chat panel (right side)
│   │   ├── editor-area.js             # Tab bar + editor viewport container
│   │   ├── bottom-panel.js            # Terminal panel container
│   │   ├── status-bar.js              # Bottom status bar
│   │   ├── explorer/
│   │   │   ├── explorer.js            # Multi-project explorer view
│   │   │   ├── project-section.js     # Collapsible project with action buttons
│   │   │   ├── file-tree.js           # Recursive file/folder tree
│   │   │   └── file-tree-item.js      # Single file/folder row
│   │   ├── editor/
│   │   │   ├── tab-bar.js             # File tabs with [ProjectName] prefix
│   │   │   ├── tab.js                 # Single tab component
│   │   │   ├── editor-pane.js         # Single editor instance
│   │   │   ├── virtual-scroll.js      # Virtual scrolling viewport
│   │   │   ├── line-renderer.js       # Renders a single line with highlights
│   │   │   └── gutter-renderer.js     # Line numbers, fold markers
│   │   ├── terminal/
│   │   │   ├── terminal-tabs.js       # Terminal tab bar
│   │   │   ├── terminal-pane.js       # Single terminal instance (xterm.js)
│   │   │   └── agent-logs.js          # Agent output/input logs
│   │   ├── search/
│   │   │   ├── search-panel.js        # Search sidebar view
│   │   │   ├── search-input.js        # Search input with options
│   │   │   └── search-results.js      # Results grouped by project/file
│   │   ├── git/
│   │   │   ├── source-control.js      # Source control sidebar view
│   │   │   ├── project-scm.js         # Per-project git section
│   │   │   └── diff-view.js           # Inline diff viewer
│   │   ├── agent/
│   │   │   ├── agent-panel.js         # Agent sidebar (project list + tasks)
│   │   │   ├── task-list.js           # Per-project task list
│   │   │   ├── chat-view.js           # Chat conversation in secondary sidebar
│   │   │   ├── chat-message.js        # Single message (user/assistant)
│   │   │   ├── tool-call-result.js    # Collapsible tool call display
│   │   │   ├── mcp-config.js          # MCP configuration sub-panel
│   │   │   └── permission-badge.js    # Permission level indicator
│   │   └── settings/
│   │       ├── settings-panel.js      # Full settings view
│   │       ├── general-settings.js    # Theme, font, keybindings
│   │       ├── ai-settings.js         # Provider API keys, model selection
│   │       ├── theme-editor.js        # Theme preview & upload
│   │       └── account-settings.js    # GitHub OAuth
│   ├── state/
│   │   ├── store.js                   # Lightweight reactive store (~50 lines)
│   │   ├── workspace.js               # Workspace/project state
│   │   ├── editor.js                  # Editor/buffer state
│   │   ├── ui.js                      # UI visibility state
│   │   ├── terminal.js                # Terminal sessions state
│   │   ├── search.js                  # Search state
│   │   ├── git.js                     # Git state per project
│   │   ├── agent.js                   # Agent tasks state
│   │   └── settings.js               # Settings/config state
│   ├── lib/
│   │   ├── tauri-api.js               # Typed wrappers around invoke() calls
│   │   ├── events.js                  # Tauri event listeners
│   │   ├── keybindings.js             # Keyboard shortcut handling
│   │   └── theme.js                   # Theme CSS variable application
│   └── utils/
│       ├── dom.js                     # DOM helper utilities (createElement, etc.)
│       ├── virtual-scroll.js          # Virtual scrolling engine
│       └── debounce.js                # Debounce/throttle utilities
```

### Cargo Workspace `Cargo.toml` (root)

Members:
- `crates/rustic-core`
- `crates/rustic-db`
- `crates/rustic-agent`
- `crates/rustic-git`
- `crates/rustic-terminal`
- `src-tauri`

### Key Crate Dependencies

| Crate | Key Dependencies |
|---|---|
| `rustic-core` | `ropey`, `tree-sitter`, `tree-sitter-*` (language grammars), `serde`, `ignore` (for .gitignore-aware file walking) |
| `rustic-db` | `rusqlite` (with `bundled` feature), `serde`, `serde_json` |
| `rustic-agent` | `reqwest`, `serde_json`, `tokio`, `keyring`, `async-trait`, `futures`, `rustic-core`, `rustic-db` |
| `rustic-git` | `git2`, `serde` |
| `rustic-terminal` | `portable-pty`, `tokio`, `bytes` |
| `src-tauri` | `tauri` (v2), all `rustic-*` crates, `tokio`, `serde`, `serde_json` |

### Frontend Dependencies (`package.json`)

| Package | Purpose |
|---|---|
| `@tauri-apps/api` | Tauri IPC (invoke, events, window) |
| `xterm` | Terminal rendering in browser |
| `@xterm/addon-fit` | Auto-resize terminal |
| `vite` | Dev server for Tauri (dev dependency only) |

---

## Phase 1: Project Scaffold and Shell UI

**Goal:** Bootable Tauri 2 + Vanilla JS app with the VS Code-like layout shell. No functionality — just the visual skeleton with Gruvbox theming.

### Step 1.1: Initialize project

1. Create `package.json` with dependencies:
   - `@tauri-apps/api` v2, `xterm`, `@xterm/addon-fit`
   - Dev: `vite`
2. Create `vite.config.js`:
   ```js
   export default {
     clearScreen: false,
     server: { port: 1420, strictPort: true },
     envPrefix: ['VITE_', 'TAURI_'],
     build: { target: 'esnext', outDir: 'dist' }
   };
   ```
3. Create `src/index.html`:
   ```html
   <!DOCTYPE html>
   <html lang="en">
   <head>
     <meta charset="UTF-8" />
     <meta name="viewport" content="width=device-width, initial-scale=1.0" />
     <title>Rustic</title>
     <link rel="stylesheet" href="/styles/global.css" />
     <link rel="stylesheet" href="/styles/theme.css" />
     <link rel="stylesheet" href="/styles/layout.css" />
   </head>
   <body>
     <div id="app"></div>
     <script type="module" src="/main.js"></script>
   </body>
   </html>
   ```
4. Initialize the Cargo workspace root `Cargo.toml` with `[workspace]` listing all member crate paths
5. Create `src-tauri/Cargo.toml` depending on `tauri` v2 with features: `["devtools"]`
6. Create `src-tauri/tauri.conf.json` with:
   - `identifier`: `com.rustic.editor`
   - `windows`: single window, decorations OFF (we draw our own title bar)
   - `title`: `Rustic`
   - `width`: 1280, `height`: 800, `minWidth`: 800, `minHeight`: 600
7. Create `src-tauri/src/main.rs` with the standard Tauri 2 bootstrap:
   ```rust
   fn main() {
       tauri::Builder::default()
           .run(tauri::generate_context!())
           .expect("error while running tauri application");
   }
   ```
8. Create stub crate directories (`crates/rustic-core/`, etc.) each with a minimal `Cargo.toml` and `src/lib.rs`
9. Verify `cargo build` succeeds and `npm run tauri dev` opens a blank window

### Step 1.2: CSS theming foundation (Gruvbox)

1. Create `src/styles/global.css`:
   - CSS reset (box-sizing, margin, padding, no scrollbar flash)
   - Set `html, body, #app` to `height: 100%; overflow: hidden;`
   - Define `--font-family`, `--font-size`, `--font-family-mono` variables
   - Disable user-select on UI chrome (not editor content)
2. Create `src/styles/theme.css`:
   - Gruvbox Dark as default via CSS custom properties:
     - Backgrounds: `--bg-hard: #1d2021`, `--bg: #282828`, `--bg-soft: #32302f`, `--bg1: #3c3836`, `--bg2: #504945`, `--bg3: #665c54`, `--bg4: #7c6f64`
     - Foregrounds: `--fg: #ebdbb2`, `--fg1: #ebdbb2`, `--fg2: #d5c4a1`, `--fg3: #bdae93`, `--fg4: #a89984`
     - Accent colors: `--red: #cc241d`, `--green: #98971a`, `--yellow: #d79921`, `--blue: #458588`, `--purple: #b16286`, `--aqua: #689d6a`, `--orange: #d65d0e`
     - Bright variants: `--bright-red: #fb4934`, `--bright-green: #b8bb26`, `--bright-yellow: #fabd2f`, `--bright-blue: #83a598`, `--bright-purple: #d3869b`, `--bright-aqua: #8ec07c`, `--bright-orange: #fe8019`
     - UI variables: `--accent: var(--bright-aqua)`, `--border: var(--bg1)`, `--hover-bg: var(--bg1)`, `--active-bg: var(--bg2)`, `--selection-bg: var(--bg2)`
   - Gruvbox Light as `[data-theme="light"]` override set
   - Token colors for syntax: `--token-keyword`, `--token-string`, `--token-comment`, `--token-function`, `--token-type`, `--token-variable`, `--token-number`, `--token-operator`, `--token-punctuation`

### Step 1.3: Reactive state store

1. Create `src/state/store.js` — a lightweight pub/sub reactive store:
   ```js
   // ~50 lines. Creates observable state objects.
   // store.create({ key: initialValue }) returns { get, set, subscribe }
   // Components subscribe to state changes and re-render only what changed.
   ```
   - `createStore(initialState)` → returns `{ getState(), setState(partial), subscribe(key, callback) }`
   - When `setState` is called, only callbacks subscribed to changed keys fire
   - This replaces any framework reactivity — simple, fast, explicit
2. Create `src/state/ui.js`:
   - `activePanel`: `'explorer'` (which activity bar icon is active)
   - `primarySidebarVisible`: `true`
   - `bottomPanelVisible`: `true`
   - `secondarySidebarVisible`: `false`
   - `sidebarWidth`: `260`
   - `panelHeight`: `200`
   - `secondarySidebarWidth`: `350`

### Step 1.4: DOM utility helpers

1. Create `src/utils/dom.js`:
   ```js
   // Helper to create DOM elements cleanly:
   // el('div', { class: 'foo', onclick: handler }, [
   //   el('span', {}, 'Hello'),
   //   el('button', { class: 'btn' }, 'Click')
   // ])
   export function el(tag, attrs = {}, children = []) { ... }

   // Mount a component into a container, replacing contents
   export function mount(container, element) { ... }

   // Create inline SVG icon from path data
   export function icon(pathData, size = 16) { ... }
   ```

### Step 1.5: Main layout shell

1. Create `src/styles/layout.css` with CSS Grid:
   ```css
   #app {
     display: grid;
     grid-template-areas:
       "topbar    topbar     topbar     topbar"
       "activity  sidebar    editor     secondary"
       "activity  sidebar    panel      secondary";
     grid-template-columns: 48px var(--sidebar-width) 1fr var(--secondary-width);
     grid-template-rows: 35px 1fr var(--panel-height);
     height: 100vh;
   }
   ```
2. Create `src/main.js`:
   - Import all component modules
   - Build the app layout by calling each component's `create()` function
   - Mount into `#app`
   - Initialize Tauri event listeners
3. Create `src/components/top-bar.js`:
   - Left: Rustic logo (inline SVG, 20x20)
   - Left-center: Menu items as `<button>` elements: File, Edit, View, Agent, Help (dropdown menus deferred to Phase 14 — for now just labels)
   - Right: Toggle buttons (icons for: primary sidebar, bottom panel, secondary sidebar)
   - Far right: Window controls (minimize, maximize, close) using `@tauri-apps/api/window` — `getCurrentWindow().minimize()`, `.toggleMaximize()`, `.close()`
   - Add `data-tauri-drag-region` attribute on the top bar for window dragging (since native decorations are OFF)
4. Create `src/components/activity-bar.js`:
   - Vertical strip of icon buttons: Explorer (files icon), Search (magnifier), Source Control (branch icon), Agent (sparkle/robot icon)
   - Bottom section: Settings (gear icon), Account (person icon)
   - Active item has left border accent + lighter background
   - Clicking sets `activePanel` in ui store
   - Icons: inline SVGs using simple path data (no icon library)
5. Create `src/components/primary-sidebar.js`:
   - Reads `activePanel` from ui store
   - Swaps content between Explorer, Search, SourceControl, Agent panels
   - For now, each panel is a placeholder `<div>` with the panel name
   - Header bar showing the panel name
6. Create `src/components/editor-area.js`:
   - Placeholder showing "Open a file to start editing" centered text
   - Will later contain tab bar + editor pane
7. Create `src/components/secondary-sidebar.js`:
   - Hidden by default (width 0 or `display: none`)
   - Toggled via top bar button or when an agent task is clicked
   - Placeholder: "Agent Chat" header
8. Create `src/components/bottom-panel.js`:
   - Top bar with "Terminal" tab label + minimize button
   - Placeholder content area
   - Will later contain xterm.js terminal

### Step 1.6: Resizable panels

1. Implement drag-to-resize handles between:
   - Primary sidebar ↔ editor area (vertical splitter)
   - Editor area ↔ bottom panel (horizontal splitter)
   - Editor area ↔ secondary sidebar (vertical splitter)
2. Implementation: 4px-wide/tall drag handle div. On `mousedown`, track `mousemove` globally, update CSS custom properties (`--sidebar-width`, `--panel-height`, `--secondary-width`). Use `cursor: col-resize` / `row-resize`.
3. On `mouseup`, persist sizes to ui store.

**Deliverable:** A Gruvbox-themed window that looks like VS Code's layout skeleton with resizable panels and toggle buttons. No file/editor functionality yet.

---

## Phase 2: File Explorer (Multi-Project)

**Goal:** Working file explorer that can open multiple project folders, display file trees, and navigate.

**Depends on:** Phase 1

### Step 2.1: Workspace model in `rustic-core`

1. In `crates/rustic-core/src/workspace/project.rs`:
   - Define `Project` struct: `id: Uuid`, `name: String`, `root_path: PathBuf`, `is_expanded: bool`
   - Implement `Serialize`/`Deserialize` for Tauri IPC
2. In `crates/rustic-core/src/workspace/file_tree.rs`:
   - Define `FileNode` struct: `path: PathBuf`, `name: String`, `is_dir: bool`, `children: Option<Vec<FileNode>>`, `depth: u32`
   - `read_directory(path: &Path, depth: u32, max_depth: u32) -> Result<Vec<FileNode>>` — reads directory lazily (only one level at a time)
   - Respect `.gitignore` using the `ignore` crate's `WalkBuilder`
   - Sort: directories first, then alphabetical (case-insensitive)
3. In `crates/rustic-core/src/workspace/mod.rs`:
   - Define `Workspace` struct: `projects: Vec<Project>`
   - Methods: `add_project(path) -> Project`, `remove_project(id)`, `list_projects() -> Vec<Project>`

### Step 2.2: Tauri commands for workspace

1. In `src-tauri/src/state.rs`:
   - Define `AppState` struct holding `workspace: Mutex<Workspace>`, `db: Database`
   - Register as Tauri managed state via `.manage(AppState::new())`
2. In `src-tauri/src/commands/workspace.rs`:
   - `#[tauri::command] async fn add_project(state, path: String) -> Result<Project, String>` — opens native folder dialog if path is empty, otherwise uses provided path
   - `#[tauri::command] async fn remove_project(state, project_id: String) -> Result<(), String>`
   - `#[tauri::command] async fn list_projects(state) -> Result<Vec<Project>, String>`
3. In `src-tauri/src/commands/file_tree.rs`:
   - `#[tauri::command] async fn read_dir(path: String) -> Result<Vec<FileNode>, String>` — lazy directory listing
   - `#[tauri::command] async fn read_file(path: String) -> Result<String, String>` — read file as UTF-8

### Step 2.3: Frontend file explorer

1. Create `src/state/workspace.js`:
   - State: `projects` array (each with `id`, `name`, `rootPath`, `isExpanded`, `children`)
   - Functions: `addProject()`, `removeProject(id)`, `toggleProject(id)`, `loadChildren(path)`
   - On app start, call `invoke('list_projects')` to restore previous session's projects
2. Create `src/components/explorer/explorer.js`:
   - Header: "EXPLORER" label + "Add Project" button (folder+ icon)
   - Renders a `project-section` for each project in workspace state
   - "Add Project" calls `invoke('add_project', { path: '' })` → OS folder picker → updates state
3. Create `src/components/explorer/project-section.js`:
   - Collapsible header: caret icon + project name (bold)
   - Action buttons in header (visible on hover): New File, New Folder, Refresh, New Terminal
   - When expanded, renders file tree for the project root
4. Create `src/components/explorer/file-tree.js`:
   - Renders file/folder items for a given directory
   - Lazy loading: when a folder is expanded, call `invoke('read_dir', { path })` and cache results
5. Create `src/components/explorer/file-tree-item.js`:
   - Indentation: `padding-left: depth * 16px`
   - Folder: caret + folder icon. File: file icon (simple extension-based mapping)
   - Click folder: toggle expanded, load children if not cached
   - Click file: emit event to open in editor (wired in Phase 4)
   - Right-click: context menu (defer to Phase 14)

### Step 2.4: File tree performance

1. Only load 1 level deep on expand. Sub-directories load on demand.
2. For massive directories (1000+ items in one folder): virtual scrolling on the file list within the sidebar.
3. File system watching: use `notify` crate in Rust backend, emit Tauri events on changes, frontend refreshes affected subtree.

**Deliverable:** Multi-project explorer with add/remove folders and lazy-loading file trees.

---

## Phase 3: Editor Core

**Goal:** Open a file, display contents with syntax highlighting and virtual scrolling. Basic text editing.

**Depends on:** Phase 1

### Step 3.1: Rope buffer in `rustic-core`

1. In `crates/rustic-core/src/buffer/rope.rs`:
   - Use the `ropey` crate
   - Define `Buffer` struct: `id: BufferId`, `rope: Rope`, `file_path: Option<PathBuf>`, `is_modified: bool`, `language: Option<String>`, `undo_stack: Vec<EditGroup>`, `redo_stack: Vec<EditGroup>`
   - `BufferId` is a `u64` for unique identification
2. In `crates/rustic-core/src/buffer/edit.rs`:
   - Define `Edit` struct: `range: Range<usize>` (byte offsets), `old_text: String`, `new_text: String`
   - `EditGroup`: groups edits within 300ms for undo chunking
   - `Buffer::apply_edit(edit)` — modifies rope, pushes to undo stack, clears redo
   - `Buffer::undo()` — pops undo stack, applies inverse, pushes to redo
   - `Buffer::redo()` — pops redo stack, applies, pushes to undo
3. In `crates/rustic-core/src/buffer/line_cache.rs`:
   - `Buffer::line_count() -> usize`
   - `Buffer::get_line(idx) -> Option<RopeSlice>`
   - `Buffer::get_lines(start, end) -> Vec<String>` — for virtual scroll viewport
   - `Buffer::byte_offset_of_line(idx) -> usize`
   - `Buffer::line_of_byte(offset) -> usize`

### Step 3.2: Tree-sitter syntax highlighting

1. In `crates/rustic-core/src/syntax/languages.rs`:
   - `LanguageRegistry` maps file extensions to tree-sitter `Language` objects
   - Start with ~14 languages: Rust, JavaScript, TypeScript, Python, Go, C, C++, Java, JSON, TOML, HTML, CSS, Markdown
   - Each grammar is a Cargo feature flag (opt-in)
2. In `crates/rustic-core/src/syntax/highlight.rs`:
   - `SyntaxHighlighter` struct: holds `Parser`, `Tree`, highlight `Query`
   - `SyntaxHighlighter::new(language) -> Option<Self>`
   - `SyntaxHighlighter::highlight_lines(rope, start_line, end_line) -> Vec<HighlightedLine>`
   - `HighlightedLine = Vec<Span>` where `Span { start_col, end_col, highlight_class: String }`
   - `highlight_class` maps to token names: `keyword`, `string`, `comment`, `function`, `type`, `variable`, `number`, `operator`, `punctuation`
   - Use `tree_sitter_highlight::Highlighter` and `HighlightConfiguration`
3. Incremental parsing:
   - On edit: call `old_tree.edit(InputEdit { ... })`, reparse with `parser.parse_with(callback, Some(&old_tree))`
   - Only recompute highlights for affected line range + context

### Step 3.3: Tauri commands for editor

1. In `src-tauri/src/state.rs`:
   - Add `buffers: Mutex<HashMap<BufferId, Buffer>>` and `highlighters: Mutex<HashMap<BufferId, SyntaxHighlighter>>` to `AppState`
2. In `src-tauri/src/commands/editor.rs`:
   - `open_file(path) -> Result<{ buffer_id, line_count, language }>` — creates Buffer, detects language, creates highlighter
   - `get_visible_lines(buffer_id, start, end) -> Result<Vec<RenderedLine>>` — returns lines with syntax spans. `RenderedLine { line_number, text, spans: Vec<Span> }`
   - `edit_buffer(buffer_id, line, col, text, delete_count) -> Result<EditResponse>` — applies edit, returns updated line range
   - `save_file(buffer_id) -> Result<()>` — writes rope to disk
   - `undo(buffer_id) -> Result<EditResponse>`
   - `redo(buffer_id) -> Result<EditResponse>`
   - `close_buffer(buffer_id) -> Result<()>`

### Step 3.4: Frontend editor rendering

1. Create `src/state/editor.js`:
   - `openBuffers`: Map of `bufferId -> { id, filePath, fileName, projectName, lineCount, language, isModified }`
   - `activeBufferId`: currently active buffer
   - `viewportLines`: currently visible rendered lines
   - Functions: `openFile(path, projectName)`, `closeBuffer(id)`, `setActiveBuffer(id)`, `saveActiveBuffer()`
2. Create `src/components/editor/editor-pane.js`:
   - Main editor component for a single buffer
   - Contains: gutter (left) + virtual scroll viewport (right)
   - Keyboard input via a hidden `<textarea>` overlay (positioned at cursor for IME support)
   - `onInput`: read input, send `edit_buffer` to backend, clear textarea
   - `onKeyDown`: handle Enter, Backspace, Delete, Tab, arrows, Home/End, Ctrl+Z/Y
3. Create `src/components/editor/virtual-scroll.js` (also `src/utils/virtual-scroll.js` for reuse):
   - Accepts: `lineCount`, `lineHeight` (e.g., 20px), `viewportHeight`
   - Computes: `totalHeight = lineCount * lineHeight`, `visibleStart = Math.floor(scrollTop / lineHeight)`, `visibleEnd = visibleStart + Math.ceil(viewportHeight / lineHeight) + overscan`
   - Renders a container div with `height: totalHeight` (for scrollbar) and an inner div positioned at `top: visibleStart * lineHeight`
   - On scroll: update visible range, call `invoke('get_visible_lines', { buffer_id, start, end })` debounced via requestAnimationFrame
   - Overscan: 10 extra lines above and below for smooth scrolling
4. Create `src/components/editor/line-renderer.js`:
   - Takes a `RenderedLine` object
   - Renders text with `<span>` per syntax span, each with a CSS class: `.token-keyword`, `.token-string`, etc.
   - CSS classes map to theme colors: `.token-keyword { color: var(--token-keyword); }`
5. Create `src/components/editor/gutter-renderer.js`:
   - Renders line numbers for visible lines
   - Dimmer color (`var(--fg4)`), right-aligned
   - Active line number highlighted

### Step 3.5: Basic text input

1. Hidden textarea approach:
   - Position a 1x1px `<textarea>` at the cursor position (for IME positioning)
   - On `input` event: read value, send edit to backend, clear textarea
   - On `keydown`: handle special keys
   - Arrow keys: update cursor position locally, fetch new lines if scrolling past viewport
2. Cursor state: `cursorLine`, `cursorCol` in editor state
3. Cursor rendering: blinking pipe at cursor position (CSS animation, absolute-positioned thin div)
4. Selection: `selectionStart` / `selectionEnd` (line, col) — Shift+arrows extends selection, semi-transparent background on selected ranges

**Deliverable:** Open a file, see syntax-highlighted code with virtual scrolling, type and edit, undo/redo.

---

## Phase 4: Tabs and Multi-File Editing

**Goal:** Open multiple files in tabs, switch between them, project-prefixed tab labels.

**Depends on:** Phase 2 (explorer click-to-open), Phase 3 (editor core)

### Step 4.1: Tab bar

1. Create `src/components/editor/tab-bar.js`:
   - Horizontally scrollable row of tabs from `openBuffers`
   - Active tab: bottom accent border + lighter background
   - Overflow: horizontal scroll with hidden scrollbar (Shift+wheel to scroll)
2. Create `src/components/editor/tab.js`:
   - Display: `[ProjectName] filename.ext` — project name in dimmer color, filename in normal
   - Modified indicator: dot before filename when `isModified`
   - Close button (x) on hover or always on active tab
   - Click: set active buffer
   - Middle-click: close tab
   - Context menu: defer to Phase 14

### Step 4.2: Wire explorer to editor

1. In `file-tree-item.js`, on file click: call `editorState.openFile(filePath, projectName)`
2. In `editor.js` store: `openFile` checks if buffer already exists → activate it. Otherwise call `invoke('open_file')`, add to `openBuffers`, set active.
3. Update `editor-area.js` to render tab bar above editor pane.

### Step 4.3: Buffer management

1. Per-tab state: cursor position, scroll position. Save/restore on tab switch.
2. Dirty indicator on tab when `isModified`
3. Keyboard shortcuts:
   - `Ctrl+S`: save active buffer
   - `Ctrl+W`: close active tab
   - `Ctrl+Tab` / `Ctrl+Shift+Tab`: cycle tabs

**Deliverable:** Full tabbed editing with project-prefixed tabs.

---

## Phase 5: Terminal Integration

**Goal:** Embedded terminal with per-project spawning and agent terminal visibility.

**Depends on:** Phase 1 (bottom panel), Phase 2 (project awareness)

### Step 5.1: PTY backend in `rustic-terminal`

1. In `crates/rustic-terminal/src/pty.rs`:
   - Use `portable-pty` crate for cross-platform PTY
   - `PtySession` struct: `id: SessionId`, `master: Box<dyn MasterPty>`, `child: Box<dyn Child>`, `reader`, `writer`, `cwd: PathBuf`, `label: String`, `is_agent: bool`
   - `PtySession::new(cwd, shell: Option<String>) -> Result<Self>` — spawns default shell at given directory
   - `PtySession::write(data: &[u8])` — send input
   - `PtySession::resize(cols, rows)` — resize PTY
2. In `crates/rustic-terminal/src/shell.rs`:
   - `TerminalManager` struct: `sessions: HashMap<SessionId, PtySession>`
   - `create_session(cwd, label, is_agent) -> SessionId`
   - `destroy_session(id)`
   - `list_sessions() -> Vec<SessionInfo>` where `SessionInfo = { id, label, cwd, is_agent }`
   - Output streaming: spawn a tokio task per session that reads PTY output and sends via Tauri events

### Step 5.2: Tauri commands for terminal

1. In `src-tauri/src/commands/terminal.rs`:
   - `create_terminal(cwd: Option<String>, label: Option<String>, is_agent: bool) -> Result<SessionInfo>`
   - `write_terminal(session_id, data) -> Result<()>`
   - `resize_terminal(session_id, cols, rows) -> Result<()>`
   - `close_terminal(session_id) -> Result<()>`
   - `list_terminals() -> Result<Vec<SessionInfo>>`
2. Event streaming: emit `terminal-output` event with `{ session_id, data }` for each PTY read chunk

### Step 5.3: Frontend terminal

1. Create `src/state/terminal.js`:
   - `sessions`: array of `SessionInfo`
   - `activeSessionId`: currently active terminal
   - Functions: `createTerminal(cwd, label)`, `closeTerminal(id)`, `setActive(id)`
2. Create `src/components/terminal/terminal-tabs.js`:
   - Tab bar showing all sessions (label + close button)
   - "+" button to create new terminal (at app working directory)
   - Agent terminals shown with distinct icon/label
3. Create `src/components/terminal/terminal-pane.js`:
   - Mount `xterm.js` Terminal instance for active session
   - On mount: `terminal.open(container)`, apply fit addon, call `resize_terminal` with computed size
   - Listen to `terminal-output` event → `terminal.write(data)`
   - On keypress: `invoke('write_terminal', { session_id, data })`
   - Theme xterm.js to match Gruvbox colors

### Step 5.4: Per-project terminal

1. In `project-section.js`, "New Terminal" button calls `createTerminal(project.rootPath, project.name)`
2. Opens bottom panel if hidden, switches to new terminal tab
3. Agent terminals (wired in Phase 8): labeled "Agent: task-name", user can attach/view without interrupting

**Deliverable:** Working terminal in bottom panel with tabs and per-project spawning.

---

## Phase 6: Search

**Goal:** Per-project and global text search with results display.

**Depends on:** Phase 2 (workspace/project model)

### Step 6.1: Search backend in `rustic-core`

1. In `crates/rustic-core/src/search/content_search.rs`:
   - Use `grep-regex` + `grep-searcher` crates (the libraries behind ripgrep), or `ignore` crate for walking + regex matching
   - `SearchEngine::search(query: &SearchQuery) -> Vec<SearchResult>`
   - `SearchQuery { pattern, is_regex, case_sensitive, whole_word, paths: Vec<PathBuf>, include_glob, exclude_glob }`
   - `SearchResult { file_path, matches: Vec<SearchMatch> }`
   - `SearchMatch { line_number, line_text, match_start, match_end }`
   - Stream results via channel for incremental UI updates
2. In `crates/rustic-core/src/search/file_search.rs`:
   - Quick file name fuzzy search: `find_files(query, paths) -> Vec<PathBuf>` (for Ctrl+P, Phase 14)

### Step 6.2: Tauri commands for search

1. In `src-tauri/src/commands/search.rs`:
   - `search_in_project(project_id, query) -> Result<Vec<SearchResult>>`
   - `search_global(query) -> Result<Vec<SearchResult>>`
   - Streaming: emit `search-result` events as matches are found, `search-complete` when done
   - `cancel_search()` — sets cancellation token

### Step 6.3: Frontend search panel

1. Create `src/state/search.js`:
   - `query`, `results`, `isSearching`, `scope` ('global' or project ID), `options` (regex, case, wholeWord)
2. Create `src/components/search/search-panel.js`:
   - Scope selector: dropdown with "All Projects" + individual project names
   - Search input with toggle buttons (regex, case-sensitive, whole-word)
   - Results area
3. Create `src/components/search/search-results.js`:
   - Results grouped by file (collapsible)
   - File path with project prefix for global search
   - Each match: line number + text with match highlighted in accent color
   - Click match: open file in editor and scroll to line

**Deliverable:** Working search with per-project and global scope.

---

## Phase 7: Source Control (Git)

**Goal:** Per-project git status, staging, committing, diff viewing.

**Depends on:** Phase 2 (multi-project workspace)

### Step 7.1: Git backend in `rustic-git`

1. In `crates/rustic-git/src/repo.rs`:
   - `GitRepo::open(path) -> Result<Self>` using `git2::Repository::discover(path)`
   - `GitRepo::head_branch() -> Result<String>`
   - `GitRepo::branches() -> Result<Vec<BranchInfo>>`
2. In `crates/rustic-git/src/status.rs`:
   - `GitRepo::status() -> Result<Vec<FileStatus>>` with `StatusType` enum: `New, Modified, Deleted, Renamed, Untracked, Conflicted`
   - `GitRepo::stage(paths)`, `unstage(paths)`, `commit(message)`, `discard_changes(paths)`
3. In `crates/rustic-git/src/diff.rs`:
   - `GitRepo::diff_file(path) -> Result<FileDiff>` — hunks with added/removed lines
   - `GitRepo::diff_staged() -> Result<Vec<FileDiff>>`

### Step 7.2: Tauri commands for git

1. In `src-tauri/src/commands/git.rs`:
   - `git_status(project_id) -> Result<{ branch, files, ahead, behind }>`
   - `git_stage(project_id, paths)`, `git_unstage(...)`, `git_commit(project_id, message)`, `git_discard(project_id, paths)`
   - `git_diff(project_id, path) -> Result<FileDiff>`
   - `git_branches(project_id)`, `git_checkout_branch(project_id, branch)`

### Step 7.3: Frontend source control

1. Create `src/state/git.js`:
   - `projectStatuses`: Map of project ID → git status
   - Auto-refresh on file save, focus, and fs-watch events
2. Create `src/components/git/source-control.js`:
   - Per-project collapsible sections (like explorer)
   - Each section: branch name, file change count badge
3. Create `src/components/git/project-scm.js`:
   - Staged changes list + unstaged changes list (collapsible)
   - Per-file: status icon + path + action buttons (stage/unstage, discard)
   - Click file: open diff view
   - Commit: text input + commit button at top
   - "Stage All" / "Unstage All" buttons
4. Create `src/components/git/diff-view.js`:
   - Inline diff with red/green highlighting
   - Read-only special editor mode

**Deliverable:** Per-project git integration with staging, committing, diffing.

---

## Phase 8: Agent System

**Goal:** Multi-provider AI agent with per-project tasks, tool use, parallel execution, and chat UI.

**Depends on:** Phase 3 (editor/buffer), Phase 5 (terminal), Phase 12 (SQLite)

### Step 8.1: AI provider abstraction

1. In `crates/rustic-agent/src/provider/mod.rs`:
   ```rust
   #[async_trait]
   pub trait AiProvider: Send + Sync {
       async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolDef>, config: &ProviderConfig) -> Result<AiResponse>;
       async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolDef>, config: &ProviderConfig) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>>>>>;
       fn name(&self) -> &str;
       fn available_models(&self) -> Vec<ModelInfo>;
   }
   ```
   - `Message { role: Role, content: Vec<ContentBlock> }` — `Role = User | Assistant | System | Tool`
   - `ContentBlock = Text(String) | ToolUse { id, name, input } | ToolResult { tool_use_id, content }`
   - `AiResponse { content: Vec<ContentBlock>, usage: TokenUsage, stop_reason: StopReason }`
   - `StreamChunk` = incremental text or tool use delta
2. In `crates/rustic-agent/src/config.rs`:
   - `AiConfig { providers: Vec<ProviderEntry> }`
   - `ProviderEntry { provider_type, api_key_id, default_model, base_url: Option<String> }`
   - `ProviderType` enum: `Claude, OpenAi, Gemini, Compatible`
   - API keys via `keyring` crate (OS keychain)

### Step 8.2: Provider implementations

1. `claude.rs`: Anthropic API (`https://api.anthropic.com/v1/messages`), SSE streaming, tool use format, `anthropic-version` header
2. `openai.rs`: OpenAI API (`https://api.openai.com/v1/chat/completions`), maps internal format to OpenAI's `tool_calls`
3. `gemini.rs`: Gemini API (`https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`), maps to Gemini's `contents` format
4. `compatible.rs`: Generic OpenAI-compatible with configurable `base_url` (works with OpenRouter, Grok, local models)

### Step 8.3: Tool system

1. In `crates/rustic-agent/src/tools/mod.rs`:
   - `ToolDef { name, description, parameters: serde_json::Value }` (JSON Schema)
   - `ToolExecutor` trait: `async fn execute(&self, name, params, context: &TaskContext) -> Result<ToolOutput>`
2. Built-in tools:
   - `file_ops.rs`: `read_file`, `write_file`, `create_file`, `list_directory`, `search_files` — respects permissions, creates checkpoints before writes
   - `terminal.rs`: `run_command { command, cwd }` — executes in PTY, returns output, requires permission check
   - `search.rs`: `grep_search { query, path, include, exclude }` — content search wrapper

### Step 8.4: Task executor

1. `Task` struct: `id, project_id, title, messages, status: TaskStatus, created_at, provider_type, model`
   - `TaskStatus`: `Running, Completed, Failed, Cancelled`
2. `TaskExecutor::run(task, provider, tools, context)`:
   1. Send messages to provider with tool definitions
   2. If response contains tool use: execute tool, append result, loop back
   3. If text only: task turn complete, await user input
   4. Stream chunks via channel for real-time UI
3. `TaskContext { project_root, permissions, checkpoint_manager }`
4. In `crates/rustic-agent/src/task/permissions.rs`:
   - `Permissions { global: PermissionLevel, project_overrides: HashMap<ProjectId, PermissionLevel> }`
   - `PermissionLevel`: `Admin` (bypass all), `ReadWrite` (read + write + commands with confirmation), `ReadOnly` (read only)
   - `check(action, project_id) -> Allowed | Denied | NeedsConfirmation(String)`

### Step 8.5: Tauri commands for agent

1. In `src-tauri/src/commands/agent.rs`:
   - `create_task(project_id, title) -> Result<TaskInfo>`
   - `send_message(task_id, message) -> Result<()>` — adds user message, kicks off executor
   - `cancel_task(task_id)`, `delete_task(task_id, remove_changes: bool)`
   - `list_tasks(project_id: Option<String>)`, `get_task_messages(task_id)`
   - `set_permissions(project_id: Option<String>, level)`
   - `get_ai_config()`, `set_ai_provider(provider_type, api_key, model, base_url)`
2. Events:
   - `agent-stream`: `{ task_id, chunk }` — real-time text
   - `agent-tool-use`: `{ task_id, tool_name, tool_input }` — tool call start
   - `agent-tool-result`: `{ task_id, tool_use_id, output }` — tool call complete
   - `agent-task-status`: `{ task_id, status }` — status change
   - `agent-permission-request`: `{ task_id, action, description }` — confirmation needed

### Step 8.6: Frontend agent UI

1. Create `src/state/agent.js`:
   - `tasks`: Map of taskId → `{ id, projectId, title, status, messages, isStreaming }`
   - `activeTaskId`: which task is shown in secondary sidebar
   - Listen to all agent events, update state reactively
2. Create `src/components/agent/agent-panel.js` (primary sidebar):
   - Grouped by project (collapsible sections)
   - Each project header: permission badge (icon, click to change), "New Task" button
   - Task list per project
3. Create `src/components/agent/task-list.js`:
   - Each task: title, status indicator (spinner/check/X), click → open secondary sidebar
   - Delete button with confirmation: "Remove changes?" or "Keep changes?"
4. Create `src/components/agent/chat-view.js` (secondary sidebar):
   - Scrollable message list
   - User messages: distinct styling
   - Assistant messages: supports basic markdown (bold, italic, code blocks)
   - Tool calls: inline `tool-call-result` components
   - Input bar at bottom: textarea + send button
   - "Stop" button while streaming
5. Create `src/components/agent/tool-call-result.js`:
   - Collapsible card: tool name, input params, output
   - For file writes: show mini diff
   - Collapsed by default after completion
6. Create `src/components/agent/permission-badge.js`:
   - Shield icon with color: green (Admin), blue (ReadWrite), yellow (ReadOnly)
   - Click to cycle permission level
7. Permission dialog:
   - Listen for `agent-permission-request` events
   - Modal: "Agent wants to [action]. Allow?" → Allow / Deny / Always Allow

**Deliverable:** Full AI agent system with multi-provider, tool use, parallel tasks, streaming chat, permissions.

---

## Phase 9: MCP Integration

**Goal:** MCP client support for external tool servers.

**Depends on:** Phase 8

### Step 9.1: MCP client in `rustic-agent`

1. In `crates/rustic-agent/src/mcp/config.rs`:
   - `McpServerConfig { id, name, transport: McpTransport, enabled }`
   - `McpTransport` enum: `Stdio { command, args, env }` | `Sse { url, headers }`
2. In `crates/rustic-agent/src/mcp/client.rs`:
   - MCP client protocol (JSON-RPC 2.0 over stdio or SSE)
   - `McpClient::connect(config) -> Result<Self>`
   - `McpClient::list_tools() -> Result<Vec<ToolDef>>`
   - `McpClient::call_tool(name, arguments) -> Result<Value>`
   - `McpClient::disconnect()`
   - Connection lifecycle: reconnect on failure, timeouts
3. `McpManager`: manages multiple MCP connections
   - On task creation, aggregate MCP tools + built-in tools

### Step 9.2: Tauri commands for MCP

1. Extend `src-tauri/src/commands/agent.rs`:
   - `add_mcp_server(config)`, `remove_mcp_server(id)`, `list_mcp_servers()`, `test_mcp_server(id) -> Vec<ToolDef>`

### Step 9.3: Frontend MCP config

1. Create `src/components/agent/mcp-config.js`:
   - Sub-section in Agent sidebar
   - List of servers with connection status
   - "Add Server" form: name, transport type, command/URL, args, env
   - "Test Connection" button

**Deliverable:** MCP client integrating external tools into the agent.

---

## Phase 10: Shadow Git / Checkpoint System

**Goal:** Automatic file snapshots before AI edits with per-message rollback.

**Depends on:** Phase 8, Phase 12

### Step 10.1: Checkpoint manager

1. In `crates/rustic-agent/src/checkpoint/mod.rs`:
   - `CheckpointManager` struct using `rustic-db`
   - `Checkpoint { id, task_id, message_index, timestamp, file_snapshots: Vec<FileSnapshotId> }`
   - `FileSnapshot { id, file_path, content: Vec<u8>, was_new: bool }`
2. In `crates/rustic-agent/src/checkpoint/snapshot.rs`:
   - `create_checkpoint(task_id, message_index) -> CheckpointId`
   - `snapshot_file(checkpoint_id, file_path)` — reads current content, stores in SQLite
   - `revert_to(checkpoint_id)` — restores all files. Files that `was_new` get deleted.
   - `list_checkpoints(task_id)`, `delete_task_checkpoints(task_id)`
3. Integration: before any `write_file`/`create_file` tool, call `snapshot_file`. At start of each user message processing, create checkpoint.

### Step 10.2: Tauri commands

1. In `src-tauri/src/commands/checkpoint.rs`:
   - `list_checkpoints(task_id)`, `revert_to_checkpoint(checkpoint_id)`, `preview_checkpoint(checkpoint_id) -> Vec<FileChange>`

### Step 10.3: Frontend checkpoint UI

1. In `chat-view.js`: each assistant message with file changes shows a "Checkpoint" marker with "Revert to here" button
2. Clicking shows confirmation listing files that will be reverted
3. Task deletion: "Keep changes" or "Revert all changes" option

**Deliverable:** Automatic checkpoint system with per-message rollback.

---

## Phase 11: Settings Panel

**Goal:** Full settings UI for themes, fonts, keybindings, AI providers, accounts.

**Depends on:** Phase 8 (AI config), Phase 12 (SQLite)

### Step 11.1: Settings infrastructure

1. In `crates/rustic-core/src/config/settings.rs`:
   ```rust
   pub struct UserSettings {
       pub general: GeneralSettings,
       pub editor: EditorSettings,
       pub theme: ThemeSettings,
       pub keybindings: Vec<Keybinding>,
       pub ai: AiSettings,
   }
   ```
   - Loaded from SQLite on startup with defaults for missing values
2. In `crates/rustic-core/src/config/theme.rs`:
   - `Theme` struct with all color slots
   - Built-in: `gruvbox_dark()`, `gruvbox_light()`
   - `Theme::from_toml(content)` and `Theme::from_json(content)` for custom themes
3. In `crates/rustic-core/src/config/keymap.rs`:
   - `Keybinding { key, command, when: Option<String> }` — VS Code JSON compatible
   - `KeybindingSet::from_vscode_json(json)` — import from VS Code keybindings.json
   - Default keybindings matching VS Code

### Step 11.2: Tauri commands

1. In `src-tauri/src/commands/settings.rs`:
   - `get_settings()`, `update_settings(settings)`
   - `import_theme(path)`, `import_keybindings(path)`
   - `set_api_key(provider_type, api_key)` (stores in OS keyring)
   - `delete_api_key(provider_type)`, `check_api_key(provider_type) -> bool`
   - `github_oauth_start() -> auth_url`, `github_oauth_callback(code) -> AccountInfo`

### Step 11.3: Frontend settings

1. Create `src/components/settings/settings-panel.js`:
   - Full-page view (replaces editor area, like VS Code)
   - Left: category list (General, Editor, Appearance, Keybindings, AI Providers, Accounts)
   - Right: settings form for selected category
   - Search bar to filter settings
2. `general-settings.js`: font family (text input + Google Font URL or custom upload), font size, UI scale
3. `ai-settings.js`: per-provider API key (password field, checkmark if exists), model dropdown, base URL for compatible, temperature slider, "Test Connection" button
4. `theme-editor.js`: theme selector dropdown, live preview, "Import Theme" button (TOML/JSON)
5. `account-settings.js`: GitHub "Connect" button → OAuth flow → show account info

### Step 11.4: Theme application

1. In `src/lib/theme.js`:
   - `applyTheme(theme)` — sets CSS custom properties on `document.documentElement`
   - Called on startup and on theme change
   - Also updates xterm.js theme

**Deliverable:** Complete settings panel with themes, fonts, keybindings, AI config, GitHub OAuth.

---

## Phase 12: SQLite Database Integration

**Goal:** Persistent storage for all app state. Build early, integrate throughout.

**Depends on:** None — start alongside Phase 1

**NOTE:** Build this alongside Phase 1. Other phases use it incrementally.

### Step 12.1: Database setup

1. In `crates/rustic-db/src/connection.rs`:
   - `Database::new(path) -> Result<Self>` — opens/creates at app data directory
   - `rusqlite` with `bundled` feature
   - `run_migrations()` — applies pending migrations
   - WAL mode: `PRAGMA journal_mode=WAL;`
   - Foreign keys: `PRAGMA foreign_keys=ON;`

### Step 12.2: Migrations

1. `001_initial.sql`:
   ```sql
   CREATE TABLE IF NOT EXISTS projects (
       id TEXT PRIMARY KEY,
       name TEXT NOT NULL,
       root_path TEXT NOT NULL UNIQUE,
       created_at TEXT NOT NULL DEFAULT (datetime('now')),
       settings_json TEXT
   );
   CREATE TABLE IF NOT EXISTS user_settings (
       key TEXT PRIMARY KEY,
       value_json TEXT NOT NULL,
       updated_at TEXT NOT NULL DEFAULT (datetime('now'))
   );
   ```

2. `002_agent_tasks.sql`:
   ```sql
   CREATE TABLE IF NOT EXISTS tasks (
       id TEXT PRIMARY KEY,
       project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
       title TEXT NOT NULL,
       status TEXT NOT NULL DEFAULT 'created',
       provider_type TEXT NOT NULL,
       model TEXT NOT NULL,
       created_at TEXT NOT NULL DEFAULT (datetime('now')),
       updated_at TEXT NOT NULL DEFAULT (datetime('now'))
   );
   CREATE TABLE IF NOT EXISTS messages (
       id TEXT PRIMARY KEY,
       task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
       role TEXT NOT NULL,
       content_json TEXT NOT NULL,
       created_at TEXT NOT NULL DEFAULT (datetime('now')),
       sort_order INTEGER NOT NULL
   );
   CREATE TABLE IF NOT EXISTS mcp_servers (
       id TEXT PRIMARY KEY,
       name TEXT NOT NULL,
       transport_type TEXT NOT NULL,
       config_json TEXT NOT NULL,
       enabled INTEGER NOT NULL DEFAULT 1,
       created_at TEXT NOT NULL DEFAULT (datetime('now'))
   );
   ```

3. `003_checkpoints.sql`:
   ```sql
   CREATE TABLE IF NOT EXISTS checkpoints (
       id TEXT PRIMARY KEY,
       task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
       message_index INTEGER NOT NULL,
       created_at TEXT NOT NULL DEFAULT (datetime('now'))
   );
   CREATE TABLE IF NOT EXISTS file_snapshots (
       id TEXT PRIMARY KEY,
       checkpoint_id TEXT NOT NULL REFERENCES checkpoints(id) ON DELETE CASCADE,
       file_path TEXT NOT NULL,
       content BLOB NOT NULL,
       was_new INTEGER NOT NULL DEFAULT 0
   );
   CREATE INDEX IF NOT EXISTS idx_snapshots_checkpoint ON file_snapshots(checkpoint_id);
   CREATE INDEX IF NOT EXISTS idx_checkpoints_task ON checkpoints(task_id);
   CREATE INDEX IF NOT EXISTS idx_messages_task ON messages(task_id, sort_order);
   ```

### Step 12.3: Repository layer

1. `project_repo.rs`: CRUD for `projects` — `insert`, `get`, `list`, `delete`, `update_settings`
2. `task_repo.rs`: CRUD for `tasks` and `messages` — `insert_task`, `list_tasks_for_project`, `update_status`, `delete_task`, `insert_message`, `get_messages_for_task`
3. `checkpoint_repo.rs`: CRUD for `checkpoints` and `file_snapshots`
4. `settings_repo.rs`: Key-value — `set_setting(key, json)`, `get_setting(key)`, `get_all()`
5. `mcp_repo.rs`: CRUD for `mcp_servers`

### Step 12.4: Integration points

- **Phase 2**: Persist projects (restore on restart)
- **Phase 8**: Persist task history and conversations
- **Phase 9**: Persist MCP server configs
- **Phase 10**: Store file snapshots
- **Phase 11**: Persist user settings

**Deliverable:** SQLite database layer integrated across all features.

---

## Phase 13: LSP Client

**Goal:** Language Server Protocol for autocomplete, diagnostics, hover, go-to-definition, auto-format.

**Depends on:** Phase 3 (editor core)

### Step 13.1: LSP client infrastructure

1. Add to `rustic-core` or create `crates/rustic-lsp/`:
   - Use `lsp-types` crate for protocol types
   - `LspClient`: manages JSON-RPC communication with a language server process over stdio
   - `LspClient::start(command, args, root_uri) -> Result<Self>`
   - `LspClient::initialize(capabilities) -> Result<InitializeResult>`
   - `LspClient::send_request<R: Request>(params) -> Result<R::Result>`
   - `LspClient::send_notification<N: Notification>(params)`
   - Incoming notification handler (diagnostics, progress) via channel
2. `LspManager`: one client per language per project
   - Auto-detect language server based on file type
   - Server configs stored in settings

### Step 13.2: LSP features

1. **Text sync**: `didOpen`, `didChange` (incremental), `didSave`, `didClose`
2. **Autocomplete**: triggered on `.`, `::`, `->`, etc. or Ctrl+Space. Completion popup near cursor.
3. **Diagnostics**: incoming `publishDiagnostics` → underline errors (red), warnings (yellow). Gutter icons.
4. **Hover**: mouse hover (500ms delay) → tooltip with docs/type info
5. **Go to definition**: Ctrl+Click or F12 → opens target file
6. **Auto-format on save**: `textDocument/formatting` request before saving

### Step 13.3: Tauri commands

1. Extend editor commands:
   - `get_completions(buffer_id, line, col)`, `get_hover(buffer_id, line, col)`, `goto_definition(buffer_id, line, col)`, `format_document(buffer_id)`
2. Events: `diagnostics-updated`, `lsp-progress`

**Deliverable:** LSP with autocomplete, diagnostics, hover, go-to-def, auto-format.

---

## Phase 14: Polish, Packaging, Logo/Branding

**Goal:** Final polish, menus, context menus, shortcuts, packaging, branding.

**Depends on:** All previous phases

### Step 14.1: Dropdown menus and shortcuts

1. Top bar dropdown menus:
   - **File**: New File, Open File, Add Folder, Remove Folder, Save, Save All, Settings, Exit
   - **Edit**: Undo, Redo, Cut, Copy, Paste, Find, Find in Files
   - **View**: Toggle Sidebar, Toggle Panel, Toggle Secondary Sidebar, Command Palette
   - **Agent**: New Task, View Tasks, Configure Providers, MCP Servers
   - **Help**: About, Keyboard Shortcuts, Documentation
2. Context menus:
   - File tree: New File, New Folder, Rename, Delete, Copy Path, Reveal in File Manager
   - Editor tab: Close, Close Others, Close All, Close to Right, Copy Path
   - Editor area: Cut, Copy, Paste, Go to Definition, Find References
3. Wire all keyboard shortcuts via keymap system

### Step 14.2: Command Palette

1. Create `src/components/command-palette.js`:
   - `Ctrl+Shift+P`: modal with search input, lists all commands
   - `Ctrl+P`: quick file open — fuzzy search file names across all projects

### Step 14.3: Status bar

1. Create `src/components/status-bar.js`:
   - Left: current branch, error/warning count
   - Right: cursor position (Ln, Col), language, encoding, line ending, indentation
   - Clickable segments

### Step 14.4: Logo and app packaging

1. Create `rustic_icon.svg` — Gruvbox-themed geometric mark
2. Generate app icons: `cargo tauri icon rustic_icon.svg`
3. Configure `tauri.conf.json` for production (identifier, version, NSIS installer)
4. Production build: `npm run tauri build`

### Step 14.5: Visual polish

1. CSS transitions: sidebar open/close, tab switching
2. Loading states: skeleton loaders for file tree, search
3. Empty states: meaningful messages
4. File icons: ~20 SVG icons based on extension
5. Tooltips for icon buttons
6. Drag-and-drop: reorder tabs

**Deliverable:** Polished, packaged application.

---

## Dependency Graph

```
Phase 1 (Shell UI)
  ├── Phase 2 (Explorer) ──────────────────────┐
  │     └── Phase 6 (Search)                    │
  │     └── Phase 7 (Source Control)            │
  ├── Phase 3 (Editor Core) ───────────────────┤
  │     └── Phase 13 (LSP)                      │
  ├── Phase 4 (Tabs) ← Phase 2 + Phase 3       │
  ├── Phase 5 (Terminal)                        │
  │                                             │
  Phase 12 (SQLite) ← start alongside Phase 1  │
  │                                             │
  Phase 8 (Agent) ← Phase 3 + Phase 5 + 12 ───┤
  │     └── Phase 9 (MCP) ← Phase 8            │
  │     └── Phase 10 (Checkpoints) ← 8 + 12    │
  │                                             │
  Phase 11 (Settings) ← Phase 8 + Phase 12     │
  │                                             │
  Phase 14 (Polish) ← ALL ─────────────────────┘
```

### Recommended Build Order

1. **Phase 1 + Phase 12** (scaffold + database — in parallel)
2. **Phase 2** (explorer — needs project model)
3. **Phase 3** (editor core — can overlap with Phase 2)
4. **Phase 4** (tabs — needs 2 + 3)
5. **Phase 5** (terminal — can start during Phase 3/4)
6. **Phase 6** (search)
7. **Phase 7** (source control)
8. **Phase 8** (agent — biggest phase)
9. **Phase 10** (checkpoints — immediately after agent)
10. **Phase 9** (MCP — extends agent)
11. **Phase 11** (settings)
12. **Phase 13** (LSP — independent of agent)
13. **Phase 14** (polish — last)

---

## Critical Files

These are the most architecturally important files in the project:

| File | Why |
|------|-----|
| `Cargo.toml` (root) | Workspace definition — all crate members and shared deps |
| `src-tauri/src/state.rs` | Central `AppState` — holds DB, workspace, buffers, terminals, agent. The backbone. |
| `crates/rustic-core/src/buffer/rope.rs` | Core text buffer — every editor operation flows through this |
| `crates/rustic-agent/src/task/executor.rs` | Agentic loop (send to AI → execute tools → repeat) — the key differentiator |
| `src/main.js` | Frontend entry point — builds entire UI layout |
| `src/state/store.js` | Reactive store — all UI state management flows through this |
| `src/components/editor/virtual-scroll.js` | Virtual scrolling — what makes large files performant |
| `crates/rustic-db/src/connection.rs` | Database initialization — all persistence depends on this |
