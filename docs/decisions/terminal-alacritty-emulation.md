# ADR: `alacritty_terminal` migration — scope and rationale

**Status:** Implemented — 2026-05-31 (additive headless emulator; see
`crates/rustic-terminal/src/emulator.rs`)
**Context:** Terminal Enhancement goal, item #1 ("Migrate backend from
`portable-pty` to `alacritty_terminal` for better TUI support").

## The architectural reality

The original goal framed the work as *"migrate the backend to
`alacritty_terminal` for better TUI support, while keeping xterm.js + WebGL on
the frontend."* Those two halves are in tension, and it's worth writing down why
so we don't build a redundant component.

The current data path is:

```
shell process
  → portable-pty            (byte transport only: spawn, read, write, resize)
  → terminal-output event   (raw PTY bytes, lossy-UTF8)
  → xterm.js + WebGL         (FULL VT emulation + GPU rendering)
```

**xterm.js is already a complete terminal emulator.** It parses every escape
sequence, maintains the screen grid, scrollback, colors, cursor, mouse modes,
and renders the result. vim / htop / lazygit already render through it today.
The reason TUIs render well isn't the PTY layer — it's xterm.js plus the
careful config in `terminal-pane.jsx` (`windowsPty: { backend: 'conpty' }`,
`allowProposedApi`, `convertEol: false`, the Cascadia font metrics, WebGL).

`alacritty_terminal` is *also* a full VT emulator. It consumes PTY bytes and
produces a **grid of cells** — not a byte stream. xterm.js consumes a **byte
stream**, not a grid. So you cannot insert `alacritty_terminal` *between* the
PTY and xterm.js: there is no rendering destination for an Alacritty grid while
xterm.js remains the renderer. Putting both emulators in the pipeline means
parsing every escape sequence twice, to no benefit, and risks the two emulators
disagreeing about screen state.

**Conclusion:** A straight "swap `portable-pty` for `alacritty_terminal`" while
keeping xterm.js is not a coherent change. `portable-pty` is the *transport*;
`alacritty_terminal` is an *emulator*. They are not substitutes.

## What `portable-pty` actually is, and why we keep it

`portable-pty` does ConPTY/openpty management: spawn a shell, get a master
read/write handle, resize. That job still needs doing regardless of any
emulator. Replacing it would mean hand-rolling ConPTY (and the
already-hard-won Windows exit/EOF handling in `pty.rs` /
`commands/terminal.rs`). There is no TUI-rendering reason to touch it. **Keep
`portable-pty`.**

## The genuinely valuable use of a headless emulator

There *is* a real problem a backend emulator solves, just not the one the goal
named. Today the agent reads its terminals via `read_terminal_output`, which
returns `read_tail()` of the **raw byte buffer** — escape sequences and all.
For a TUI (or even colorized output), the model sees `\x1b[2J\x1b[H` noise
instead of the rendered screen.

A **headless `alacritty_terminal::Term`**, fed the same bytes as the rolling
buffer, would let us render the *current screen as plain text* on demand and
hand the model a clean grid. That is the version worth building:

```
shell → portable-pty → output-reader thread ─┬─→ raw buffer  → frontend (xterm.js renders)
                                             └─→ headless Term (alacritty) → grid→text for the AGENT
```

Note this is **additive**, not a migration: the frontend path is untouched;
we add a parallel headless emulator purely to give the agent a rendered view.

### Implementation sketch (if/when prioritized)

- Add `alacritty_terminal` to `crates/rustic-terminal/Cargo.toml`. It is pure
  Rust (no C deps), so it respects the workspace's zero-C-dependency rule.
- Per session, hold a `Term<L>` + `vte::ansi::Processor`. In the output-reader
  thread, after `append_output(raw)`, also `processor.advance(&mut term, &buf)`.
- On `resize_session`, also resize the `Term` (`term.resize(TermSize{..})`) so
  the grid dimensions track the PTY.
- Add `TerminalManager::render_screen(id) -> String` that walks
  `term.grid()` rows and emits trimmed text. Wire a new branch into the
  agent's `read_terminal_output` tool (e.g. a `rendered: bool` arg) so the
  model can ask for "what's on screen now" vs. "raw scrollback".
- Concurrency: keep the `Term` behind the session's existing lock; the
  reader thread is the only writer. Confirm the chosen `EventListener` is
  `Send` (use a no-op listener — we don't need bells/title events).

### Cost / benefit

- **Benefit:** materially better agent comprehension of TUI / colorized output.
- **Cost:** one more emulator to feed and resize; ~a few hundred lines; new
  dependency tree (sizeable but pure-Rust).
- **Not blocking** the frontend enhancement work (reorder / split / layout),
  which is independent and already shipped.

## Decision

1. **Do not** replace `portable-pty` with `alacritty_terminal`. Keep
   `portable-pty` as the transport and xterm.js as the frontend emulator.
2. Adopt `alacritty_terminal` as an **additive, backend-only, headless**
   emulator whose sole purpose is giving the **agent** a rendered text view of
   a terminal.

## What was actually built (2026-05-31)

Implemented exactly as sketched above:

- `alacritty_terminal = "0.26"` added to `crates/rustic-terminal/Cargo.toml`
  (vte 0.15 comes transitively, re-exported as `alacritty_terminal::vte`).
- `crates/rustic-terminal/src/emulator.rs` — `TerminalEmulator` wraps a
  headless `Term<VoidListener>` + `vte::ansi::Processor`. `advance(&[u8])`,
  `resize(cols, rows)`, `render_screen() -> String` (walks the grid's
  `display_iter`, trims trailing blanks, skips wide-char spacer cells).
- `PtySession` holds `Arc<Mutex<TerminalEmulator>>`. The output-reader thread
  feeds it the same bytes it appends to the raw buffer; `resize` keeps the grid
  in lock-step. `TerminalManager::render_screen(id)` renders on demand.
- Agent surface: `AgentTerminals::render_screen` (default-impl falls back to
  `read_output`) + the `read_terminal_output` tool gained a `rendered: bool`
  arg. `rendered: true` → clean current-screen text; default → raw byte tail.
- Frontend display path (xterm.js) is completely untouched.
- 5 unit tests in `emulator.rs` cover SGR stripping, cursor/clear resolution,
  multi-line, in-place redraw (only final frame survives), and resize.
