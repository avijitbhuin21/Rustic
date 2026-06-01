# Rustic — Application Audit

**Date:** 2026-06-01 · **Branch:** `rebuilt-ui` · **Scope:** whole repo (~88k LOC: Rust crates, Tauri host, React frontend)

Read-only audit. **No code was changed** — these are findings + recommendations only.

## Reports

1. **[Security](01-security-audit.md)** — Tauri capabilities, path scoping, secrets, XSS, SSRF, git/credential handling, command execution.
2. **[UI / UX](02-ui-ux-audit.md)** — layout, accessibility, motion, tooltips, empty/error/loading states, consistency.
3. **[Cleanliness & Good Practices](03-cleanliness-good-practices-audit.md)** — stray files, dead code, comment hygiene, panic/robustness patterns, module size.
4. **[Performance (runtime)](04-performance-audit.md)** — rendering, virtualization, memoization, IPC volume, hot Rust paths, bundle. *(Distinct from the agent-cost work in `docs/perf_findings*.md`.)*

## Overall read

The project is in **strong shape** — clearly built by someone who understands the platform's footguns and has already closed many of them (SSRF IP-pinning, DOMPurify everywhere, native trust dialogs, shell allowlisting, code-splitting, search caps, file-tree virtualization, ~13% comments, only 3 TODOs). The findings are mostly **defense-in-depth and polish**, not structural defects.

## Top recommendations across all four

| Priority | Finding | Report |
|----------|---------|--------|
| 1 | `fs` plugin capability is `{"path":"**"}` — frontend file I/O bypasses all Rust path-scoping (biggest blast-radius item) | SEC-01 |
| 2 | ✅ **Done** — `lock().unwrap()` poison-cascade on the IPC surface; fixed via `sync_ext::lock_safe()` across 13 files | CLN-02 |
| 3 | Remove `src-tauri/Pasted text #1` (unrelated chat transcript committed to git) | CLN-01 |
| 4 | Add `React.memo` to list items + virtualize search results & chat turns | PERF-01/02 |
| 5 | `<MotionConfig reducedMotion="user">` for app-wide reduced-motion a11y | UX-01 |
| 6 | Validate custom-provider `base_url` (https-only, reuse `ip_addr_is_private`) | SEC-03 |
| 7 | Unify on the `Tooltip` component + accessible names on icon buttons | UX-02/05 |
| 8 | De-hardcode the version string / fake `UTF-8`·`LF` indicators in the status bar | UX-03 / CLN-05 |

Each report has its own severity-ranked table and a prioritized remediation order.
