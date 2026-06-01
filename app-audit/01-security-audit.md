# Rustic — Security Audit

**Date:** 2026-06-01
**Branch:** `rebuilt-ui`
**Scope:** Full repo — Tauri host (`src-tauri`), Rust crates (`crates/*`), React frontend (`src`).
**Method:** Read-only static review. No changes made. No dynamic testing / fuzzing performed.

---

## Executive summary

The application has a **noticeably mature security posture for a desktop IDE/agent.** Many of the classic Tauri-app footguns have already been closed deliberately, with inline comments referencing prior findings (`F-07`, sensitive-file tiers, etc.). Highlights of what is already done *well*:

- **SSRF defense in `web_tools` is genuinely strong** — http→https upgrade, rejection of `localhost` and every private/reserved IP range (RFC1918, loopback, link-local, CGNAT, IPv4-mapped IPv6, **cloud-metadata `169.254.169.254`**), DNS resolution validation, *and* IP-pinning via `reqwest`'s `.resolve()` so the validated address is the one actually connected to. Redirects are followed manually with re-validation on every hop. This closes the DNS-rebinding TOCTOU window that most implementations miss.
- **All model/markdown/SVG output is sanitized with DOMPurify** before `dangerouslySetInnerHTML` (chat, markdown preview, SVG preview). SVG uses the SVG profile; markdown whitelists only GFM checkbox tags.
- **Secrets live in the OS keychain** (`keyring`), never in plaintext config, with graceful fallback.
- **Git credentials** are injected via `http.extraHeader` (not URL-embedded, keeping them out of reflog/stderr), and `credential.helper=` is cleared per-invocation to avoid interactive-helper hangs.
- **State-changing trust operations require a native OS confirmation dialog** that a compromised webview cannot synthesize: saving a GitHub token (`git_set_token`) and repointing a git remote (`git_add_remote`). This is exactly the right mitigation against XSS-driven credential/exfil attacks.
- **Shell spawn is allowlisted** (`validate_shell_program`, F-07) so an XSS cannot launch an arbitrary binary via `create_terminal`.
- **Agent file tools are confined to the project root** (`resolve_with_scope`) with a tiered sensitive-file blocklist (private keys, `.env`, AWS creds, service-account JSON, gitignored files).
- **Git operations use argv arrays**, never a shell string — no command injection.

No **critical** (remotely exploitable, no interaction) vulnerability was found. The findings below are mostly **defense-in-depth** gaps and hardening opportunities. The single most important one is **SEC-01** (the `fs` capability glob).

| ID | Severity | Title |
|----|----------|-------|
| SEC-01 | **High** | `fs` plugin capability is `{"path": "**"}` — frontend can read/write any file, bypassing all Rust path-scoping |
| SEC-02 | Medium | CSP `connect-src`/`img-src`/`media-src` allow wildcard `https:` — broad exfiltration channel if XSS ever lands |
| SEC-03 | Medium | Custom-provider `base_url` is unvalidated — API keys can be sent to an arbitrary host |
| SEC-04 | Low | Terminal write/resize/close commands have no session-ownership check |
| SEC-05 | Low | Windows system-root check uses substring `contains()` — both over- and under-matches possible |
| SEC-06 | Low | `resolve_with_scope` returns the non-canonical joined path after validating the canonical one |
| SEC-07 | Info | GitHub OAuth client ID is committed (expected for a public device-flow client) |
| SEC-08 | Info | Verify request/response bodies carrying API keys are never logged at any tracing level |

---

## Findings

### SEC-01 — `fs` capability glob defeats Rust-side path scoping  **[High]**

`src-tauri/capabilities/default.json`:

```json
{ "identifier": "fs:allow-write-text-file", "allow": [{ "path": "**" }] },
{ "identifier": "fs:allow-read-text-file",  "allow": [{ "path": "**" }] },
{ "identifier": "fs:allow-read-file",       "allow": [{ "path": "**" }] }
```

The frontend calls `@tauri-apps/plugin-fs` (`writeTextFile`, `readTextFile`) **directly** in at least 8 components (`markdown-preview.jsx`, `svg-preview.jsx`, `html-preview.jsx`, `xlsx-preview.jsx`, `monaco-editor.jsx`, `prompt-box.jsx`, `appearance-settings.jsx`, `state/settings.js`). Because the capability grants `**`, these calls reach **any path on the filesystem** and **completely bypass** the carefully written `src-tauri/src/path_scope.rs` guards (`validate_writable_path` / `validate_readable_path`) — those guards only protect the *custom* `#[tauri::command]` handlers, not the fs plugin.

**Impact:** The strong sensitive-path blocklist (`~/.ssh`, browser profiles, `~/.aws`, system roots) is *only enforced for custom commands*. Any frontend code path — or any XSS that survives DOMPurify / CSP — can read `~/.ssh/id_rsa` or write to a Windows Startup folder via the plugin. This turns what should be a contained write into arbitrary-file-write / arbitrary-file-read.

**Recommendation (in priority order):**
1. Scope the capability to the directories the app legitimately needs (`$HOME`, `$APPCONFIG`, project roots) instead of `**`. Tauri supports path-variable scopes and per-window capabilities.
2. Better: route **all** file I/O through the existing custom commands (`read_file_content`, a `write_file_content`) so `path_scope.rs` is the single chokepoint, and drop the broad fs-plugin grant entirely. The previews already `invoke('read_file_content', …)` for *reading* — but then `writeTextFile` for *writing*, which is the inconsistency to close.
3. At minimum, document that `path_scope.rs` is **not** a security boundary for the frontend today, so nobody mistakes it for one.

---

### SEC-02 — Wildcard `https:` in CSP connect/img/media  **[Medium]**

`tauri.conf.json` → `app.security.csp`:

```
connect-src 'self' ipc: http://ipc.localhost https: ws: wss:;
img-src 'self' data: blob: https: asset: http://asset.localhost;
media-src 'self' data: blob: asset: http://asset.localhost
```

`connect-src https: ws: wss:` permits the webview to open a connection to **any** host on the internet. This is the natural consequence of supporting arbitrary AI providers (Anthropic, Gemini, OpenAI, OpenRouter, custom base URLs) and remote avatars — so it is *defensible* — but it means the CSP provides **no exfiltration containment** if an XSS ever lands. Combined with SEC-01, an injected script could read a local secret and POST it anywhere.

**Recommendation:** If the provider set is effectively closed, pin `connect-src` to the known API hosts (`https://api.anthropic.com`, `https://openrouter.ai`, `https://generativelanguage.googleapis.com`, `https://api.github.com`, `https://github.com`, the OpenRouter stats endpoint, fonts). For genuinely user-configurable custom endpoints this is hard to lock down statically; accept the residual risk but treat XSS prevention (DOMPurify everywhere — already good) as the primary control and keep it that way.

---

### SEC-03 — Custom-provider `base_url` is unvalidated  **[Medium]**

`crates/rustic-agent/src/config.rs` exposes `base_url: Option<String>` per provider, and the provider implementations send the API key (`Authorization: Bearer …` / `x-api-key`) to that URL. There is no scheme/host validation that I can see at the config layer (contrast with the careful `validate_git_url` in `git.rs`).

**Impact:** A user (or anything that can write the provider config / settings) can point a provider at `http://…` or an attacker host, and the stored API key will be transmitted there in cleartext (http) or to the wrong party. This is partly "user shoots own foot," but config can be set programmatically and keys are high-value.

**Recommendation:** Require `https://` for any `base_url`, reject private/loopback hosts unless explicitly a localhost-LLM use case (Ollama/LM Studio commonly use `http://localhost:11434` — if you support that, allowlist loopback *only*, and warn). Reuse the `ip_addr_is_private` helper from `web_tools`.

---

### SEC-04 — No session-ownership check on terminal write/resize/close  **[Low]**

`write_terminal`, `resize_terminal`, `close_terminal`, `read_terminal_buffer`, `read_terminal_screen` take a raw `session_id: u64` and operate on whatever session matches, with no check that the caller "owns" it. For a single-user local app this is low risk, but it means an XSS could write arbitrary keystrokes into an **agent-owned** terminal (which may be running with broader autonomy) or scrape another session's buffer.

**Recommendation:** Low priority. If you ever add multi-surface isolation, gate agent-owned sessions so the user-facing IPC can't inject into them. Today, accept and document.

---

### SEC-05 — Windows system-root detection uses substring `contains()`  **[Low]**

`path_scope.rs::path_is_in_system_root` (Windows) does `lc.contains(":\\windows")` etc. This:
- **Over-matches** harmlessly (a project literally at `D:\windows-tools\…` containing the substring `:\windows`? No — needs `:\windows` exactly; but `D:\Windows Backup` *does* match `:\windows` and would be falsely blocked). Over-blocking is safe but surprising.
- The comment itself acknowledges it's a heuristic. The `:\` prefix anchors it to a drive root reasonably well, but it is not component-accurate.

**Recommendation:** Prefer component-wise comparison (split on separators, compare normalized components) rather than substring matching, consistent with the Unix branch which already does `s == root || s.starts_with("{root}/")`.

---

### SEC-06 — `resolve_with_scope` validates canonical but returns joined  **[Low]**

In `file_ops.rs`, `resolve_with_scope` canonicalizes the existing ancestor and checks `starts_with(canon_root)`, then **returns the non-canonical `joined` path**. For files that don't exist yet, the deepest *existing* ancestor is what gets validated; a symlink created between the check and the write (TOCTOU) or a not-yet-existing intermediate component could in principle let a write land outside root. In practice the project root canonicalization + ancestor check make this hard to exploit, and it's the agent (not a remote attacker) acting.

**Recommendation:** Return the canonicalized path and operate on *that*, or re-validate immediately before the write syscall. Low severity given the threat model (the agent is semi-trusted and the user approves the workspace).

---

### SEC-07 — Committed GitHub OAuth client ID  **[Info]**

`git.rs`: `const GITHUB_CLIENT_ID: &str = "Ov23lijXgTEVp8hmIRf3";`. This is **expected and correct** for the OAuth Device Flow — the client ID is public by design and there is no client secret in the device flow. No action needed; noted so a future reviewer doesn't flag it as a leaked secret.

---

### SEC-08 — Confirm secrets are never logged  **[Info]**

`github_poll_token` has an explicit `// Do not log status code or body` comment (good). Verify the same discipline holds across all provider request/response paths — particularly any `tracing::debug!`/`trace!` that might serialize a full request struct containing `api_key`, or a response body containing `access_token`. Consider a `#[derive]`-level redaction (a `Secret<String>` newtype whose `Debug` prints `***`) for `api_key`/token fields so this can't regress.

**Recommendation:** Wrap key/token fields in a redacting newtype; grep CI for `tracing::.*(api_key|token|secret)`.

---

## Things checked and found OK (so they're not re-investigated later)

- **XSS sinks** — every `dangerouslySetInnerHTML` is fed DOMPurify output; `xlsx-preview` clears `innerHTML` to `''` only (not injection). `code-copy.js` sets `innerHTML` to *static* SVG icon constants. No `eval`/`new Function`/`document.write`.
- **Command injection** — no `Command::new` with shell-interpolated strings; git args are arrays; rebase uses `GIT_EDITOR=true` to avoid editor spawns. Only `Command::new` users are `formatters.rs`, `file_tree.rs`, `app_icon.rs`, git crate — all argv-based.
- **Git URL validation** — `validate_git_url` rejects `file://`, `git://`, `ext::`, control chars; only allows `https://` and SCP-style.
- **Clone target traversal** — `validate_clone_target` rejects `..` and confines under `$HOME`.
- **Process-window hiding** — `CREATE_NO_WINDOW` set on Windows git invocations (prevents flashing consoles; minor anti-annoyance, not security).
- **Token-save / remote-change** gated behind native dialogs on a blocking thread (can't be auto-clicked by the webview).

---

## Prioritized remediation order

1. **SEC-01** — scope the `fs` capability / route writes through custom commands. *This is the one finding that materially changes the blast radius of any future XSS.*
2. **SEC-03** — validate provider `base_url` (cheap, reuses existing helper).
3. **SEC-02** — tighten CSP `connect-src` to known hosts if feasible.
4. SEC-05 / SEC-06 — correctness hardening of the path guards.
5. SEC-04 / SEC-08 — defense-in-depth, document or wrap-and-forget.
