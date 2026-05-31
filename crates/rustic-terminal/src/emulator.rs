//! Headless terminal emulation.
//!
//! This runs **alongside** `portable-pty` (the byte transport) and xterm.js
//! (the visible frontend renderer) — it replaces neither. The frontend keeps
//! rendering exactly as before. The sole job of this module is to maintain an
//! in-memory screen grid, fed the same PTY bytes as the rolling raw buffer, so
//! that the **agent** can read the *rendered* screen as plain text instead of
//! the raw escape-code stream that `read_output_tail` returns.
//!
//! Why this matters: when the model reads a terminal running a TUI (or any
//! colorized output), the raw buffer is full of `\x1b[…m` / cursor-movement
//! sequences. A real VT emulator collapses those into "what's actually on
//! screen". `alacritty_terminal` is a battle-tested, pure-Rust VT emulator, so
//! we drive a headless `Term` here and render its grid on demand.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

/// Minimal `Dimensions` implementation used to construct and resize the
/// headless `Term`. We don't keep scrollback in the emulator (the raw buffer
/// already serves that role), so `total_lines == screen_lines`.
#[derive(Clone, Copy)]
struct GridSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// A headless VT emulator for one session. Fed PTY bytes via [`advance`], kept
/// in sync with the PTY size via [`resize`], and rendered to plain text via
/// [`render_screen`].
///
/// [`advance`]: TerminalEmulator::advance
/// [`resize`]: TerminalEmulator::resize
/// [`render_screen`]: TerminalEmulator::render_screen
pub struct TerminalEmulator {
    term: Term<VoidListener>,
    parser: Processor,
    cols: usize,
    rows: usize,
}

impl TerminalEmulator {
    /// Create an emulator sized to the session's initial PTY dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let cols = (cols as usize).max(1);
        let rows = (rows as usize).max(1);
        let size = GridSize {
            columns: cols,
            screen_lines: rows,
        };
        let term = Term::new(Config::default(), &size, VoidListener);
        Self {
            term,
            parser: Processor::new(),
            cols,
            rows,
        }
    }

    /// Feed a chunk of raw PTY bytes into the emulator. Called from the
    /// output-reader thread for every chunk it reads, right after it appends
    /// the same bytes to the raw rolling buffer.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Keep the emulator grid in lock-step with the PTY size. A no-op when the
    /// dimensions are unchanged (resize events fire frequently during drags).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = (cols as usize).max(1);
        let rows = (rows as usize).max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.resize(GridSize {
            columns: cols,
            screen_lines: rows,
        });
    }

    /// Render the *visible* screen as plain text — no escape sequences. Trailing
    /// whitespace on each row is trimmed, and fully-blank trailing rows are
    /// dropped, so the result reads like a screenshot of the terminal.
    pub fn render_screen(&self) -> String {
        let grid = self.term.grid();
        let mut lines: Vec<String> = Vec::with_capacity(self.rows);
        let mut current_line: Option<i32> = None;
        let mut buf = String::new();

        for indexed in grid.display_iter() {
            // Skip the placeholder cell(s) that trail a wide glyph (CJK/emoji),
            // otherwise we'd emit a phantom space after every wide char.
            if indexed
                .cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let line = indexed.point.line.0;
            match current_line {
                Some(l) if l == line => {}
                Some(_) => {
                    lines.push(buf.trim_end().to_string());
                    buf.clear();
                }
                None => {}
            }
            current_line = Some(line);
            buf.push(indexed.cell.c);
        }
        if current_line.is_some() {
            lines.push(buf.trim_end().to_string());
        }

        // Trim trailing blank rows so a mostly-empty screen isn't padded out.
        while matches!(lines.last(), Some(l) if l.is_empty()) {
            lines.pop();
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_color_escape_codes() {
        let mut emu = TerminalEmulator::new(80, 24);
        emu.advance(b"\x1b[31mRED\x1b[0m text");
        // The rendered screen should be the plain characters, no SGR codes.
        assert_eq!(emu.render_screen(), "RED text");
    }

    #[test]
    fn resolves_cursor_movement_and_clear() {
        let mut emu = TerminalEmulator::new(80, 24);
        // Write "garbage", then clear screen + home cursor, then "clean".
        emu.advance(b"garbage\x1b[2J\x1b[Hclean");
        assert_eq!(emu.render_screen(), "clean");
    }

    #[test]
    fn keeps_multiple_lines() {
        let mut emu = TerminalEmulator::new(80, 24);
        emu.advance(b"line1\r\nline2\r\nline3");
        assert_eq!(emu.render_screen(), "line1\nline2\nline3");
    }

    #[test]
    fn in_place_redraw_reflects_latest_frame() {
        // Simulate a TUI repainting the same line: a raw byte buffer would show
        // both "loading" and "done"; the emulator should show only the final.
        let mut emu = TerminalEmulator::new(80, 24);
        emu.advance(b"loading...\r");
        emu.advance(b"done      \r");
        assert_eq!(emu.render_screen(), "done");
    }

    #[test]
    fn resize_does_not_panic_and_renders() {
        let mut emu = TerminalEmulator::new(80, 24);
        emu.advance(b"hello");
        emu.resize(40, 12);
        emu.resize(120, 30);
        assert!(emu.render_screen().contains("hello"));
    }
}
