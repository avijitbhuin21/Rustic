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
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

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

    /// Serialize the FULL grid — scrollback history *and* the visible screen —
    /// as a clean ANSI/VT string (characters + SGR color/attribute codes,
    /// `\r\n`-separated). Unlike the raw PTY byte buffer, this is the emulator's
    /// *resolved* grid: every in-place repaint a TUI (Claude Code / Ink, a
    /// progress bar, …) performed has already collapsed into the final cell
    /// state, so replaying this into xterm reproduces the history exactly once —
    /// no duplicated frames, which is the whole point versus replaying raw
    /// ConPTY output. Used by the frontend to (re)hydrate a terminal's
    /// scrollback when its xterm instance is (re)built.
    ///
    /// Lines are emitted with a leading `\x1b[0m` reset per style-run and a
    /// trailing reset per row, so a partially-styled tail can't bleed into the
    /// next line. Fully-blank leading/trailing rows are trimmed; interior blank
    /// rows are preserved so spacing in the original output survives.
    pub fn render_scrollback_ansi(&self) -> String {
        let grid = self.term.grid();
        let cols = grid.columns();
        let history = grid.history_size() as i32;
        let screen = grid.screen_lines() as i32;

        let mut rows: Vec<String> = Vec::with_capacity((history + screen) as usize);
        for line in (-history)..screen {
            let row = &grid[Line(line)];

            // Rightmost column worth emitting: a non-space glyph, or any cell
            // carrying visible styling (inverse/underline/strikeout or a
            // non-default background) that a bare trailing space would lose.
            let mut last_col: i32 = -1;
            for c in 0..cols {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let blank = cell.c == ' '
                    && is_default_bg(&cell.bg)
                    && !cell.flags.intersects(
                        Flags::INVERSE | Flags::UNDERLINE | Flags::STRIKEOUT,
                    );
                if !blank {
                    last_col = c as i32;
                }
            }

            if last_col < 0 {
                rows.push(String::new());
                continue;
            }

            let mut s = String::new();
            let mut cur_sgr = String::new();
            for c in 0..=(last_col as usize) {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let sgr = cell_sgr(cell.fg, cell.bg, cell.flags);
                if sgr != cur_sgr {
                    s.push_str(&sgr);
                    cur_sgr = sgr;
                }
                s.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
            s.push_str("\x1b[0m");
            rows.push(s);
        }

        // Trim fully-blank leading and trailing rows (a tall screen over short
        // output otherwise pads the replay with empty lines).
        while matches!(rows.first(), Some(r) if r.is_empty()) {
            rows.remove(0);
        }
        while matches!(rows.last(), Some(r) if r.is_empty()) {
            rows.pop();
        }

        rows.join("\r\n")
    }
}

/// True for the terminal's default background (the only background we treat as
/// "blank" when trimming trailing cells).
fn is_default_bg(c: &Color) -> bool {
    matches!(c, Color::Named(NamedColor::Background))
}

/// Map a `NamedColor` to its 0..=15 ANSI palette index (basic 0-7, bright 8-15,
/// dim folded onto its basic slot). Returns `None` for the special slots
/// (default fg/bg, cursor) which have no fixed palette index.
fn named_index(n: NamedColor) -> Option<u8> {
    use NamedColor::*;
    Some(match n {
        Black => 0,
        Red => 1,
        Green => 2,
        Yellow => 3,
        Blue => 4,
        Magenta => 5,
        Cyan => 6,
        White => 7,
        BrightBlack => 8,
        BrightRed => 9,
        BrightGreen => 10,
        BrightYellow => 11,
        BrightBlue => 12,
        BrightMagenta => 13,
        BrightCyan => 14,
        BrightWhite => 15,
        DimBlack => 0,
        DimRed => 1,
        DimGreen => 2,
        DimYellow => 3,
        DimBlue => 4,
        DimMagenta => 5,
        DimCyan => 6,
        DimWhite => 7,
        _ => return None, // Foreground / Background / Cursor / Bright/Dim foreground
    })
}

/// SGR parameter for one color. `is_fg` picks the 3x/9x (fg) vs 4x/10x (bg)
/// range. Returns the default-color param ("39"/"49") for special slots, which
/// the caller drops since each run already starts from a `0` reset.
fn color_param(color: Color, is_fg: bool) -> String {
    match color {
        Color::Named(n) => match named_index(n) {
            Some(i) if i < 8 => (if is_fg { 30 } else { 40 } + i as u16).to_string(),
            Some(i) => (if is_fg { 90 } else { 100 } + (i as u16 - 8)).to_string(),
            None => (if is_fg { "39" } else { "49" }).to_string(),
        },
        Color::Indexed(i) => format!("{};5;{}", if is_fg { 38 } else { 48 }, i),
        Color::Spec(rgb) => {
            format!("{};2;{};{};{}", if is_fg { 38 } else { 48 }, rgb.r, rgb.g, rgb.b)
        }
    }
}

/// Build the SGR escape for a cell's full style: always begins with `0` (reset)
/// so it is self-contained regardless of the prior cell, then appends only the
/// non-default attributes/colors. A fully-default cell yields `\x1b[0m`.
fn cell_sgr(fg: Color, bg: Color, flags: Flags) -> String {
    let mut params: Vec<String> = vec!["0".to_string()];
    if flags.contains(Flags::BOLD) {
        params.push("1".into());
    }
    if flags.contains(Flags::DIM) {
        params.push("2".into());
    }
    if flags.contains(Flags::ITALIC) {
        params.push("3".into());
    }
    if flags.contains(Flags::UNDERLINE) {
        params.push("4".into());
    }
    if flags.contains(Flags::INVERSE) {
        params.push("7".into());
    }
    if flags.contains(Flags::HIDDEN) {
        params.push("8".into());
    }
    if flags.contains(Flags::STRIKEOUT) {
        params.push("9".into());
    }
    let fg_p = color_param(fg, true);
    if fg_p != "39" {
        params.push(fg_p);
    }
    let bg_p = color_param(bg, false);
    if bg_p != "49" {
        params.push(bg_p);
    }
    format!("\x1b[{}m", params.join(";"))
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

    /// Strip SGR codes from a serialized string to assert on the plain text.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip a CSI sequence: ESC [ ... <final byte 0x40..=0x7E>
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for cc in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&cc) {
                            break;
                        }
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn scrollback_serializes_history_beyond_the_screen() {
        // A short screen (5 rows) with far more output must retain the earlier
        // lines in the serialized scrollback — they scrolled off the viewport
        // but live in the emulator's history grid.
        let mut emu = TerminalEmulator::new(80, 5);
        for i in 1..=50 {
            emu.advance(format!("line{}\r\n", i).as_bytes());
        }
        let text = strip_ansi(&emu.render_scrollback_ansi());
        assert!(text.contains("line1"), "earliest history line missing");
        assert!(text.contains("line25"), "mid history line missing");
        assert!(text.contains("line50"), "latest line missing");
    }

    #[test]
    fn scrollback_collapses_in_place_repaints_no_duplication() {
        // The crux of the duplication bug: a TUI repainting the same row (here
        // a fake progress spinner) must appear ONCE in the serialized
        // scrollback, not once per frame, because the emulator resolves the
        // grid instead of logging every raw frame.
        let mut emu = TerminalEmulator::new(80, 5);
        for pct in [10, 20, 30, 40, 100] {
            // Carriage return back to col 0, overwrite the same line.
            emu.advance(format!("\rProgress: {}%   ", pct).as_bytes());
        }
        let text = strip_ansi(&emu.render_scrollback_ansi());
        assert_eq!(
            text.matches("Progress:").count(),
            1,
            "in-place repaint duplicated in scrollback: {:?}",
            text
        );
        assert!(text.contains("Progress: 100%"), "final frame not preserved");
    }

    #[test]
    fn scrollback_after_clear_keeps_history_once() {
        // `clear`/cls (ESC[2J ESC[H) SCROLLS the prior screen into scrollback
        // (standard terminal behaviour — you can scroll up after `clear`), so
        // the old content is legitimately retained. The invariant that matters
        // for the duplication bug is that it appears exactly ONCE, and the new
        // content is present.
        let mut emu = TerminalEmulator::new(80, 5);
        emu.advance(b"OLD CONTENT\r\n");
        emu.advance(b"\x1b[2J\x1b[Hfresh start");
        let text = strip_ansi(&emu.render_scrollback_ansi());
        assert!(text.contains("fresh start"));
        assert_eq!(
            text.matches("OLD CONTENT").count(),
            1,
            "cleared content duplicated in scrollback: {:?}",
            text
        );
    }

    #[test]
    fn scrollback_preserves_basic_colors() {
        let mut emu = TerminalEmulator::new(80, 5);
        // Red "ERR", reset, then default "ok".
        emu.advance(b"\x1b[31mERR\x1b[0m ok");
        let ansi = emu.render_scrollback_ansi();
        assert!(ansi.contains("31"), "red SGR not serialized: {:?}", ansi);
        assert_eq!(strip_ansi(&ansi), "ERR ok");
    }

    #[test]
    fn scrollback_preserves_truecolor_and_bold() {
        let mut emu = TerminalEmulator::new(80, 5);
        emu.advance(b"\x1b[1m\x1b[38;2;10;20;30mX\x1b[0m");
        let ansi = emu.render_scrollback_ansi();
        assert!(ansi.contains("38;2;10;20;30"), "truecolor lost: {:?}", ansi);
        assert!(ansi.contains('1'), "bold lost: {:?}", ansi);
    }
}
