# Rustic

A VS Code-inspired agentic IDE with first-class multi-project workspace support and a built-in AI agent system. Built with Rust + Tauri 2 on the backend and vanilla JS on the frontend.

## Quick start

Prerequisites: [Rust](https://rustup.rs) (stable), [Bun](https://bun.sh).

```bash
bun install
bun run tauri dev
```

Production build:

```bash
bun run tauri build           # debug-style binary, includes devtools
bun run tauri build -- --no-default-features   # production binary, devtools stripped
```

## Layout

```
crates/
  rustic-core/       # Editor primitives: rope buffer, tree-sitter highlighter, search, formatter, LSP, workspace
  rustic-db/         # SQLite (rusqlite) storage: tasks, checkpoints, settings, projects. WAL + transactional migrations
  rustic-git/        # libgit2 wrapper: status, diff, log, conflict resolve, remote/clone
  rustic-terminal/   # portable-pty backend
  rustic-agent/      # AI agent: providers (Claude/OpenAI/Gemini/OpenAI-compatible), MCP, tools, checkpoints
src-tauri/           # Tauri 2 shell, command handlers, app lifecycle, IPC
src/                 # Vanilla JS frontend (no framework, no TypeScript)
  components/        # UI components (editor, agent panel, explorer, terminal, settings, ...)
  lib/               # markdown helper, debug logger, theme, keybindings, Tauri-API wrappers
  state/             # Custom reactive store: editor, agent, workspace, git, settings, ...
  styles/            # CSS (one file per area)
  utils/             # DOM helper, format helpers
references/          # Read-only reference dumps (claude-code-structure, roo-code-structure). Gitignored.
```

## Architecture decisions

- **Frontend stack**: vanilla JS + ES modules, no React/Svelte, no TypeScript. Custom `el()` DOM helper and a tiny pub/sub store (`src/state/store.js`).
- **Editor engine**: ropey rope buffer + tree-sitter parser, virtual scrolling. Tree-sitter parser is persisted per buffer and reparsed incrementally on each edit — no full reparse per keystroke.
- **Database**: SQLite via rusqlite. WAL mode + `synchronous=NORMAL`. Migrations are transactional (each in its own `BEGIN/COMMIT`) and the on-disk DB is backed up to `app.db.bak.<timestamp>` before any new migration runs.
- **AI agent**: provider abstraction in `rustic-agent::provider` (Claude / OpenAI / Gemini / OpenAI-compatible). MCP server support. Per-message **whole-project snapshot** checkpoint; revert restores from the snapshot mirror.
- **Secrets**: API keys live in the OS keychain (`keyring` crate). The webview never sees raw keys — it gets the sentinel `"__STORED__"` and the backend resolves it.
- **File writes**: every save goes through `atomic_write` (sibling temp + fsync + rename) so a crash mid-write cannot corrupt source files.
- **Path scope**: agent tools refuse paths that resolve outside the project root after canonicalization. Tauri commands that mutate the filesystem refuse paths inside system directories (Windows / Unix lists in `src-tauri/src/path_scope.rs`).

## Security & privacy

- **CSP** set in `src-tauri/tauri.conf.json` (`default-src 'self'`). LLM/MCP markdown output is sanitized via DOMPurify before reaching `innerHTML` — see `src/lib/markdown.js`.
- **SSRF** check on every redirect hop in `web_fetch`; rejects private IP literals, loopback, link-local, CGNAT, and IPv4-mapped variants.
- **`git_clone`**: only `https://` and SCP-style `user@host:path`. `target_dir` must be inside `$HOME`.
- **Devtools**: enabled by default for `cargo run` / `bun run tauri dev`. Production builds use `--no-default-features` to strip the inspector.

## Status

The base IDE (14 phases — shell UI, explorer, editor, tabs, terminal, search, git, agent, MCP, checkpoints, settings, SQLite, LSP, polish) is complete. Agent enhancements through phase 9 are in. See `git log` for the recent commit trail.

Test plan after a fresh `bun install`:

1. `bun run tauri dev` boots without errors.
2. Open a file, edit, save — atomic save, no corruption.
3. Modify a file externally, save in Rustic — overwrite-or-cancel prompt fires.
4. Close the window with unsaved changes — per-file save prompt fires before exit.
5. AI provider settings — re-enter your key once (legacy SQLite keys auto-migrate to keychain on first launch). Settings panel shows `Configured` instead of the raw key.
6. Send a message → revert button — restores the project to the pre-message snapshot.
7. Launch a second instance — focuses the existing window; any path argument is forwarded.

## Release / distribution gaps

The build pipeline ships locally-installable bundles, but there are several
production-shipping concerns the codebase does NOT yet address. These need
your input (signing keys, hosting choices, hardware) before they can be
turned on.

### Code signing

- **Windows**: `src-tauri/tauri.conf.json` has no `signingIdentity` /
  `certificateThumbprint`. Unsigned binaries trigger Windows SmartScreen on
  every install. To enable: obtain a code-signing cert (DigiCert, Sectigo,
  etc.; ~$300/year), put the thumbprint into `bundle.windows.signCommand`,
  and run `cargo tauri build` from a machine with the cert in its cert
  store.
- **macOS**: Apple Developer Program membership ($99/year) + a Developer ID
  Application certificate. Set `bundle.macOS.signingIdentity` and (for
  Gatekeeper) `bundle.macOS.providerShortName`. Notarization step also
  required (see Tauri docs).
- **Linux**: AppImages don't strictly need signing but `.deb` / `.rpm`
  packaging benefits from a GPG key.

### Auto-updater

Not configured. To enable:

1. Add `tauri-plugin-updater = "2"` to `src-tauri/Cargo.toml`.
2. Generate a signing keypair: `cargo tauri signer generate`. Keep the
   private key out of source control; copy the public key into the config.
3. Add an `updater` block to `tauri.conf.json` with:
   - `pubkey`: the generated public key
   - `endpoints`: list of URLs hosting the latest update manifest
     (`latest.json`).
4. Host the latest binaries + a `latest.json` (Tauri's manifest schema)
   somewhere (S3, GitHub releases, your own CDN).
5. Wire the plugin in `src-tauri/src/lib.rs` and call
   `app.updater().check()` from a menu item / on launch.

The CSP in `tauri.conf.json` already allows `connect-src https:` so updater
fetches will work without further changes.

### Cross-platform

The codebase is developed on Windows 10 and CI runs the Rust suite on
`windows-latest` only. Likely-broken paths on macOS / Linux until tested:

- Terminal shell defaults (`portable-pty` works everywhere but the default
  shell selection is Windows-flavored).
- Path handling — `to_string_lossy()` is everywhere; non-UTF-8 paths on
  Linux will lose information.
- Line-ending handling — on save, the rope's bytes go to disk verbatim. A
  Windows-edited file then opened on Linux keeps CRLF, which is fine for
  most tools but breaks some (shell scripts).
- File watcher — `notify` is cross-platform but FSEvents quirks on macOS,
  inotify limits on Linux. Untested.

To validate: run on a Mac and a Linux box, fix the inevitable backslash /
shell / line-ending bugs that surface, then add a `macos-latest` and
`ubuntu-latest` runner to `.github/workflows/ci.yml`.

## License

Personal project — no public license selected yet.
