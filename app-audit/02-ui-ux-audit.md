# Rustic — UI / UX Audit

**Date:** 2026-06-01
**Branch:** `rebuilt-ui`
**Scope:** `src/` React frontend — 113 components, 13 Zustand stores, `globals.css`, shadcn/ui (Radix) component library.
**Method:** Read-only static review of component structure, interaction patterns, accessibility attributes, theming, and state choreography. No live/visual testing or screen-reader run performed — findings are code-derived and should be confirmed visually.

---

## Executive summary

The frontend is **well-architected and unusually thoughtful** about interaction detail. The layout is a clean VS Code-style shell (activity bar → sidebar → editor/terminal → status bar) built on `react-resizable-panels` with carefully reasoned panel-tree keys to avoid sizing-state bleed. Notable strengths:

- **Extensive, specific empty states** — 40+ distinct "No X" messages (`No projects in your workspace yet`, `No changes to commit`, `No models match the filter`, …). This is the single biggest UX-quality signal: nearly every list/panel has a real empty state, not a blank box.
- **Robust error handling at the shell level** — top-level `ErrorBoundary` with a real recovery screen + reload, plus `installGlobalErrorHandlers()` wiring uncaught errors/rejections to a backend crash log *before first paint*.
- **Consistent toast feedback** via `sonner`, including a thoughtful *persistent* (duration: Infinity) toast for the actionable "git not found" case rather than an auto-dismissing one.
- **Loading affordances** — `Skeleton` used in 17 components, `Loader2` spinners in 20; previews show skeletons while reading file content.
- **Motion choreography is deliberate** — the chat dock and its prompt box share one `PROMPT_SPRING` so they read as a single motion; comments show real care about HMR/StrictMode remount costs.
- **A skip-link exists** (`globals.css` line ~180, `&:focus { left: 0 }`) — a genuine a11y nicety most apps skip.
- **Native confirm dialogs** for destructive/trust actions via a shared `ConfirmDialogHost`.

The findings below are refinements, not structural problems. The two worth prioritizing are **UX-01** (reduced-motion) and **UX-02** (tooltip inconsistency), both of which affect accessibility and polish across many surfaces.

| ID | Severity | Title |
|----|----------|-------|
| UX-01 | Medium | App-wide animations ignore `prefers-reduced-motion` (only `agent-plan.jsx` honors it) |
| UX-02 | Medium | Tooltip inconsistency — 25 files use native `title=`, 12 use the `Tooltip` component |
| UX-03 | Low | Status bar shows hardcoded `v0.3.1` (app is 0.3.4) and always-on `UTF-8` / `LF` |
| UX-04 | Low | Single global `ErrorBoundary` — one panel's render crash blanks the entire app |
| UX-05 | Low | Icon-only buttons: confirm every one has an accessible name (`aria-label`) |
| UX-06 | Low | Mixed loading idioms (Skeleton vs spinner) with no obvious rule for which to use |
| UX-07 | Info | `bindListeners()` is invoked in both `App` and `AgentPanel` |

---

## Findings

### UX-01 — Animations don't respect `prefers-reduced-motion`  **[Medium, Accessibility]**

`framer-motion` drives entrance/morph animations in **9 component files** (chat dock spring, model-change divider, condense banner, stream-retry banner, ask-user-inline, agent plan, etc.). Only **one** of them — `ui/agent-plan.jsx` — checks `window.matchMedia('(prefers-reduced-motion: reduce)')`. The primary chat-dock mount (`agent-panel.jsx`) always plays a spring (`initial={{ opacity: 0, x: 24 }}`), as do the banners and dividers.

**Impact:** Users who set "reduce motion" at the OS level (a real accessibility need — vestibular disorders, motion sensitivity) still get sliding/springing panels. WebView2/WKWebView both expose the media query, so the signal is available.

**Recommendation:** Add a shared hook (`useReducedMotion` — framer-motion ships one: `import { useReducedMotion } from 'framer-motion'`) and either (a) gate `initial`/`animate` on it, or (b) wrap the app in framer's `MotionConfig reducedMotion="user"`, which makes *all* descendant motion components honor the OS setting with a single line. Option (b) is the cheapest comprehensive fix.

---

### UX-02 — Two parallel tooltip systems  **[Medium, Consistency]**

25 component files use the **native `title="..."` attribute** for hover hints; 12 use the design-system **`Tooltip`/`TooltipContent`** (Radix). The status bar, for instance, uses `title="Sign in to GitHub"` and `title={`Signed in as ${label}`}` while elsewhere the app uses styled tooltips.

**Why it matters:**
- Native `title` tooltips are **unstyled** (OS-default, light background even in dark theme), have a **fixed ~1s delay** you can't tune (the app's Tooltip uses `delayDuration={300}`), don't appear on **keyboard focus** reliably, and **truncate** long text differently per-OS.
- The result is visibly inconsistent hint behavior depending on which control the user hovers.

**Recommendation:** Standardize on the `Tooltip` component for all interactive controls. Native `title` is acceptable only as a non-interactive fallback (e.g., truncated text showing its full value). A quick sweep replacing `title=` on `<button>`/icon controls would unify the feel — and it pairs naturally with UX-05 (the same controls need accessible names anyway).

---

### UX-03 — Stale/placeholder status-bar metadata  **[Low]**

`shell/status-bar.jsx`:
- `<span>Rustic v0.3.1</span>` — **hardcoded and drifted**; `tauri.conf.json` is at `0.3.4` (and v0.3.5 is in flight). A version string that lies erodes trust and confuses bug reports.
- `<span>UTF-8</span>` and `<span>LF</span>` are **hardcoded literals** — they display for every file regardless of the file's real encoding or line endings. A CRLF file still reads "LF". VS Code users expect these to be *live* and even *clickable* (to convert).

**Recommendation:**
- Source the version from `package.json`/`tauri.conf.json` at build time (Vite `define` or `import.meta.env`) instead of a literal.
- Either wire `UTF-8`/`LF` to the actual detected encoding/EOL of the active document, or remove them until they're real — a fake indicator is worse than none.

---

### UX-04 — One global error boundary, no per-region isolation  **[Low]**

`main.jsx` wraps `<App/>` in a single `ErrorBoundary`. A render/lifecycle throw *anywhere* — a malformed XLSX in a preview, a bad diff, a third-party Monaco hiccup — collapses the **entire window** to the "Something went wrong / Reload" screen, losing all other panels' state.

**Recommendation:** Add boundary granularity around the high-risk, independently-recoverable regions: each editor preview (the `previews/*` family parse untrusted file content), the agent chat view, the terminal host, and the SCM/diff panel. A failed preview should show an inline "couldn't render this file" card while the rest of the IDE keeps working. The existing `ErrorBoundary` class can be reused with a lighter inline fallback prop.

---

### UX-05 — Verify accessible names on icon-only controls  **[Low, Accessibility]**

There are ~26 `size="icon"`/`size="xs"` button usages and many bare `<lucide-icon>`-in-`<button>` controls. The repo has 45 `aria-label` occurrences across 44 files — decent coverage — but icon-only buttons are exactly the controls that *need* an accessible name and are easiest to miss. Window controls, activity-bar icons, tab close buttons, and toolbar toggles should each have an `aria-label` (or a `Tooltip` that also sets one).

**Recommendation:** Audit every `<button>` whose only child is an icon; ensure an `aria-label`. Combined with UX-02, a `Tooltip`-wrapped icon button can supply both the visible hint and the accessible name.

---

### UX-06 — Mixed loading idioms  **[Low, Consistency]**

Both `Skeleton` (17 files) and `Loader2` spinners (20 files) are used for "loading" states, with no obvious convention. Inconsistent loaders make the app feel less cohesive (some panels shimmer, some spin, for the same kind of wait).

**Recommendation:** Adopt a simple rule and apply it: **skeletons for content whose shape is known** (file preview, lists, chat history) and **spinners for indeterminate actions** (a button mid-request, a probe). Document it once in a comment or a tiny `loading` primitive.

---

### UX-07 — Duplicate `bindListeners()` wiring  **[Info]**

`App.jsx` calls `useAgent.getState().bindListeners()` at startup *and* `AgentPanel` calls `bindListeners().then(cleanup => …)` on mount, treating the return as a cleanup fn. App's comment says it's a true singleton returning a no-op cleanup; `AgentPanel`'s usage assumes a meaningful cleanup. These two assumptions disagree. It's currently harmless (idempotent), but it's a latent footgun: if `bindListeners` ever starts returning a *real* unsubscribe, `AgentPanel` unmounting would tear down listeners that `App` still depends on.

**Recommendation:** Pick one owner. Since App needs the listeners alive regardless of which agent UI is mounted (its own comment says so), bind **only** in App and have `AgentPanel` not touch listener lifecycle.

---

## Things checked and found good (no action)

- **Empty states** — comprehensive and specific across explorer, SCM, search, agent, MCP, models, terminals, formatters, fonts. Excellent.
- **Skip link** present in `globals.css`.
- **Crash resilience** — global handlers + boundary + backend crash log.
- **Theming** — proper CSS-variable token system, dark variant, locally-bundled variable fonts (Geist/Inter/Victor Mono) with `font-display: swap`, and a font-rehydration path on reload (a subtle correctness win most apps get wrong).
- **Confirm dialog** centralized (`ConfirmDialogHost`), and trust actions use **native** OS dialogs.
- **Panel sizing** — deliberate distinct `id`/group keys to stop `react-resizable-panels` from bleeding sizes across structurally different layouts.
- **Radix primitives** (dialog, dropdown, popover, tooltip, select) bring focus-trapping, escape-handling, and ARIA roles for free on those surfaces.

---

## Prioritized order

1. **UX-01** — wrap in `<MotionConfig reducedMotion="user">` (one line, app-wide a11y win).
2. **UX-02 / UX-05** — unify on `Tooltip` and ensure icon buttons have accessible names (do together).
3. **UX-03** — fix the lying version string + fake encoding/EOL indicators.
4. **UX-04** — add per-region error boundaries around previews and the chat/terminal hosts.
5. UX-06 / UX-07 — consistency + lifecycle cleanup.
