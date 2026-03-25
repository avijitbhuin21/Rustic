# Rustic — Implementation Progress

## Completed

### Phase 1: Project Scaffold and Shell UI

**Status:** Complete

#### Step 1.1: Initialize project
- Created `package.json` with `@tauri-apps/api` v2, `xterm`, `@xterm/addon-fit`, `vite`, `@tauri-apps/cli`
- Created `vite.config.js` with root set to `src/`, port 1420, esnext target
- Created `Cargo.toml` workspace with all 5 crates + `src-tauri`
- Created `src-tauri/Cargo.toml` with Tauri v2 (`devtools` feature), all crate deps
- Created `src-tauri/tauri.conf.json` — decorations OFF, 1280x800, identifier `com.rustic.editor`
- Created `src-tauri/src/main.rs` and `src-tauri/src/lib.rs` (Tauri bootstrap)
- Created `src-tauri/build.rs` (tauri-build)
- Created `src-tauri/capabilities/default.json` with `core:default`
- Created stub crates: `rustic-core` (with submodule dirs: buffer, syntax, workspace, search, config), `rustic-db`, `rustic-agent`, `rustic-git`, `rustic-terminal`
- Generated app icons from `rsutic_icon.svg` using `npx tauri icon`
- `npm install` and `cargo build` both succeed

#### Step 1.2: CSS theming foundation
- `src/styles/global.css` — CSS reset, font variables, scrollbar styling, base element styles
- `src/styles/theme.css` — Gruvbox Dark (default) + Gruvbox Light (`[data-theme="light"]`), all background/foreground/accent/bright/token variables

#### Step 1.3: Reactive state store
- `src/state/store.js` — `createStore(initialState)` with `getState`, `setState`, `subscribe(key, cb)` pub/sub pattern
- `src/state/ui.js` — UI state: `activePanel`, sidebar/panel visibility + widths/heights

#### Step 1.4: DOM utility helpers
- `src/utils/dom.js` — `el()` for creating DOM elements, `mount()` for mounting, `icon()` and `iconMulti()` for inline SVG icons

#### Step 1.5: Main layout shell
- `src/styles/layout.css` — CSS Grid layout (topbar, activity, sidebar, editor, secondary, panel), resize handle styles, status bar
- `src/styles/activity-bar.css` — Activity bar item styles with active state
- `src/styles/sidebar.css` — Sidebar header and content styles
- `src/styles/editor.css` — Editor placeholder styles
- `src/styles/terminal.css` — Bottom panel header, tabs, actions, content
- `src/styles/tabs.css` — Tab bar base styles
- `src/index.html` — Entry point loading all CSS + main.js
- `src/main.js` — App initialization, mounts all components, syncs CSS variables
- `src/components/top-bar.js` — Logo, menu buttons (File/Edit/View/Agent/Help), toggle buttons (sidebar/panel/secondary), window controls (minimize/maximize/close via Tauri API), drag region
- `src/components/activity-bar.js` — Explorer, Search, Source Control, Agent icons (top), Settings (bottom); click toggles activePanel + sidebar visibility
- `src/components/primary-sidebar.js` — Header with panel title, content area with placeholder, reacts to activePanel + visibility changes
- `src/components/editor-area.js` — Placeholder "Open a file to start editing"
- `src/components/secondary-sidebar.js` — "Agent Chat" header, hidden by default, toggles via state
- `src/components/bottom-panel.js` — Terminal tab, minimize button, placeholder content, toggles via state
- `src/components/status-bar.js` — Fixed bottom bar showing version, encoding, line ending

#### Step 1.6: Resizable panels
- Drag-to-resize handles on primary sidebar (vertical) and bottom panel (horizontal)
- Updates CSS custom properties (`--sidebar-width`, `--panel-height`, `--secondary-width`) in real-time
- Min/max constraints applied (sidebar: 160-600px, panel: 100px-appHeight-200px)

### Phase 2: File Explorer (Multi-Project)

**Status:** Complete

#### Step 2.1: Workspace model in `rustic-core`
- `crates/rustic-core/src/workspace/project.rs` — `Project` struct with `id` (UUID), `name`, `root_path`, `is_expanded`; `Serialize`/`Deserialize` for Tauri IPC
- `crates/rustic-core/src/workspace/file_tree.rs` — `FileNode` struct with `path`, `name`, `is_dir`, `children`, `depth`; `read_directory()` using `ignore` crate's `WalkBuilder` for `.gitignore`-aware reading; sorted directories-first then case-insensitive alphabetical
- `crates/rustic-core/src/workspace/mod.rs` — `Workspace` struct with `add_project()`, `remove_project()`, `list_projects()`; duplicate detection by path
- Added deps: `uuid` v1 (v4+serde), `ignore` 0.4, `anyhow` 1

#### Step 2.2: Tauri commands for workspace
- `src-tauri/src/state.rs` — `AppState` with `Mutex<Workspace>`, registered as managed state
- `src-tauri/src/commands/workspace.rs` — `add_project`, `remove_project`, `list_projects` commands
- `src-tauri/src/commands/file_tree.rs` — `read_dir` (lazy directory listing), `read_file_content` (UTF-8 read)
- `src-tauri/src/lib.rs` — Wired all commands into `invoke_handler`, added `tauri-plugin-dialog`
- `src-tauri/capabilities/default.json` — Added `dialog:default`, `dialog:allow-open` permissions
- Added deps: `tauri-plugin-dialog` v2, `@tauri-apps/plugin-dialog` (npm)

#### Step 2.3: Frontend file explorer
- `src/lib/tauri-api.js` — Typed wrappers around `invoke()` with graceful fallback outside Tauri
- `src/state/workspace.js` — Workspace state store with `addProject()`, `removeProject()`, `toggleProject()`, `loadChildren()`, `refreshProject()`, `initWorkspace()`; in-memory children cache
- `src/components/explorer/explorer.js` — Explorer panel with "Add Project" button, renders project sections, empty state with CTA
- `src/components/explorer/project-section.js` — Collapsible project header with name, caret, hover action buttons (Refresh, Remove)
- `src/components/explorer/file-tree.js` — Renders file/folder items for a directory, triggers lazy load
- `src/components/explorer/file-tree-item.js` — Individual item with indentation (`depth * 16px`), folder expand/collapse, file click emits `rustic:open-file` event, extension-based icon coloring
- `src/styles/explorer.css` — Full explorer styling (project sections, file tree items, hover states, actions)
- Updated `src/components/primary-sidebar.js` — Now renders real explorer component when panel is 'explorer'
- Updated `src/main.js` — Calls `initWorkspace()` on startup

#### Step 2.4: File tree performance
- Lazy loading: only 1 level deep loaded on expand via `read_directory(path, depth)` with `max_depth(1)`
- In-memory cache (`childrenCache` Map) prevents re-fetching already-loaded directories
- `refreshProject()` clears cache subtree and reloads
- FS watching deferred to later (will use `notify` crate)

### Phase 3: Editor Core

**Status:** Complete

#### Step 3.1: Rope buffer in `rustic-core`
- `crates/rustic-core/src/buffer/rope.rs` — `Buffer` struct with `ropey::Rope`, `BufferId` (atomic u64), file path, language detection, modified state
- `Buffer::from_file()`, `Buffer::from_string()`, `Buffer::info()` for IPC
- `Buffer::apply_edit()` with 300ms edit grouping for undo chunking
- `Buffer::undo()` / `Buffer::redo()` — pops edit groups, applies inverse edits, manages undo/redo stacks
- `Buffer::save()` — writes rope to disk, clears modified flag
- Line access: `line_count()`, `get_line()`, `get_lines()`, `byte_offset_of_line()`, `line_of_byte()`
- Language detection from file extension (14 languages supported)
- `crates/rustic-core/src/buffer/edit.rs` — `Edit` struct (byte_offset, old_text, new_text), `EditGroup`, `Edit::inverse()`
- Added deps: `ropey` 1

#### Step 3.2: Tree-sitter syntax highlighting
- `crates/rustic-core/src/syntax/languages.rs` — `LanguageRegistry` with `get_language()` and `get_highlight_query()` for 13 languages (Rust, JS, TS/TSX, Python, Go, C, C++, Java, JSON, TOML, HTML, CSS, Markdown)
- All languages behind Cargo feature flags (default: all enabled)
- `crates/rustic-core/src/syntax/highlight.rs` — `SyntaxHighlighter` using `tree_sitter_highlight::Highlighter`
- `SyntaxHighlighter::highlight_lines(rope, start, end)` returns `Vec<RenderedLine>` with `Span { start_col, end_col, highlight_class }`
- 24 highlight names mapped to 9 simplified token classes: keyword, string, comment, function, type, variable, number, operator, punctuation
- Added deps: `tree-sitter` 0.24, `tree-sitter-highlight` 0.24, 13 grammar crates

#### Step 3.3: Tauri commands for editor
- `src-tauri/src/commands/editor.rs` — 7 commands:
  - `open_file(path)` — creates Buffer + SyntaxHighlighter, returns BufferInfo; deduplicates by path
  - `get_visible_lines(buffer_id, start, end)` — returns highlighted RenderedLine objects
  - `edit_buffer(buffer_id, line, col, new_text, delete_count)` — applies edit, returns line_count + is_modified
  - `save_file(buffer_id)` — writes to disk
  - `undo_edit(buffer_id)` / `redo_edit(buffer_id)` — undo/redo
  - `close_buffer(buffer_id)` — removes buffer + highlighter
- Updated `AppState` with `buffers: Mutex<HashMap<BufferId, Buffer>>`, `highlighters: Mutex<HashMap<BufferId, SyntaxHighlighter>>`
- `src/lib/tauri-api.js` — Added `openFile`, `getVisibleLines`, `editBuffer`, `saveFile`, `undoEdit`, `redoEdit`, `closeBuffer` wrappers

#### Step 3.4: Frontend editor rendering
- `src/state/editor.js` — Editor state store: `openBuffers`, `activeBufferId`, `cursorLine`, `cursorCol`, `scrollTop`; per-buffer view state save/restore on tab switch
- `src/components/editor/editor-pane.js` — Main editor component with:
  - Virtual scrolling: spacer div for total height, visible line range with 10-line overscan, throttled scroll handler
  - Gutter (line numbers) synced with visible lines
  - Fetches lines from backend via `get_visible_lines` on scroll/buffer change
- `src/components/editor/line-renderer.js` — Renders line text with `<span>` per syntax span, CSS class per token type
- `src/components/editor/gutter-renderer.js` — Line numbers, active line highlighted
- `src/styles/editor.css` — Full editor styling: pane layout, gutter, lines, cursor blink animation, token color classes
- Updated `src/components/editor-area.js` — Shows editor pane when buffer active, placeholder when empty

#### Step 3.5: Basic text input
- Hidden `<textarea>` for keyboard input capture (IME-compatible with compositionstart/compositionend)
- Text input: `input` event → `edit_buffer` → cursor advance
- Key handling: Enter (new line), Backspace (delete char/join lines), Delete, Tab (4 spaces)
- Arrow keys: Up/Down/Left/Right with line wrapping, Home/End
- Ctrl+S: save, Ctrl+Z: undo, Ctrl+Y/Ctrl+Shift+Z: redo
- Click-to-place cursor (calculates line/col from mouse position)
- Cursor: blinking 2px accent-colored bar, positioned absolutely
- Auto-scroll to keep cursor in viewport
- `src/utils/debounce.js` — `debounce()` and `throttleRAF()` utilities
- File open wired from explorer: `rustic:open-file` custom event → `openFile()`

### Phase 4: Tabs and Multi-File Editing

**Status:** Complete

#### Step 4.1: Tab bar and tab components
- `src/components/editor/tab-bar.js` — Horizontally scrollable row of tabs from `openBuffers`, re-renders on buffer/active changes, Shift+wheel horizontal scroll
- `src/components/editor/tab.js` — Single tab: `[ProjectName] filename.ext` display (project name dimmed), modified dot indicator, close button (hover/active), click to activate, middle-click to close
- `src/styles/tabs.css` — Full tab styling: active tab with accent bottom border, hover states, modified dot, close button visibility transitions

#### Step 4.2: Wire explorer to editor
- Updated `file-tree-item.js` — `rustic:open-file` event now includes `projectName`
- Updated `file-tree.js` — Threads `projectName` from project section to file tree items
- Updated `project-section.js` — Passes `project.name` to `createFileTree()`
- Updated `main.js` — Reads `projectName` from event detail and passes to `openFile(path, projectName)`
- Updated `editor-area.js` — Renders tab bar above editor pane in `.editor-container` wrapper; shows placeholder when no buffer active

#### Step 4.3: Buffer management
- Per-tab cursor/scroll state already implemented via `bufferViewState` Map in `editor.js` (save on switch, restore on activate)
- Dirty indicator: `isModified` dot on tab via `updateBufferModified()`
- Keyboard shortcuts added to `editor-pane.js`:
  - `Ctrl+S`: save active buffer (existing)
  - `Ctrl+W`: close active tab
  - `Ctrl+Tab` / `Ctrl+Shift+Tab`: cycle through open tabs
- `src/styles/editor.css` — Added `.editor-container` flex column layout

### Phase 5: Terminal Integration

**Status:** Complete

#### Step 5.1: PTY backend in `rustic-terminal`
- `crates/rustic-terminal/Cargo.toml` — Added `portable-pty` 0.8, `anyhow` 1
- `crates/rustic-terminal/src/pty.rs` — `PtySession` struct with atomic `SessionId`, `new(cwd, label, is_agent)` spawns default shell via `portable-pty`, `write()`, `resize()`, `take_reader()` for output streaming thread
- `crates/rustic-terminal/src/shell.rs` — `TerminalManager` with `create_session()` (returns `SessionInfo` + reader), `write_session()`, `resize_session()`, `destroy_session()`, `list_sessions()`
- `crates/rustic-terminal/src/lib.rs` — Re-exports `SessionId`, `SessionInfo`, `TerminalManager`

#### Step 5.2: Tauri commands for terminal
- `src-tauri/src/commands/terminal.rs` — 5 commands:
  - `create_terminal(cwd, label, is_agent)` — creates PTY session, spawns background thread for output streaming via `terminal-output` Tauri event
  - `write_terminal(session_id, data)` — sends input to PTY
  - `resize_terminal(session_id, cols, rows)` — resizes PTY
  - `close_terminal(session_id)` — destroys session
  - `list_terminals()` — lists all sessions
- Updated `src-tauri/src/state.rs` — Added `terminal_manager: Mutex<TerminalManager>` to `AppState`
- Updated `src-tauri/src/commands/mod.rs` — Added `terminal` module
- Updated `src-tauri/src/lib.rs` — Registered all 5 terminal commands
- Updated `src-tauri/capabilities/default.json` — Added `core:event:default` permission for event streaming

#### Step 5.3: Frontend terminal
- `src/state/terminal.js` — Terminal state store: `sessions` array, `activeSessionId`; `createTerminal()`, `closeTerminal()`, `setActiveSession()`; auto-shows bottom panel on create
- `src/components/terminal/terminal-tabs.js` — Tab bar with session tabs (icon, label, close button), "+" button to create new terminal, active tab styling, agent icon for agent terminals
- `src/components/terminal/terminal-pane.js` — xterm.js terminal instances per session:
  - Dynamic xterm.js + CSS loading
  - Gruvbox Dark theme applied to xterm.js
  - Per-session terminal instances (create on demand, hide/show on switch)
  - `onData` → `writeTerminal()` for input, `terminal-output` event → `terminal.write()` for output
  - `ResizeObserver` for auto-fit on container resize
  - `FitAddon` for proper column/row calculation, synced to backend via `resizeTerminal()`
- `src/lib/tauri-api.js` — Added `createTerminal`, `writeTerminal`, `resizeTerminal`, `closeTerminal`, `listTerminals`, `onTerminalOutput` (event listener)

#### Step 5.4: Per-project terminal
- Updated `src/components/explorer/project-section.js` — "New Terminal" action button on project headers, opens terminal at project root with project name as label
- Updated `src/components/bottom-panel.js` — Replaced placeholder with real `terminal-tabs` + `terminal-pane` components
- Updated `src/styles/terminal.css` — Full terminal styling: tabs, pane, xterm instance sizing

### Phase 6: Search

**Status:** Complete

#### Step 6.1: Search backend in `rustic-core`
- `crates/rustic-core/src/search/content_search.rs` — `SearchEngine::search(query)` using `ignore::WalkBuilder` for `.gitignore`-aware file walking + `regex` crate for pattern matching
  - `SearchQuery` with `pattern`, `is_regex`, `case_sensitive`, `whole_word`, `paths`, `include_glob`, `exclude_glob`
  - `SearchResult` with `file_path` and `Vec<SearchMatch>` (line_number, line_text, match_start, match_end)
  - Supports regex/literal, case sensitivity, whole word matching, glob include/exclude filters
  - Skips binary/unreadable files gracefully
- `crates/rustic-core/src/search/file_search.rs` — `find_files(query, paths, max_results)` for filename substring search (Ctrl+P future use)
- Added deps: `regex` 1, `glob` 0.3

#### Step 6.2: Tauri commands for search
- `src-tauri/src/commands/search.rs` — 2 commands:
  - `search_in_project(project_id, pattern, options...)` — searches within a single project's directory
  - `search_global(pattern, options...)` — searches across all workspace projects
- Updated `src-tauri/src/commands/mod.rs` — Added `search` module
- Updated `src-tauri/src/lib.rs` — Registered both search commands

#### Step 6.3: Frontend search panel
- `src/state/search.js` — Search state store: `query`, `results`, `isSearching`, `scope` (global or project ID), `isRegex`, `caseSensitive`, `wholeWord`; debounced `performSearch()`, `setQuery()`, `setScope()`, `toggleOption()`
- `src/components/search/search-panel.js` — Search sidebar view:
  - Text input with focus-on-show, Enter to search
  - Toggle buttons: regex (.*), case (Aa), whole word (ab) with active state
  - Scope selector dropdown: "All Projects" + individual project names from workspace state
- `src/components/search/search-results.js` — Results display:
  - Summary line (N results in M files)
  - Results grouped by file, collapsible sections with file icon, path, project prefix, match count badge
  - Each match: line number + text with highlighted match in accent color
  - Click match → opens file in editor
- `src/styles/search.css` — Full search panel styling
- Updated `src/components/primary-sidebar.js` — Wired search panel (replaced placeholder)
- Updated `src/lib/tauri-api.js` — Added `searchInProject`, `searchGlobal` wrappers
- Updated `src/index.html` — Added search.css

### Phase 7: Source Control (Git)

**Status:** Complete

#### Step 7.1: Git backend in `rustic-git`
- `crates/rustic-git/Cargo.toml` — Added `git2` 0.19, `anyhow` 1
- `crates/rustic-git/src/repo.rs` — `GitRepo::open(path)` via `Repository::discover()`, `head_branch()`, `branches()` returning `Vec<BranchInfo>`
- `crates/rustic-git/src/status.rs` — `GitRepo::status()` returning `GitStatus { branch, files }` with `FileStatus { path, status: StatusType, is_staged }`; `stage()`, `unstage()`, `commit()`, `discard_changes()` operations; handles index (staged) and worktree (unstaged) changes for all status types (New, Modified, Deleted, Renamed, Untracked, Conflicted)
- `crates/rustic-git/src/diff.rs` — `GitRepo::diff_file(path)` for unstaged diff, `diff_staged()` for staged diff; returns `FileDiff { file_path, hunks, additions, deletions }` with `DiffHunk { header, lines }` and `DiffLine { origin, content, old_lineno, new_lineno }`; uses `diff.print(Patch)` for borrow-safe parsing

#### Step 7.2: Tauri commands for git
- `src-tauri/src/commands/git.rs` — 8 commands:
  - `git_status(project_id)` — returns branch name + file statuses
  - `git_stage(project_id, paths)` — stages files to index
  - `git_unstage(project_id, paths)` — unstages files via `reset_default`
  - `git_commit(project_id, message)` — commits staged changes, returns commit OID
  - `git_discard(project_id, paths)` — discards working tree changes via `checkout_head`
  - `git_diff(project_id, path)` — unstaged diff for specific file
  - `git_diff_staged(project_id)` — diff of all staged changes
  - `git_branches(project_id)` — lists local and remote branches
- Helper `get_project_path()` resolves project ID to root path from workspace
- Updated `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs` — Registered all 8 git commands

#### Step 7.3: Frontend source control panel
- `src/state/git.js` — Git state store: `projectStatuses` map (projectId → GitStatus); `refreshGitStatus()`, `refreshAllGitStatuses()`, `stageFiles()`, `unstageFiles()`, `commitChanges()`, `discardChanges()`
- `src/components/git/source-control.js` — Source control sidebar: per-project sections with auto-refresh on mount and project changes, refresh button in header
- `src/components/git/project-scm.js` — Per-project SCM section:
  - Header: project name, branch icon + name (colored), file count badge
  - Commit area: input field + commit button (Enter to submit)
  - Staged changes group: file list with unstage button, "Unstage All" bulk action
  - Changes (unstaged) group: file list with stage/discard buttons, "Stage All" bulk action
  - File entries: status badge (A/M/D/R/U/C with color), filename + directory, hover action buttons
- `src/components/git/diff-view.js` — Inline diff view: file header with path + stats (+additions/-deletions), hunk headers, diff lines with red/green highlighting, gutter origin markers, line numbers
- `src/styles/git.css` — Full source control and diff styling
- Updated `src/components/primary-sidebar.js` — Wired source control panel (replaced placeholder)
- Updated `src/lib/tauri-api.js` — Added all 8 git API wrappers
- Updated `src/index.html` — Added git.css

### Phase 8: Agent System

**Status:** Complete

#### Step 8.1: AI provider abstraction
- `crates/rustic-agent/src/provider/mod.rs` — Core types: `Role`, `ContentBlock` (Text/ToolUse/ToolResult), `Message`, `ToolDef`, `AiResponse`, `TokenUsage`, `StopReason`, `ProviderConfig`, `ModelInfo`; `AiProvider` async trait with `chat()`, `name()`, `available_models()`
- `crates/rustic-agent/src/config.rs` — `AiConfig`, `ProviderEntry`, `ProviderType` enum (Claude/OpenAi/Gemini/Compatible)
- Added deps: `async-trait`, `reqwest` (json+stream), `tokio` (full), `uuid`, `futures`, `regex`, `ignore`, `glob`

#### Step 8.2: Provider implementations
- `crates/rustic-agent/src/provider/claude.rs` — `ClaudeProvider`: Anthropic Messages API, system message extraction, tool use format, response conversion, 3 model definitions (Sonnet/Opus/Haiku 4)
- `crates/rustic-agent/src/provider/openai.rs` — `OpenAiProvider`: OpenAI Chat Completions API, tool_calls format mapping, content block conversion, 3 model definitions (GPT-4o/4o-mini/o3-mini)
- `crates/rustic-agent/src/provider/compatible.rs` — `CompatibleProvider`: delegates to OpenAI provider with custom base_url (works with OpenRouter, Grok, local models)

#### Step 8.3: Tool system
- `crates/rustic-agent/src/tools/mod.rs` — `ToolOutput`, `ToolContext` (project_root + permissions), `ToolExecutor` trait, `BuiltinTools` combining all built-in tools
- `crates/rustic-agent/src/tools/file_ops.rs` — 4 tools: `read_file`, `write_file`, `create_file`, `list_directory`; all respect permission levels, create parent dirs as needed
- `crates/rustic-agent/src/tools/terminal.rs` — `run_command` tool: runs shell commands via `std::process::Command`, platform-aware (cmd on Windows, sh on Unix)
- `crates/rustic-agent/src/tools/search.rs` — `grep_search` tool: regex search with ignore-aware walking, include/exclude glob filters, max 100 results

#### Step 8.4: Task executor & permissions
- `crates/rustic-agent/src/task/permissions.rs` — `PermissionLevel` (Admin/ReadWrite/ReadOnly), `Action` (Read/Write/Execute)
- `crates/rustic-agent/src/task/executor.rs` — `TaskExecutor::run_turn()`: agentic loop that sends messages to provider, executes tool calls, appends results, loops until text-only response; emits `TaskEvent`s (TextDelta, ToolUse, ToolResult, StatusChange, MessageComplete) via channel
- `crates/rustic-agent/src/task/mod.rs` — `TaskStatus` enum, `TaskInfo` struct

#### Step 8.5: Tauri commands for agent
- `src-tauri/src/commands/agent.rs` — 8 commands:
  - `create_task(project_id, title)` — creates task with auto-detected provider/model
  - `send_message(task_id, message)` — adds user message, spawns async agentic loop in background thread, forwards events to frontend via Tauri events (agent-stream, agent-tool-use, agent-tool-result, agent-task-status)
  - `list_tasks(project_id)`, `get_task_messages(task_id)`, `delete_task(task_id)`
  - `set_ai_provider(provider_type, api_key, model, base_url)` — configures AI provider
  - `get_ai_config()` — returns current config
  - `set_permissions(project_id, level)` — sets per-project permission level
- `src-tauri/src/state.rs` — Added `AgentState` with tasks, ai_config, project_permissions; added to `AppState`

#### Step 8.6: Frontend agent UI
- `src/state/agent.js` — Agent state store: tasks map, activeTaskId; event listeners for stream/tool-use/tool-result/status; `createTask()`, `sendMessage()`, `setActiveTask()`, `deleteTaskAction()`
- `src/components/agent/agent-panel.js` — Primary sidebar panel: per-project sections with "New Task" button, task list with status indicators (spinner/check/X), click to open chat, delete button
- `src/components/agent/chat-view.js` — Secondary sidebar chat: scrollable message list, user/assistant/tool message styling, tool use cards with input JSON, tool result display, basic markdown rendering (code blocks, inline code, bold), text input with Enter-to-send, auto-scroll
- Updated `src/components/secondary-sidebar.js` — Now renders real chat view
- Updated `src/components/primary-sidebar.js` — Wired agent panel
- `src/styles/agent.css` — Full agent panel + chat styling: task list, spinner animation, chat messages, tool cards, code blocks, input area
- Updated `src/lib/tauri-api.js` — Added 8 agent API wrappers + 4 event listeners
- Updated `src/index.html` — Added agent.css

### Phase 9: MCP Integration

**Status:** Complete

#### Step 9.1: MCP client in `rustic-agent`
- `crates/rustic-agent/src/mcp/config.rs` — `McpServerConfig { id, name, transport, enabled }`, `McpTransport` enum: `Stdio { command, args, env }` | `Sse { url, headers }`
- `crates/rustic-agent/src/mcp/client.rs` — `McpClient`: JSON-RPC 2.0 over stdio transport; `connect()` spawns child process, sends `initialize` + `notifications/initialized`; `list_tools()` → `Vec<ToolDef>`, `call_tool(name, arguments)` → `Value`; `disconnect()` kills child; implements `Drop` for cleanup
- `crates/rustic-agent/src/mcp/mod.rs` — `McpManager`: manages multiple MCP connections; `add_server()`, `remove_server()`, `list_servers()`, `test_server()` (connect + list tools + disconnect), `connect_all()`, `all_tools()`, `call_tool()`, `disconnect_all()`

#### Step 9.2: Tauri commands for MCP
- Extended `src-tauri/src/commands/agent.rs` — 4 new commands:
  - `add_mcp_server(name, transport_type, command, args, url)` — creates server config with UUID
  - `remove_mcp_server(id)` — removes and disconnects
  - `list_mcp_servers()` — returns all configs
  - `test_mcp_server(id)` — connects, lists tools, disconnects; returns `Vec<ToolDef>`
- Updated `src-tauri/src/state.rs` — Added `McpManager` to `AgentState`

#### Step 9.3: Frontend MCP config UI
- `src/components/agent/mcp-config.js` — MCP configuration sub-section in agent panel:
  - Server list with name, transport info, test/remove action buttons
  - "Add Server" form: name, transport type dropdown (Stdio/SSE), command+args or URL fields (toggle based on transport), save/cancel buttons
  - "Test Connection" button shows tool count on success, error on failure
- Updated `src/components/agent/agent-panel.js` — Includes MCP config section at bottom
- Updated `src/styles/agent.css` — Full MCP config styling (server list, add form, hover actions)
- Updated `src/lib/tauri-api.js` — Added `addMcpServer`, `removeMcpServer`, `listMcpServers`, `testMcpServer` wrappers

### Phase 12: SQLite Database Integration

**Status:** Complete

#### Step 12.1: Database setup
- `crates/rustic-db/Cargo.toml` — Added `rusqlite` (bundled), `anyhow`, `uuid`, `chrono`, `serde_json`
- `crates/rustic-db/src/connection.rs` — `Database::new(path)` opens/creates SQLite DB, enables WAL mode + foreign keys, runs pending migrations via `_migrations` tracking table; `Database::in_memory()` for testing

#### Step 12.2: SQL migrations
- `crates/rustic-db/src/migrations/001_initial.sql` — `projects` table (id, name, root_path, settings_json) + `user_settings` key-value table
- `crates/rustic-db/src/migrations/002_agent_tasks.sql` — `tasks`, `messages`, `mcp_servers` tables with foreign keys and cascading deletes
- `crates/rustic-db/src/migrations/003_checkpoints.sql` — `checkpoints`, `file_snapshots` tables with indexes on checkpoint_id, task_id, message sort_order

#### Step 12.3: Repository layer
- `crates/rustic-db/src/models.rs` — Row types: `ProjectRow`, `TaskRow`, `MessageRow`, `McpServerRow`, `CheckpointRow`, `FileSnapshotRow`, `SettingRow`
- `crates/rustic-db/src/project_repo.rs` — `insert_project`, `get_project`, `get_project_by_path`, `list_projects`, `delete_project`, `update_project_settings`
- `crates/rustic-db/src/task_repo.rs` — `insert_task`, `get_task`, `list_tasks_for_project`, `update_task_status`, `delete_task`, `insert_message`, `get_messages_for_task`, `get_next_sort_order`
- `crates/rustic-db/src/checkpoint_repo.rs` — `insert_checkpoint`, `list_checkpoints`, `get_checkpoint`, `delete_task_checkpoints`, `insert_file_snapshot`, `get_file_snapshots`, `get_snapshots_for_task_up_to`
- `crates/rustic-db/src/settings_repo.rs` — `set_setting`, `get_setting`, `get_all_settings`, `delete_setting`
- `crates/rustic-db/src/mcp_repo.rs` — `insert_mcp_server`, `list_mcp_servers`, `get_mcp_server`, `update_mcp_server_enabled`, `delete_mcp_server`

#### Step 12.4: Integration into Tauri app
- Updated `src-tauri/src/state.rs` — Added `db: Mutex<Database>` to `AppState`, `new()` takes `Database` parameter
- Updated `src-tauri/src/lib.rs` — `setup` hook resolves app data directory, creates `Database::new(app_data_dir/rustic.db)`, passes to `AppState::new(db)`

### Phase 10: Shadow Git / Checkpoint System

**Status:** Complete

#### Step 10.1: Checkpoint manager in `rustic-agent`
- `crates/rustic-agent/src/checkpoint/mod.rs` — `CheckpointInfo` (id, task_id, message_index, created_at, file_count) and `FileChange` (file_path, change_type) types
- `crates/rustic-agent/src/checkpoint/snapshot.rs` — Core checkpoint operations:
  - `create_checkpoint(db, task_id, message_index)` — creates a checkpoint row in SQLite
  - `snapshot_file(db, checkpoint_id, file_path)` — reads current file content, stores in SQLite; marks non-existent files as `was_new` so revert will delete them
  - `revert_to(db, checkpoint_id)` — restores all snapshotted files; deletes files that were newly created
  - `list_checkpoints(db, task_id)` — lists checkpoints with file counts
  - `preview_checkpoint(db, checkpoint_id)` — returns list of file changes without applying
  - `delete_task_checkpoints(db, task_id)` — cleanup
- Added `rustic-db` and `chrono` as dependencies to `rustic-agent`

#### Step 10.2: Tauri commands for checkpoints
- `src-tauri/src/commands/checkpoint.rs` — 3 commands:
  - `list_checkpoints(task_id)` — returns `Vec<CheckpointInfo>`
  - `revert_to_checkpoint(checkpoint_id)` — reverts files, returns `Vec<FileChange>`
  - `preview_checkpoint(checkpoint_id)` — returns `Vec<FileChange>` (dry run)
- Registered commands in `src-tauri/src/lib.rs`

#### Step 10.3: Checkpoint integration into tool execution
- Added `SnapshotFn` callback type to `ToolContext` in `crates/rustic-agent/src/tools/mod.rs`
- Updated `file_ops.rs` — `write_file` and `create_file` tools call `snapshot_fn` before modifying disk
- Updated `src-tauri/src/commands/agent.rs` `send_message`:
  - Creates a checkpoint at the start of each user message processing
  - Builds a `SnapshotFn` closure capturing the DB (via `Arc<Mutex<Database>>`) and checkpoint ID
  - Passes `snapshot_fn` into `ToolContext` for the background executor thread
- Updated `src-tauri/src/state.rs` — Changed `db` field to `Arc<Mutex<Database>>` for safe sharing with background threads

#### Step 10.4: Frontend checkpoint UI
- Updated `src/components/agent/chat-view.js`:
  - Loads checkpoints via `listCheckpoints(taskId)` when rendering
  - Detects assistant messages containing `write_file`/`create_file` tool uses
  - Renders checkpoint marker with purple accent border, file count, and "Revert" button
  - Revert flow: calls `previewCheckpoint` first, shows `confirm()` dialog listing file changes, then calls `revertToCheckpoint`
- Updated `src/styles/agent.css` — Checkpoint marker styles (purple theme, hover transition)
- Updated `src/lib/tauri-api.js` — Added `listCheckpoints`, `revertToCheckpoint`, `previewCheckpoint` wrappers

### Phase 11: Settings Panel

**Status:** Complete

#### Step 11.1: Settings infrastructure in `rustic-core`
- `crates/rustic-core/src/config/settings.rs` — `UserSettings` (general, editor, theme, keybindings, ai) with `GeneralSettings` (font_family, font_size, ui_scale, auto_save), `EditorSettings` (tab_size, insert_spaces, word_wrap, line_numbers, cursor_blink/style, render_whitespace), `ThemeSettings` (active_theme, custom_themes), `AiSettings` (default_provider, max_tokens, temperature); all with serde + defaults
- `crates/rustic-core/src/config/theme.rs` — `Theme` struct with 30+ color slots (bg/fg/accent/bright/token colors); `gruvbox_dark()`, `gruvbox_light()` built-ins; `from_toml()`, `from_json()` for custom theme import; `builtin()`, `builtin_names()`
- `crates/rustic-core/src/config/keymap.rs` — `Keybinding { key, command, when }` (VS Code JSON compatible); `KeybindingSet` with `from_vscode_json()` import and `defaults()` (16 common shortcuts)
- Added `toml` 0.8 dependency to `rustic-core`

#### Step 11.2: Tauri commands for settings
- `src-tauri/src/commands/settings.rs` — 6 commands:
  - `get_settings()` — loads from SQLite `user_settings` key, returns defaults if missing
  - `update_settings(settings)` — saves full `UserSettings` to SQLite
  - `get_active_theme()` — resolves active theme (built-in or custom from DB)
  - `list_themes()` — returns built-in + custom theme names
  - `import_theme(path)` — reads TOML/JSON file, stores in DB, adds to custom themes list
  - `import_keybindings(path)` — reads VS Code JSON, merges into settings

#### Step 11.3: Frontend settings panel
- `src/state/settings.js` — Settings state store: `settings`, `themes`, `activeCategory`, `isOpen`; `loadSettings()`, `saveSettings()`, `updateSetting(path, value)`, `openSettings()`, `closeSettings()`
- `src/components/settings/settings-panel.js` — Full-page settings view (replaces editor area): left category sidebar (General, Editor, Appearance, Keybindings, AI Providers) with icons + right content area; close button
- `src/components/settings/general-settings.js` — Font family, font size, UI scale, auto save toggle + delay
- `src/components/settings/editor-settings.js` — Tab size, insert spaces, word wrap, line numbers, cursor blink/style, render whitespace
- `src/components/settings/theme-settings.js` — Theme selector dropdown (built-in + custom), "Import Theme" button with file dialog
- `src/components/settings/ai-settings.js` — Default provider selector, temperature slider, max tokens; per-provider sections (Claude/OpenAI/Gemini/Compatible) with API key, model, base URL, "Save Provider" button
- `src/components/settings/keybindings-settings.js` — Import from VS Code button, keybindings table (key, command, when)
- `src/styles/settings.css` — Full settings panel styling: category sidebar, setting rows, toggle switches, slider, inputs/selects, provider cards, keybindings table
- Updated `src/components/editor-area.js` — Shows settings panel when `isOpen`, hides editor/placeholder
- Updated `src/components/activity-bar.js` — Settings button toggles settings panel (not sidebar)
- Updated `src/index.html` — Added settings.css
- Updated `src/lib/tauri-api.js` — Added 6 settings API wrappers

#### Step 11.4: Theme application
- `src/lib/theme.js` — `applyTheme(theme)` sets CSS custom properties on document root + dispatches `rustic:theme-changed` event; `getXtermTheme()` returns xterm.js-compatible theme object; `getCurrentTheme()`
- Updated `src/main.js` — Loads and applies saved theme on startup via `getActiveTheme()`

### Phase 13: LSP Client

**Status:** Complete

#### Step 13.1: LSP client infrastructure
- `crates/rustic-core/src/lsp/transport.rs` — `StdioTransport`: JSON-RPC 2.0 over stdio; `start(command, args)` spawns child process; `send_request(method, params)` with Content-Length framing; `send_notification()`; `read_message()` parses headers + JSON body; `kill()` + `Drop` cleanup
- `crates/rustic-core/src/lsp/client.rs` — `LspClient`: wraps transport with typed LSP operations; `start(command, args, root_uri, language_id)` auto-initializes with capabilities; text sync (`did_open/change/save/close`), `completion()`, `hover()`, `goto_definition()`, `format()`; simplified helpers (`hover_string`, `completion_simple`, `goto_definition_simple`, `format_simple`) to avoid leaking `lsp_types` across crate boundaries; `shutdown()` + `Drop` cleanup
- `crates/rustic-core/src/lsp/manager.rs` — `LspManager`: one client per (project_root, language_id); `get_or_start()` auto-detects server from file extension; `stop()`/`stop_project()`/`stop_all()`; `LspServerConfig` with 8 default server configs (rust-analyzer, typescript-language-server, pylsp, gopls, clangd, vscode-json/css/html); `path_to_uri()`/`uri_to_path()` conversion helpers
- Added `lsp-types` 0.95 dependency to `rustic-core`

#### Step 13.2: Tauri commands for LSP
- `src-tauri/src/commands/lsp.rs` — 8 commands:
  - `lsp_notify_open(buffer_id)` — auto-starts LSP server for file type, sends `didOpen`
  - `lsp_notify_change(buffer_id, version)` — sends `didChange` (full document sync)
  - `lsp_notify_save(buffer_id)` — sends `didSave`
  - `lsp_notify_close(buffer_id)` — sends `didClose`
  - `get_completions(buffer_id, line, col)` — returns `Vec<CompletionEntry>` (label, kind, detail, insert_text)
  - `get_hover(buffer_id, line, col)` — returns `Option<HoverInfo>` (contents as string)
  - `goto_definition(buffer_id, line, col)` — returns `Vec<LocationInfo>` (file_path, line, col)
  - `format_document(buffer_id)` — returns `Vec<FormatEdit>` (range + new_text)
- Updated `src-tauri/src/state.rs` — Added `lsp_manager: Mutex<LspManager>` to `AppState`

#### Step 13.3: Frontend LSP UI
- `src/components/editor/autocomplete.js` — Autocomplete popup overlay: fetches completions from backend, arrow key navigation, Enter/Tab to accept, Escape to dismiss, kind badge coloring, detail text
- `src/components/editor/hover-tooltip.js` — Hover tooltip: delayed show (500ms) on mouse move, markdown-ish rendering (code blocks, inline code), positioned above cursor, auto-hide on mouse leave
- Updated `src/components/editor/editor-pane.js`:
  - Wired autocomplete popup + hover tooltip as overlay elements
  - Ctrl+Space triggers autocomplete at cursor position
  - F12 / Ctrl+Click triggers goto-definition, opens target file
  - Ctrl+Shift+I triggers format document
  - Mouse hover over code area schedules hover tooltip
  - `lsp_notify_open` on buffer activation, `lsp_notify_change` after edits, `lsp_notify_save` on Ctrl+S
- Updated `src/styles/editor.css` — Autocomplete popup styles (item list, kind badges, selection highlight), hover tooltip styles (fixed position, code blocks, shadow)
- Updated `src/lib/tauri-api.js` — Added 8 LSP API wrappers

### Phase 14: Polish, Packaging, Logo/Branding

**Status:** Complete

#### Step 14.1: Dropdown menus and context menus
- `src/components/dropdown-menu.js` — Generic dropdown menu system: `createDropdownMenu(items)` with label, shortcut, action, separator, disabled support; `showContextMenu(items, x, y)` for right-click context menus; auto-close on outside click
- Updated `src/components/top-bar.js` — Real dropdown menus for all 5 menu buttons:
  - **File**: New File, Open File, Add Folder, Save, Settings, Exit (with keyboard shortcuts)
  - **Edit**: Undo, Redo, Cut, Copy, Paste, Find in Files
  - **View**: Toggle Sidebar/Panel/Secondary Sidebar, Command Palette, Quick Open
  - **Agent**: New Task, View Tasks, Configure Providers, MCP Servers
  - **Help**: Keyboard Shortcuts, About Rustic
- Updated `src/components/explorer/file-tree-item.js` — Right-click context menu: Open File (files only), Copy Path, Copy Name, Reveal in File Manager
- Updated `src/components/editor/tab.js` — Right-click context menu: Close, Close Others, Close to the Right, Close All, Copy Path; drag-and-drop visual feedback (draggable tabs with drop targets)

#### Step 14.2: Command Palette
- `src/components/command-palette.js` — Modal overlay with search input + filtered command list:
  - `Ctrl+Shift+P`: opens command palette (lists all commands)
  - `Ctrl+P`: opens quick file search mode
  - Arrow key navigation, Enter to execute, Escape to dismiss
  - 11 built-in commands (Save, Settings, toggle panels, show views, new terminal, format document)
  - Global keyboard shortcut listener

#### Step 14.3: Enhanced status bar
- Updated `src/components/status-bar.js` — Live-updating segments:
  - Left: branch indicator, error/warning counts
  - Right: cursor position (Ln/Col), language from active buffer, encoding (UTF-8), line ending (LF), indentation (Spaces: 4)
  - Subscribes to `editorStore` for real-time cursor/buffer updates

#### Step 14.4: Visual polish
- `src/styles/polish.css` — Comprehensive polish styles:
  - Dropdown menus: fixed position, rounded corners, shadows, hover accent, shortcut labels, separators
  - Command palette: overlay backdrop, rounded modal, input/list styling, selected highlight
  - Status bar: flex layout, clickable segments, proper spacing
  - CSS transitions on sidebar/panel open/close (0.15s ease)
  - File icon colors by extension (20 extensions: rs, js, ts, py, go, json, toml, md, html, css, etc.)
  - Loading skeleton animation (shimmer gradient)
  - CSS tooltip system via `data-tooltip` attribute
  - Tab drag-and-drop visual states (dragging opacity, drop target border)
- Updated `src/index.html` — Added polish.css

#### Step 14.5: Production packaging
- Updated `src-tauri/tauri.conf.json`:
  - NSIS installer config for Windows
  - `shortDescription`, `longDescription`, `copyright`, `category` metadata
  - Ready for `npm run tauri build` production builds

---

## All Phases Complete

All 14 phases of the Rustic implementation plan have been completed:
- **Phase 1-4**: Shell UI, File Explorer, Editor Core, Tabs
- **Phase 5-7**: Terminal, Search, Source Control (Git)
- **Phase 8-9**: Agent System, MCP Integration
- **Phase 10**: Shadow Git / Checkpoint System
- **Phase 11**: Settings Panel
- **Phase 12**: SQLite Database Integration
- **Phase 13**: LSP Client
- **Phase 14**: Polish, Packaging, Logo/Branding
