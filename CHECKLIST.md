# Rustic work checklist

Open work only. Completed items are removed (git history keeps the record). Mark `[x]` when done, then delete the line in the next cleanup.

## Needs real-hardware verification

Logic for these is unit-tested, cross-compiled for `x86_64-pc-windows-msvc` and covered by `rustic-server/tests/peer_e2e.rs` (two local servers); only the physical-device parts are left.

- [ ] Windows: agent terminals in Git Bash (detection, prompt stripping, completion)
- [ ] Two devices: peer sync over Wi-Fi switch, Cloudflare path, pairing pop-up bringing the window forward
- [ ] Windows: a finished Git Bash command no longer counts as running (launcher → `usr\bin\bash.exe` resolution in `crates/rustic-terminal/src/pty.rs`)

## v0.8.6 follow-ups (tracker #13 / #14 fixes)

- [ ] Verify in a built app (unit-tested only so far): `read_terminal_output` on an agent terminal that idle-closed returns its last output with the closed note; `check_other_active_agents` no longer lists the caller's edit-tool files under a peer that is running commands
- [ ] File history: writes a backgrounded command makes after the 25 s hand-off are still not credited to the task. Holding the window open until the command ends would also credit the user's own edits while a dev server runs (revert would undo them), so it needs a design that credits only commands that end on their own

## v0.8.6 follow-ups (tracker #15 / #16 fixes, paste bug)

- [ ] Close tracker #15 and #16 via `website/issue-actions.json` + deploy once the user says so (fixed in source, left open on purpose)
- [ ] Verify in a built app: `grep_search` with `include: "{a.jsx,b.jsx}"` and `exclude: "{vendor,references}/**"`; `list_themes` / `read_theme` after `tool_search`; Cmd+V of a screenshot into the composer after a `.pptx` tab has been opened

## v0.8.6 follow-ups (tracker #17 fix)

- [ ] Close tracker #17 via `website/issue-actions.json` + deploy once the user says so (fixed in source, left open on purpose)
- [ ] Verify in a built app: run `echo repro17-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456` in an agent terminal, then `read_terminal_output` (default and `rendered: true`) shows no wrapper text


## v0.8.6 follow-ups (Nov 2026 audit fixes, tracker #18 to #21)

Everything below was unit-tested or checked in headless Chromium only. Nothing has run in a built desktop app.

Verify in a built desktop app (WKWebView / WebView2):
- [ ] App ACL: every feature still works from the main window (the permission is generated from `generate_handler!`: 331 commands; the 23 frontend command names not in it are server-only commands that were never registered on desktop), and `window.__TAURI_INTERNALS__.invoke('list_projects')` from a remote-backend window is rejected
- [ ] Main-window navigation guard: film view, live HTML preview and the browser devtools pane still load; a link in rendered markdown cannot replace the app
- [ ] `rustic-media://`: images, video, fonts and the PDF/DOCX range fetches still load in the main window; a sandboxed preview frame and a remote-backend window are refused
- [ ] Chat: remote images in agent output show the click-to-load placeholder; a long turn (100+ tool calls) scrolls without jumping; a streaming reply looks the same as before
- [ ] Terminals: after a long session `lsof -p <pid> | wc -l` stays flat and `ps -o stat= -p ...` shows no zombies; the log line `open-file limit before=256 after=10240` appears at startup
- [ ] MCP: a non-read-only tool prompts in Ask/Auto-edit, is refused in Chat, runs unasked in Full auto; "always allow" covers that one tool only

Audit items not done:
- [ ] Task histories are never evicted from memory (`AgentTask.messages`). About 45 call sites read it and an empty vec means "new task" in places, so eviction needs a design, not a patch
- [ ] Incremental persist still serialises every message on each call; only the locking was fixed. Skipping unchanged rows would weaken the "newer run re-persists everything" self-healing
- [ ] Monaco is created and disposed on every tab switch (`<ActiveView key={active.id}>`). Keeping one editor and swapping models is a rewrite of `monaco-editor.jsx`
- [ ] A new reqwest client per provider instance, so no connection reuse across turns. Not shared: two code paths build their own current-thread runtimes, and a pooled connection must not outlive the runtime that made it
- [ ] Every image in history is read from disk and base64-encoded on each provider request (the resize itself is cached)
- [ ] Each terminal's monitor polls every 100 ms. Left as is: command-completion latency for PowerShell and user terminals depends on it
- [ ] Path-mode preview (`/proxy/<port>/`) still runs the previewed page on the IDE's origin. The session cookie is no longer forwarded and is HttpOnly, but the page can still call the IDE's API as the logged-in user; only subdomain mode isolates it
- [ ] Pairing: the 6-digit code is now bound to the request token, which stops a third party's request from matching. An active man-in-the-middle on a URL (unpinned) address can still search for a colliding token
- [ ] Verify in a built desktop app (checked in headless Chromium only): mermaid diagrams in chat and in `.md` / `.mmd` previews look as before; a model-written diagram that carries a tag with a URL, a markdown image, an `img:` shape or CSS `url()` stays a code fence; `.svg` preview unchanged
- [ ] Verify in a built app (store-level check only): during a `/goal` loop, and after a blocked goal, sending a message clears the goal capsule and the loop stops at the end of the current turn; a new `/goal` starts it again
- [ ] Verify in a built app (headless component check only): quit with an `ask_user` question open, reopen the chat, the question form is back and answering it sends the answers as a message; an expanded `ask_user` row shows markdown, not JSON
- [ ] Verify in a built app (DB test only): quit with a `/goal` blocked and with one mid-loop; on reopen the capsule shows the same turn count, the blocked one still shows blocked with its reason, and nothing runs until a message, an answer or a new `/goal` is sent

Release:
- [ ] Close tracker #18 to #21 via `website/issue-actions.json` + deploy once the user says so
- [ ] `docker-compose.yml` now sets `RUSTIC_TRUSTED_PROXIES` for the bundled Caddy. Anyone publishing the server port directly must remove it