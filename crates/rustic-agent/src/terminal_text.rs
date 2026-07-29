//! Cleanup of raw pty output for model- and chat-facing consumption.
//!
//! The rolling buffer a pty fills is a byte stream, not text: ConPTY (and any
//! colorized program) interleaves SGR codes, cursor moves, erase-in-line,
//! bracketed-paste toggles, cursor show/hide and window-title OSC sequences
//! with the actual characters. Worse, ConPTY *repaints the whole prompt line*
//! as the command is typed, so the buffer contains several copies of it.
//!
//! `run_command` used to hand that stream straight to the model, which made
//! results nearly unreadable and defeated `slice_output_since_command` (it
//! searched for the plain command text inside a buffer where the echo is split
//! by color codes, never matched, and so returned the entire buffer including
//! every prompt redraw).
//!
//! This module resolves the stream into plain lines: escape sequences are
//! dropped, `\r` / `\b` are applied as real cursor moves so progress bars
//! collapse to their final frame, and blank-line runs are squeezed. The
//! frontend's xterm panel is untouched — it still gets the raw bytes with
//! colors intact.

/// Resolve a raw pty byte-stream slice into readable plain text: escape
/// sequences removed, carriage-return / backspace overwrites applied, blank
/// runs squeezed, trailing whitespace trimmed.
pub fn sanitize_terminal_output(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut buf: Vec<char> = Vec::new();
    let mut col: usize = 0;
    let mut chars = raw.chars().peekable();

    let put = |buf: &mut Vec<char>, col: &mut usize, c: char| {
        if *col < buf.len() {
            buf[*col] = c;
        } else {
            buf.push(c);
        }
        *col += 1;
    };

    while let Some(c) = chars.next() {
        match c {
            '\x1b' => skip_escape(&mut chars),
            '\n' => {
                lines.push(trim_line(&buf));
                buf.clear();
                col = 0;
            }
            '\r' => col = 0,
            '\x08' => col = col.saturating_sub(1),
            '\t' => {
                let next_stop = (col / 8 + 1) * 8;
                while col < next_stop {
                    put(&mut buf, &mut col, ' ');
                }
            }
            c if (c as u32) < 0x20 || c == '\x7f' => {}
            c => put(&mut buf, &mut col, c),
        }
    }
    if !buf.is_empty() {
        lines.push(trim_line(&buf));
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.is_empty() && matches!(out.last(), Some(l) if l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while matches!(out.first(), Some(l) if l.is_empty()) {
        out.remove(0);
    }
    while matches!(out.last(), Some(l) if l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Drop everything up to and including the shell's echo of `cmd`, so the model
/// sees only what the command actually produced. Uses the LAST echo, which also
/// discards ConPTY's earlier prompt-line repaints. Falls back to the unchanged
/// text when no echo is found (e.g. a session that never echoes).
pub fn strip_command_echo(text: &str, cmd: &str) -> String {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return text.to_string();
    }
    let first_line = cmd.lines().next().unwrap_or(cmd).trim();
    let lines: Vec<&str> = text.lines().collect();
    let seeded = format!("$ {}", first_line);

    let echo_at = lines.iter().rposition(|l| {
        let t = l.trim();
        t == first_line
            || t == seeded
            || t.ends_with(first_line) && t.len() < first_line.len() + 200
    });

    match echo_at {
        Some(i) => lines[(i + 1)..]
            .join("\n")
            .trim_start_matches('\n')
            .to_string(),
        None => text.to_string(),
    }
}

/// Drop trailing shell-prompt lines (`PS C:\…>`, `C:\…>`, `user@host:~$`, a
/// bare `$`/`#`/`>`), which the shell reprints after every command and which
/// carry no information for the model.
pub fn strip_trailing_prompt(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    while let Some(last) = lines.last() {
        if last.trim().is_empty() || is_prompt_line(last) {
            lines.pop();
        } else {
            break;
        }
    }
    lines.join("\n")
}

/// Full cleanup pipeline for one command's output: sanitize, cut the echo, drop
/// the trailing prompt.
pub fn clean_command_output(raw: &str, cmd: &str) -> String {
    let sanitized = sanitize_terminal_output(raw);
    let sliced = strip_command_echo(&sanitized, cmd);
    strip_trailing_prompt(&sliced)
}

fn trim_line(buf: &[char]) -> String {
    let s: String = buf.iter().collect();
    s.trim_end().to_string()
}

fn is_prompt_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t == "$" || t == "#" || t == ">" {
        return true;
    }
    if t.starts_with("PS ") && t.ends_with('>') {
        return true;
    }
    let b = t.as_bytes();
    if b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && t.ends_with('>') {
        return true;
    }
    if let Some(at) = t.find('@') {
        let user = &t[..at];
        if !user.is_empty()
            && !user.contains(char::is_whitespace)
            && t.contains(':')
            && (t.ends_with('$') || t.ends_with('#'))
        {
            return true;
        }
    }
    false
}

/// Consume the remainder of an escape sequence whose introducing `ESC` was just
/// read. Handles CSI, OSC/DCS/SOS/PM/APC string sequences, charset selection and
/// the single-character forms.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    break;
                }
            }
        }
        Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
            while let Some(c) = chars.next() {
                if c == '\x07' {
                    break;
                }
                if c == '\x1b' {
                    if let Some('\\') = chars.peek() {
                        chars.next();
                    }
                    break;
                }
            }
        }
        Some('(') | Some(')') | Some('*') | Some('+') | Some('#') | Some('%') => {
            chars.next();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sgr_and_cursor_codes() {
        let raw = "\x1b[93mSet-ExecutionPolicy\x1b[m \x1b[90m-Scope\x1b[m Process";
        assert_eq!(
            sanitize_terminal_output(raw),
            "Set-ExecutionPolicy -Scope Process"
        );
    }

    #[test]
    fn strips_cursor_visibility_and_erase_sequences() {
        let raw = "\x1b[?25lhello\x1b[K\x1b[?25h\n\x1b[33Xworld\x1b[6;1H";
        assert_eq!(sanitize_terminal_output(raw), "hello\nworld");
    }

    #[test]
    fn strips_osc_title_sequence() {
        let raw = "\x1b]0;C:\\Windows\\cmd.exe\x07real output";
        assert_eq!(sanitize_terminal_output(raw), "real output");
    }

    #[test]
    fn carriage_return_overwrites_progress_frames() {
        let raw = "10%\r50%\r100% done\n";
        assert_eq!(sanitize_terminal_output(raw), "100% done");
    }

    #[test]
    fn backspace_erases_previous_char() {
        assert_eq!(sanitize_terminal_output("abX\x08c"), "abc");
    }

    #[test]
    fn squeezes_blank_line_runs() {
        let raw = "a\n\x1b[K\n\x1b[K\n\x1b[K\nb";
        assert_eq!(sanitize_terminal_output(raw), "a\n\nb");
    }

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(sanitize_terminal_output("a\tb"), "a       b");
    }

    #[test]
    fn echo_slice_uses_last_repaint() {
        let text = "PS D:\\p> git status\nPS D:\\p> git status\nOn branch main";
        assert_eq!(strip_command_echo(text, "git status"), "On branch main");
    }

    #[test]
    fn echo_slice_matches_seeded_marker() {
        let text = "$ git status\nOn branch main";
        assert_eq!(strip_command_echo(text, "git status"), "On branch main");
    }

    #[test]
    fn echo_slice_falls_back_when_absent() {
        let text = "unrelated output";
        assert_eq!(strip_command_echo(text, "git status"), "unrelated output");
    }

    #[test]
    fn trailing_powershell_prompt_dropped() {
        let text = "output line\nPS D:\\Programming\\Projects>";
        assert_eq!(strip_trailing_prompt(text), "output line");
    }

    #[test]
    fn trailing_bash_prompt_dropped() {
        let text = "output line\nuser@host:~/dir$";
        assert_eq!(strip_trailing_prompt(text), "output line");
    }

    #[test]
    fn output_line_ending_in_angle_bracket_kept() {
        let text = "<html>\n<div>";
        assert_eq!(strip_trailing_prompt(text), "<html>\n<div>");
    }

    #[test]
    fn full_pipeline_on_conpty_noise() {
        let raw = "\x1b[?25l\x1b[HPS D:\\proj> \x1b[93mgit\x1b[m log \x1b[90m--oneline\x1b[m -2\x1b[K\
                   \x1b[?25h\x1b[HPS D:\\proj> \x1b[93mgit\x1b[m log \x1b[90m--oneline\x1b[m -2\x1b[K\n\
                   \x1b[33mdc717db\x1b[m first commit\n\x1b[33m49df4fe\x1b[m second commit\n\
                   PS D:\\proj> ";
        let cleaned = clean_command_output(raw, "git log --oneline -2");
        assert_eq!(cleaned, "dc717db first commit\n49df4fe second commit");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(clean_command_output("", "git status"), "");
    }
}
