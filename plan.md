# Rustic React Migration Plan

Migrating frontend from vanilla JS → React 19 + Vite + shadcn/ui + Monaco. Backend (Tauri 2 + Rust) unchanged.

---

## ✅ Done

- **Cleanup** — all ~140 vanilla JS/CSS files removed; only `assets/`, `index.html`, new React skeleton retained.
- **Foundation** — shadcn/ui (25 components) on Radix + Tailwind 4, path aliases (`@/`), Monaco vite chunk, dark mode default. Auto-installed: radix-ui, tw-animate-css, @fontsource-variable/geist, react-resizable-panels, next-themes, react-diff-view, unidiff, @tauri-apps/plugin-fs, cmdk.
- **Rust changes** — `tauri-plugin-fs` registered (Cargo + lib.rs + capabilities), bumped to 2.5.1.
- **Shell** — `src/components/shell/`: TopBar, ActivityBar, SidebarHost, EditorAreaHost, BottomPanelHost, StatusBar; `useLayout` zustand store; `App.jsx` assembled with `ResizablePanelGroup`.
- **Explorer** — `react-arborist` tree with lazy `read_dir`, project list, add-folder dialog, F2 inline rename, right-click context menu (new file/folder, rename, delete, copy path, reveal, open in terminal).
- **Editor** — Monaco lazy-loaded with bundled TS IntelliSense (no external LSP); tab bar with drag-reorder; routes by file kind → markdown / image / pdf / svg / hex / diff previews; `Ctrl+S` saves via `plugin-fs.writeTextFile`; per-tab cursor restore on reopen.
- **SCM** — Status / branches / log / conflicts / commit form / branch switcher / commit history with expandable per-commit file list. `react-diff-view` + `unidiff` for diffs. Diff view also opens as editor tab (kind: 'diff') from SCM clicks and commit-history clicks.
- **Terminal** — `xterm` + FitAddon + per-project pty tabs.
- **Search** — query/replace with case/word/regex toggles, include/exclude globs, streaming results, click match opens at line.
- **Agent / Chat** — chat view with streaming, tool calls, MCP / Rules / Skills / Workflows tabs, permission/question prompt dialogs, cost indicator, task switcher dropdown (rename/delete via `rename_task` / `delete_task`).
- **Settings** — General / Editor / Appearance / Keys / AI tabs with live save; keybindings via file-picker import; theme picker.
- **Cross-store wiring** — `App.jsx` syncs `useExplorer.activeProjectId` → `useGit` + `useAgent` so SCM and Agent panels follow Explorer.
- **Status bar** — live branch + ahead/behind from `useGit`, dirty file count + cursor pos + language from `useEditor`, conflict counter.
- **Polish** — Confirm dialog system (`confirm({...})` async API mounted globally), Command palette (Ctrl+P files / Ctrl+Shift+P commands), Theme bridge (maps Rust `Theme` struct fields → CSS vars), file watcher → tree refresh (`rustic:fs-change`), Onboarding wizard (first-launch, 4 steps), Shortcut cheatsheet (Ctrl+/).
- **Tauri param-casing audit + 28 fixes** — agent-built panels had invented JS-shaped arg names; all corrected across 12 files (MCP, rules, skills, workflows, AI provider, search, chat, terminal, etc.).
- **Theme bridge shape** — verified Rust `Theme` struct is flat (`bg_hard/bg/bg_soft/bg1..bg4/fg..fg4/accent/border/bright_*`); `theme-bridge.jsx` maps fields → `--bg-*`, `--text-*`, `--accent-*`, `--status-*`, `--syntax-*`.
- **File watcher shape** — verified static: `FsChangeEvent { project_path, changed_dirs }` matches `file-tree.jsx`.
- **Production build verified** — `bun run tauri build --no-bundle` → 7m 39s. `rustic.exe` = **78.13 MB** (target was 75-80 MB; +8 MB / +11.6% vs 70 MB pre-rewrite).
- **Dev build verified** — `bun run build` 8.35s, 2162 modules, no warnings. Lazy chunks: Monaco/DiffEditor/PDF/cmdk all deferred.

### Size budget actual

| | Pre-rewrite | After |
|---|---|---|
| `rustic.exe` | ~70 MB | **78.13 MB** |
| Dev bundle (largest chunks) | n/a | vendor 450 KB · xterm 283 KB · pdf 410 KB · markdown 65 KB · radix 102 KB · diff 53 KB · icons 28 KB · cmdk 14 KB · monaco 11 KB · app 145 KB · CSS 95 KB |

---

## 📋 Remaining

### Live runtime verification (requires `bun tauri dev` to be run)
- [ ] RAM at idle (target: ~30 MB main / ~80-100 MB total)
- [ ] RAM in active session (3 files, no LSPs)
- [ ] Cold start time vs old vanilla build
- [ ] plugin-fs `writeTextFile` actually saves through new Rust binary
- [ ] Exercise each fixed `invoke` flow panel-by-panel for any straggler bugs
- [ ] Verify Monaco web workers load correctly under WebView2 in production binary

### Tier 4 — Deliberately deferred (substantial architectural work)
- [ ] **Editor ↔ Rust rope buffer bridge** — currently saves via `plugin-fs.writeTextFile`. Bridging Monaco edits to `open_file` + `edit_buffer` + `save_file` would give syntax-aware persistence and external-change detection. Non-trivial: Monaco's edit model batches deltas; position semantics differ. Defer until needed.
- [ ] **Multi-pane editor splits** — needs `useEditor` refactored from flat `tabs[]` to tree of pane groups + drag-to-split tab UX. Half-day rewrite.
- [ ] **Explorer DnD** — needs Rust `move_entry` command first (only `rename_entry` + `copy_entry` exist; copy+delete simulation is unsafe for large files).
- [ ] **i18n / log viewer / perf-debug helpers** — port from old `src/lib/` when there's a real need.

### Tier 5 — Optional integration depth
- [ ] AI providers config lookup — `ai-providers-settings.jsx` looks up `config.providers[type]` by both backend `ProviderType` variant AND the UI slug because exact storage key shape wasn't pinned down. Verify against runtime and pick one.
- [ ] Drag-and-drop in Explorer (depends on `move_entry` above)
- [ ] Commit history file-diff already opens an editor tab; if multi-pane lands, can split-view the diff alongside the file
- [ ] Status bar: when no editor tab is active, the cursor cell hides — could show project / task summary instead

### Known gaps / minor follow-ups
- Some commands have `// TODO(param-audit)`-style residual notes only where Rust signatures were ambiguous (e.g. provider key shape). None block compile.
- Onboarding wizard currently has no "back" button between steps — forward-only with skip. Add if it matters.
- Keybindings UI is import-only (no inline editor). The file import path is functional; an editor can come later.

---

## Architectural decisions (locked)

| Area | Choice |
|------|--------|
| Framework | React 19 |
| UI | shadcn/ui on Radix + Tailwind 4 |
| Code editor | Monaco, lazy-loaded, bundled TS only (no external LSP) |
| External LSPs | Off by default, opt-in via settings |
| File tree | react-arborist |
| State | zustand (per-domain stores) |
| Git | Backend `git2` exposed via Tauri commands; UI calls them directly |
| Markdown | `marked` + DOMPurify |
| Diff | `react-diff-view` + `unidiff` for SCM; Monaco `DiffEditor` for code |
| Package manager | Bun (`bun`, `bunx`, `bun add`) — never npm/npx |
| Bundler / dev server | Vite 5 |

## Risks / things to watch

- **Tauri command param contracts** — audit caught 28 bugs but only covers what was written before the audit; any new `invoke` call needs to re-check the Rust signature.
- **Monaco web workers under Tauri WebView2** — production build compiled; runtime worker URL resolution untested.
- **`react-arborist` peer dep is React ≥16.14** with internal react-dnd 14; works against React 19 in dev/build but watch for warnings.
- **AI config storage key shape** — provider lookup currently falls back across two key conventions; confirm against live runtime.
