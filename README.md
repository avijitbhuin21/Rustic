# Rustic

**An agentic, multi-project code editor.** Rustic pairs a fast, VS Code–style editing
surface with a first-class AI agent that can read, edit, run, and reason about your
whole workspace — with every change captured as a revertible per-message snapshot.

Built with **Rust + Tauri 2** on the backend and **React 19 + Vite** on the frontend.
The Rust workspace is **pure Rust — zero C dependencies** (`gix` for git, `rusqlite`'s
bundled SQLite, `portable-pty` for terminals).

---

## Highlights

- **Built-in AI agent** — multi-provider (Anthropic Claude, OpenAI, Google Gemini,
  any OpenAI-compatible endpoint, and OpenRouter with auto-registered models),
  with tool use (file read/edit, shell, web fetch/search, media generation),
  **MCP** server support, and **sub-agent orchestration** for parallel work.
- **Skills & workflows** — reusable instruction bundles and saved multi-step
  playbooks, installable from GitHub repos and invocable inline from the chat via `/`.
- **`@` mentions** — type `@` in the chat to reference a project **file** (passed by
  path so the agent reads it on demand) or attach a live **terminal** (matched by
  name or pid).
- **Per-message checkpoints** — each turn snapshots the whole project; one click
  reverts files (and the chat) to any prior point. Backed by a `gix` shadow repo
  with a debounced filesystem watcher.
- **Monaco editor** with multi-tab editing, previews (Markdown, images, PDF, docx,
  xlsx), and LSP-backed features.
- **Integrated terminals** — `ConPTY`/`portable-pty` backed, with grid and split
  layouts and persistent scrollback across remounts.
- **Multi-project workspace**, file explorer (`react-arborist`), global search,
  and git integration.
- **Secrets in the OS keychain** — API keys never reach the webview.

## Quick start

Prerequisites: [Rust](https://rustup.rs) (stable) and [Bun](https://bun.sh).

> This project uses **Bun** for all JS tooling — use `bun` / `bunx`, not npm/yarn.

```bash
bun install
bun run tauri dev
```

Production build — pass `--no-default-features` to strip the bundled devtools
inspector (installers land under `target/release/bundle/`):

```bash
bun run tauri build -- --no-default-features   # production: devtools stripped
bun run tauri build                            # devtools-enabled "debug-style" build
```

State-mutating git operations (commit, merge, rebase, clone) shell out to the
`git` CLI at runtime, so a `git` binary on `PATH` is required for those.

## Project layout

```
crates/
  rustic-core/        Editor primitives: rope buffer, search, formatter, LSP, workspace
  rustic-treesitter/  Tree-sitter grammars + incremental highlighting
  rustic-db/          SQLite (rusqlite) storage: tasks, checkpoints, settings, projects (WAL + transactional migrations)
  rustic-git/         Pure-Rust git via gix: status, diff, log, shadow repo for checkpoints
  rustic-terminal/    PTY backend (ConPTY on Windows / portable-pty elsewhere)
  rustic-agent/       AI agent: providers, MCP, tools, skills, workflows, sub-agents, checkpoints
src-tauri/            Tauri 2 shell: command handlers, app lifecycle, IPC
src/                  React 19 frontend (Vite + Tailwind 4 + Radix UI)
  components/         editor (Monaco), agent chat, explorer, terminal (xterm.js), settings, onboarding
  state/              Zustand stores: agent, editor, workspace, terminal, settings, models, …
  lib/                Tauri-API wrappers, commands, clipboard, markdown, utils
  styles/             Tailwind layers + global CSS
docs/                 Design decisions, perf findings, educated guesses
audit-report/         Standing performance-audit tracking
```

## Architecture notes

- **Frontend** — React 19 + Vite, styled with Tailwind CSS 4 and Radix UI
  primitives. State is plain **Zustand** stores (one per domain). The editor is
  **Monaco**; terminals are **xterm.js** (webgl + fit + search + web-links addons),
  with persistent instances that keep scrollback across layout/remount changes.
- **Pure-Rust workspace** — no C toolchain required to build. Git is **`gix`**;
  SQLite is `rusqlite`'s bundled build; terminals use `portable-pty`/ConPTY.
- **AI agent** — provider abstraction in `rustic-agent::provider`
  (Claude / OpenAI / Gemini / OpenAI-compatible). OpenRouter models auto-register
  with cost/context metadata. Tools run behind a permission model (Chat /
  Edit / Auto). MCP servers are supported. Long-running commands run in
  background PTY terminals the agent can poll.
- **Checkpoints** — every user message opens a whole-project snapshot in a `gix`
  shadow repo; reverting restores files from the snapshot mirror and truncates the
  chat to that point.
- **Secrets** — API keys live in the OS keychain (`keyring` crate). The webview
  only ever sees the sentinel `"__STORED__"`; the backend resolves the real key.
- **Safe file writes** — every save goes through `atomic_write` (temp + fsync +
  rename) so a crash mid-write can't corrupt source files.
- **Path scope** — agent tools refuse paths resolving outside the project root
  after canonicalization; filesystem commands refuse system directories
  (`src-tauri/src/path_scope.rs`).

## Security & privacy

- **CSP** is set in `src-tauri/tauri.conf.json`. LLM/MCP markdown output is
  sanitized before it is rendered.
- **SSRF guard** on every redirect hop in `web_fetch` — rejects private,
  loopback, link-local, CGNAT, and IPv4-mapped addresses.
- **`git_clone`** accepts only `https://` and SCP-style `user@host:path`, and
  the target directory must live inside `$HOME`.
- API keys are never persisted in the database or exposed to the frontend.

## Building & distribution

`bun run tauri build` produces locally-installable bundles (on Windows: an NSIS
`*_x64-setup.exe` and an MSI). A few production concerns are intentionally **not**
configured yet:

- **Code signing** — builds are unsigned, so Windows SmartScreen warns on install.
  Enabling it needs a code-signing certificate and `bundle.windows` config.
- **Auto-updater** — not wired up. Enabling it needs `tauri-plugin-updater`, a
  signing keypair, and a hosted `latest.json` manifest.
- **Cross-platform** — primary development and CI are on Windows; macOS/Linux
  paths (default shell selection, line endings, file watching) are largely
  untested.

## License

Personal project — no public license selected yet.
