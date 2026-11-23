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
    let last_line = cmd.lines().last().unwrap_or(cmd).trim();
    let lines: Vec<&str> = text.lines().collect();
    let seeded = format!("$ {}", first_line);
    // Long commands wrap across several pty rows (and ConPTY repaints the
    // prompt in between), so no single row ends with the whole command. The
    // last row of the echo still ends with the command's tail, and the first
    // row still contains its head — match on those fragments instead.
    let tail = tail_fragment(last_line, 24);
    let head = head_fragment(first_line, 24);

    let echo_at = lines.iter().rposition(|l| {
        let t = l.trim();
        t == first_line
            || t == seeded
            || (t.ends_with(first_line) && t.len() < first_line.len() + 200)
            || (!tail.is_empty() && t.ends_with(tail) && t.len() < last_line.len() + 200)
    });

    let echo_at = echo_at.or_else(|| {
        // No row ends with the tail (the shell cut the echo short). Skip the
        // leading run of prompt rows / rows that carry the command's head.
        if head.is_empty() {
            return None;
        }
        let mut idx = None;
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim();
            if t.is_empty() || is_prompt_line(t) || t.contains(head) {
                idx = Some(i);
            } else {
                break;
            }
        }
        idx
    });

    // A long command's echo wraps across pty rows, and line editors (zsh ZLE,
    // readline) redraw the wrap with `\r`/cursor moves that the line-based
    // sanitizer overlays into garbled residue rows (e.g. `js; ls …tail`).
    // Those rows sit right after the matched echo and still carry the
    // command's tail or head — skip past the last such row within the few
    // rows a wrapped echo can occupy.
    let echo_at = echo_at.map(|i| {
        if cmd.chars().count() < 40 {
            return i;
        }
        let window = (cmd.chars().count() / 40 + 2).min(10);
        let mut end = i;
        for (j, l) in lines.iter().enumerate().skip(i + 1).take(window) {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            if (!tail.is_empty() && t.contains(tail))
                || (!head.is_empty() && t.contains(head))
                || is_echo_residue(t, cmd)
            {
                end = j;
            }
        }
        end
    });

    match echo_at {
        Some(i) => lines[(i + 1)..]
            .join("\n")
            .trim_start_matches('\n')
            .to_string(),
        None => text.to_string(),
    }
}

/// Last `n` chars of `s` on a char boundary (whole string when shorter).
fn tail_fragment(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    let start = s.char_indices().nth(count - n).map(|(i, _)| i).unwrap_or(0);
    &s[start..]
}

/// First `n` chars of `s` on a char boundary (whole string when shorter).
fn head_fragment(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
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
/// the trailing prompt. Sentinel-wrapped commands (see `wrap_posix_command`)
/// are sliced exactly between their markers instead.
pub fn clean_command_output(raw: &str, cmd: &str) -> String {
    let sanitized = sanitize_terminal_output(raw);
    if let Some(body) = extract_sentinel_output(&sanitized) {
        return body.output;
    }
    if let Some(body) = output_before_end_marker(&sanitized) {
        return body;
    }
    let sliced = strip_command_echo(&sanitized, cmd);
    strip_trailing_prompt(&sliced)
}

/// True for a line that is exactly a sentinel end marker (`__RSTC_E_<nonce>_<exit code>`).
fn is_end_marker_line(line: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix(SENTINEL_END) else {
        return false;
    };
    let Some((nonce, code)) = rest.split_once('_') else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    digits(nonce) && digits(code.strip_prefix('-').unwrap_or(code))
}

/// Output of a finished sentinel-wrapped command whose begin marker has scrolled
/// out of the captured tail: everything before the closing end-marker line.
fn output_before_end_marker(sanitized: &str) -> Option<String> {
    let lines: Vec<&str> = sanitized.lines().collect();
    let end_at = lines.iter().rposition(|l| is_end_marker_line(l))?;
    // Only when the marker closes the buffer; anything after it but a prompt
    // belongs to a later command.
    if lines[end_at + 1..]
        .iter()
        .any(|l| !l.trim().is_empty() && !is_prompt_line(l))
    {
        return None;
    }
    Some(
        lines[..end_at]
            .join("\n")
            .trim_end_matches(['\n', ' '])
            .to_string(),
    )
}

const SENTINEL_BEGIN: &str = "__RSTC_B_";
const SENTINEL_END: &str = "__RSTC_E_";
/// The marker prefix as the wrapper's echo spells it (octal escapes).
const ECHO_MARKER_TEXT: &str = "\\137\\137RSTC_";
/// How the wrapper's echoed command line starts, up to the nonce.
const ECHO_BEGIN_TEXT: &str = "printf '\\137\\137RSTC_B_%s\\n' ";

/// Output of a sentinel-wrapped command, sliced between its markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentinelOutput {
    pub output: String,
    /// `None` while the command is still running (no end marker yet).
    pub exit_code: Option<i32>,
}

/// Wraps `cmd` for a POSIX shell so it runs as ONE unit (multi-line scripts no
/// longer hit `dquote>` continuation prompts) and its output is bracketed by
/// markers. The markers are printed via octal escapes, so the echoed command
/// line never contains the literal marker text and slicing is exact even when
/// the echo wraps or is redrawn.
pub fn wrap_posix_command(cmd: &str, nonce: u64) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(cmd.as_bytes());
    format!(
        " {ECHO_BEGIN_TEXT}{nonce}; eval \"$(printf %s '{b64}' | base64 -d)\"; printf '\\n\\137\\137RSTC_E_%s_%d\\n' {nonce} \"$?\""
    )
}

/// Finds the newest begin marker in sanitized output and returns what follows
/// it, up to the matching end marker when present.
pub fn extract_sentinel_output(sanitized: &str) -> Option<SentinelOutput> {
    let begin_at = sanitized
        .rmatch_indices(SENTINEL_BEGIN)
        .find_map(|(i, _)| {
            let line_start = i == 0 || sanitized[..i].ends_with('\n');
            line_start.then_some(i)
        })?;
    let after_marker = &sanitized[begin_at + SENTINEL_BEGIN.len()..];
    let nonce: String = after_marker
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if nonce.is_empty() {
        return None;
    }
    let body_start = after_marker
        .find('\n')
        .map(|n| n + 1)
        .unwrap_or(after_marker.len());
    let body = &after_marker[body_start..];
    let end_tag = format!("{SENTINEL_END}{nonce}_");
    let (output, exit_code) = match body.find(&end_tag) {
        Some(e) => {
            let code: String = body[e + end_tag.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            (&body[..e], code.parse::<i32>().ok())
        }
        None => (body, None),
    };
    let mut out = output.trim_end_matches(['\n', ' ']).to_string();
    if exit_code.is_none() {
        out = strip_trailing_prompt(&out);
    }
    Some(SentinelOutput {
        output: out,
        exit_code,
    })
}

/// Hides the sentinel wrapper's echo and marker lines from terminal text, one
/// line at a time. Only catches an echo line that still holds the marker text
/// in one piece; use `clean_terminal_scrollback` / `clean_rendered_screen`.
pub fn hide_sentinels(text: &str) -> String {
    text.lines()
        .filter(|l| {
            !l.contains(ECHO_MARKER_TEXT)
                && !l.trim_start().starts_with(SENTINEL_BEGIN)
                && !l.trim_start().starts_with(SENTINEL_END)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Readable scrollback for `read_terminal_output` from a raw pty tail, with every sentinel wrapper echo and marker line removed.
pub fn clean_terminal_scrollback(raw: &str) -> String {
    hide_sentinels(&sanitize_terminal_output(&strip_wrapper_echoes(raw)))
}

/// Last `max_bytes` of `text`, starting at a line boundary when the cut drops anything.
pub fn tail_lines(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    match text[start..].find('\n') {
        Some(n) if start + n + 1 < text.len() => &text[start + n + 1..],
        _ => &text[start..],
    }
}

/// Index of the start of the line that contains byte `i` of `text`.
fn line_start(text: &str, i: usize) -> usize {
    text[..i].rfind('\n').map_or(0, |p| p + 1)
}

/// Removes each sentinel wrapper's echoed command line and begin-marker line from a raw pty tail.
///
/// This has to happen before `sanitize_terminal_output`: the shell's line
/// editor wraps the long echo with `\r`, and resolving those overlays the rows
/// into one garbled line. When a wrap falls inside the marker text, that line
/// no longer contains it and no line filter can recognise it (tracker #17).
/// In the raw stream the echo is still intact: it runs from the wrapper's
/// opening `printf` to the begin marker the shell prints next.
fn strip_wrapper_echoes(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut copied = 0usize;
    let mut search = 0usize;
    while let Some(rel) = raw[search..].find(SENTINEL_BEGIN) {
        let at = search + rel;
        let digits_at = at + SENTINEL_BEGIN.len();
        search = digits_at;
        let digits = bytes[digits_at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let after = digits_at + digits;
        let own_line = (at == 0 || bytes[at - 1] == b'\n')
            && matches!(bytes.get(after), None | Some(b'\r') | Some(b'\n'));
        if digits == 0 || !own_line || at < copied {
            continue;
        }
        let region = &raw[copied..at];
        let Some(echo_start) = wrapper_echo_start(region, &raw[digits_at..after]) else {
            continue;
        };
        let marker_end = raw[after..].find('\n').map_or(raw.len(), |n| after + n + 1);
        out.push_str(&region[..echo_start]);
        copied = marker_end;
        search = marker_end;
    }
    let rest = &raw[copied..];
    // A wrapper that was typed but has not printed its begin marker yet. A
    // line editor adds at most a couple of newlines to an echo, so anything
    // further back is output that merely quotes the wrapper.
    match rest.rfind(ECHO_BEGIN_TEXT) {
        Some(i) if rest[i..].matches('\n').count() <= 2 => {
            out.push_str(&rest[..line_start(rest, i)])
        }
        _ => out.push_str(rest),
    }
    out
}

/// Where the wrapper echo for the begin marker `nonce` starts inside `region` (the raw text before that marker).
fn wrapper_echo_start(region: &str, nonce: &str) -> Option<usize> {
    if let Some(i) = region.rfind(&format!("{ECHO_BEGIN_TEXT}{nonce};")) {
        return Some(line_start(region, i));
    }
    // The echo's opening was split by a wrap or lies before the captured tail.
    // The shell prints the begin marker right after the echoed line, so the
    // physical line before the marker is the echo (or what is left of it).
    let body = region.strip_suffix('\n').unwrap_or(region);
    (!body.is_empty()).then(|| line_start(body, body.len()))
}

/// True for a row that is exactly a sentinel begin marker (`__RSTC_B_<nonce>`).
fn is_begin_marker_line(line: &str) -> bool {
    line.trim()
        .strip_prefix(SENTINEL_BEGIN)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// True for a rendered row that can only be a middle or closing row of a wrapped wrapper echo.
fn is_wrapper_echo_row(row: &str) -> bool {
    let t = row.trim();
    if t.contains(ECHO_MARKER_TEXT) || t.contains("base64 -d)\"") {
        return true;
    }
    // A long base64 run that fills the row, or is bounded only by the quotes
    // the wrapper puts around its payload.
    let is_b64 = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=');
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_b64(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_b64(bytes[i]) {
            i += 1;
        }
        if i - start >= 40 {
            let opens = start == 0 || bytes[start - 1] == b'\'';
            let closes = i == bytes.len() || bytes[i] == b'\'';
            return opens && closes;
        }
    }
    false
}

/// Hides sentinel wrapper rows from a rendered screen (`read_terminal_output` with `rendered: true`).
///
/// On the emulator grid a wrapped echo is several rows, and only the rows that
/// hold the marker text are recognisable alone. The echo of a command is every
/// row from its opening `printf` down to the begin marker.
pub fn clean_rendered_screen(screen: &str) -> String {
    let rows: Vec<&str> = screen.lines().collect();
    let mut hide: Vec<bool> = rows
        .iter()
        .map(|r| {
            r.contains(ECHO_MARKER_TEXT)
                || r.trim_start().starts_with(SENTINEL_BEGIN)
                || r.trim_start().starts_with(SENTINEL_END)
        })
        .collect();
    let is_marker = |r: &str| is_begin_marker_line(r) || is_end_marker_line(r);
    let mut last_begin = None;
    for i in 0..rows.len() {
        if !is_begin_marker_line(rows[i]) {
            continue;
        }
        last_begin = Some(i);
        let echo_first = (0..i)
            .rev()
            .take_while(|&k| !is_marker(rows[k]))
            .find(|&k| rows[k].contains(ECHO_BEGIN_TEXT));
        match echo_first {
            Some(first) => hide[first..i].iter_mut().for_each(|h| *h = true),
            None => {
                // The echo's first row scrolled off the screen (or was split by
                // a wrap). The row right above a begin marker is always the
                // echo's last row; above it, keep going while rows are plainly
                // part of the wrapper.
                let mut k = i;
                while k > 0 && rows[k - 1].trim().is_empty() {
                    k -= 1;
                    hide[k] = true;
                }
                if k > 0 && !is_marker(rows[k - 1]) {
                    k -= 1;
                    hide[k] = true;
                    while k > 0 && is_wrapper_echo_row(rows[k - 1]) {
                        k -= 1;
                        hide[k] = true;
                    }
                }
            }
        }
    }
    // A wrapper that was typed but has not printed its begin marker yet.
    if let Some(first) = rows.iter().rposition(|r| r.contains(ECHO_BEGIN_TEXT)) {
        if last_begin.is_none_or(|b| b < first) {
            hide[first..].iter_mut().for_each(|h| *h = true);
        }
    }
    rows.iter()
        .zip(&hide)
        .filter(|(_, hidden)| !**hidden)
        .map(|(row, _)| *row)
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim_line(buf: &[char]) -> String {
    let s: String = buf.iter().collect();
    s.trim_end().to_string()
}

/// A row made (almost) entirely of fragments of `cmd` — the overlay left when
/// a line editor's wrapped-echo redraw is flattened. ≥80% of its
/// non-whitespace chars must belong to tokens of ≥3 chars found in `cmd`.
fn is_echo_residue(line: &str, cmd: &str) -> bool {
    let mut total = 0usize;
    let mut matched = 0usize;
    for tok in line.split_whitespace() {
        let n = tok.chars().count();
        total += n;
        if n >= 3 && cmd.contains(tok) {
            matched += n;
        }
    }
    total >= 12 && matched * 5 >= total * 4
}

fn is_prompt_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t == "$" || t == "#" || t == ">" || t == "%" {
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
        // zsh default `user@host dir %` (macOS), and `user@host dir $`.
        let words = t.split_whitespace().count();
        if !user.is_empty()
            && !user.contains(char::is_whitespace)
            && (2..=4).contains(&words)
            && (t.ends_with(" %") || t.ends_with(" $") || t.ends_with(" #"))
        {
            return true;
        }
        // Git Bash header line `user@HOST MINGW64 /c/path` (the `$` sits on the next line).
        let mut parts = t.split_whitespace();
        let _ = parts.next();
        if let Some(env) = parts.next() {
            if ["MINGW", "MSYS", "UCRT", "CLANG"]
                .iter()
                .any(|p| env.starts_with(p))
                && words <= 4
            {
                return true;
            }
        }
    }
    // macOS bash default `host:dir user$` / bare `bash-3.2$`.
    if t.ends_with('$') && t.split_whitespace().count() <= 2 {
        let first = t.split_whitespace().next().unwrap_or("");
        if first.contains(':') || first.starts_with("bash-") || first.starts_with("sh-") {
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
    fn echo_slice_handles_wrapped_conpty_repaints() {
        let cmd = "cargo check --workspace 2>&1 | Select-String -Pattern '^(error|warning)' -Context 0,8 | Select-Object -First 60; \"CARGO_DONE\"";
        let text = "PS D:\\p> Set-ExecutionPolicy -Scope Process Bypass -Force; Clear-Host\n\
                    PS D:\\p>\n\n\
                    PS D:\\p> cargo check --workspace 2>&1 | Select-String -Pattern '^(error|warning)' -Co\n\
                    ntext 0,8 | Select-Object -PS D:\\p> cargo check --workspace 2>&1 | Select-String -Pattern '^(error|warning)' -Co\n\
                    ntext 0,8 | Select-Object -First 60; \"CARGO_DONE\"\n\
                    CARGO_DONE";
        assert_eq!(strip_command_echo(text, cmd), "CARGO_DONE");
    }

    #[test]
    fn sentinel_slices_exact_output_despite_wrapped_echo() {
        // Echo of the wrapper wraps and repaints (the tracker #3 garbling);
        // the marker text only exists in real output, so slicing is exact.
        let raw = "% \u{1b}[1m printf '\\137\\137RSTC_B_%s\\n' 7; eval \"$(printf %s 'Zm9y' | ba\rse64 -d)\"; printf '\\n\\137\\137RSTC_E_%s_%d\\n' 7 \"$?\"\n\
                   __RSTC_B_7\n== a\nresult a\n== b\nresult b\n\n__RSTC_E_7_0\n% ";
        let out = clean_command_output(raw, "for q in a b; do …; done");
        assert_eq!(out, "== a\nresult a\n== b\nresult b");
        let s = extract_sentinel_output(&sanitize_terminal_output(raw)).unwrap();
        assert_eq!(s.exit_code, Some(0));
    }

    #[test]
    fn sentinel_reports_nonzero_exit_and_running_state() {
        let done = "__RSTC_B_3\nboom\n\n__RSTC_E_3_2\n$ ";
        let s = extract_sentinel_output(done).unwrap();
        assert_eq!((s.output.as_str(), s.exit_code), ("boom", Some(2)));
        let running = "__RSTC_B_4\nstep 1\nstep 2";
        let s = extract_sentinel_output(running).unwrap();
        assert_eq!((s.output.as_str(), s.exit_code), ("step 1\nstep 2", None));
    }

    #[test]
    fn sentinel_uses_newest_command_only() {
        let raw = "__RSTC_B_1\nold\n\n__RSTC_E_1_0\n__RSTC_B_2\nnew\n\n__RSTC_E_2_0\n";
        assert_eq!(extract_sentinel_output(raw).unwrap().output, "new");
    }

    #[test]
    fn wrapper_is_single_line_and_round_trips_multiline_scripts() {
        let cmd = "python3 - <<'EOF'\nprint(\"hi\")\nEOF";
        let w = wrap_posix_command(cmd, 9);
        assert!(!w.contains('\n'), "must be typed as one line");
        assert!(
            !w.contains(SENTINEL_BEGIN),
            "echo must not contain the literal marker"
        );
        assert!(hide_sentinels(&format!("$ {w}\n__RSTC_B_9\nhi\n__RSTC_E_9_0")).trim() == "hi");
    }

    #[test]
    fn end_marker_stripped_when_begin_marker_scrolled_out_of_tail() {
        // Tracker #10: a 4 KB tail of long output has no begin marker, so the
        // end marker used to be handed to the model as the last output line.
        let raw = "…middle of a long listing\nlast real line\n\n__RSTC_E_9_0\nuser@host Rustic % ";
        assert_eq!(
            clean_command_output(raw, "python3 report.py"),
            "…middle of a long listing\nlast real line"
        );
        let failed = "partial\nboom\n\n__RSTC_E_12_-1\n";
        assert_eq!(clean_command_output(failed, "false"), "partial\nboom");
    }

    #[test]
    fn end_marker_fallback_ignores_lookalikes_and_older_commands() {
        // Output that merely mentions the marker is kept as output.
        let text = "see __RSTC_E_ in the docs\n__RSTC_E_x_0";
        assert_eq!(clean_command_output(text, "cat notes"), text);
        // A marker followed by a later command's output is not this command's end.
        let later = "old tail\n__RSTC_E_3_0\n$ echo hi\nhi";
        assert_eq!(clean_command_output(later, "echo hi"), "hi");
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

    #[test]
    fn zsh_wrapped_echo_residue_dropped() {
        // Observed on macOS zsh (120 cols): seed line, then the ZLE wrap
        // redraw flattened into one garbled row, then real output + prompt.
        let cmd = "cd /Volumes/AcasisSSDDrive/projects/Personal/Rustic && ls crates/rustic-terminal/src | tr '\\n' ' '; echo; wc -l src-tauri/src/commands/shell_integration.rs rustic-server/src/commands/terminal.rs 2>&1; git --no-pager diff crates/rustic-terminal/src/pty.rs";
        let raw = "avijit@Avijits-MacBook-Air Rustic %\n\
                   $ cd /Volumes/AcasisSSDDrive/projects/Personal/Rustic && ls crates/rustic-terminal/src | tr '\\n' ' '; echo; wc -l src-tauri/src/commands/shell_integration.rs rustic-server/src/commands/terminal.rs 2>&1; git --no-pager diff crates/rustic-terminal/src/pty.rs\n\
                   r/src/commands/terminal.rs 2>&1; git --no-pager diff crates/rustic-terminal/src/pty.rs/shell_integration.rs rustic-serve\n\
                   emulator.rs lib.rs pty.rs shell.rs\n\
                   avijit@Avijits-MacBook-Air Rustic %";
        assert_eq!(
            clean_command_output(raw, cmd),
            "emulator.rs lib.rs pty.rs shell.rs"
        );
    }

    #[test]
    fn short_command_output_not_eaten_by_residue_logic() {
        let raw = "$ echo hello world\nhello world\nuser@host dir %";
        assert_eq!(clean_command_output(raw, "echo hello world"), "hello world");
    }

    #[test]
    fn long_command_keeps_unrelated_output_rows() {
        let cmd = "cargo test -p rustic-agent terminal_text -- --nocapture --test-threads=1";
        let raw = format!("$ {cmd}\nrunning 3 tests\ntest ok\n");
        assert_eq!(clean_command_output(&raw, cmd), "running 3 tests\ntest ok");
    }

    #[test]
    fn zsh_and_macos_bash_prompts_dropped() {
        assert_eq!(strip_trailing_prompt("out\nuser@host Rustic %"), "out");
        assert_eq!(strip_trailing_prompt("out\nhost:Rustic user$"), "out");
        assert_eq!(strip_trailing_prompt("out\nbash-3.2$"), "out");
        assert_eq!(strip_trailing_prompt("out\n%"), "out");
        // Git Bash: header line + `$` on its own line.
        assert_eq!(
            strip_trailing_prompt("out\nuser@HOST MINGW64 /c/proj\n$"),
            "out"
        );
        assert_eq!(
            strip_trailing_prompt("out\nuser@HOST MSYS ~/proj (main)\n$"),
            "out"
        );
        // A normal sentence ending in % is output, not a prompt.
        assert_eq!(
            strip_trailing_prompt("coverage is now at 95 %"),
            "coverage is now at 95 %"
        );
    }

    const PROMPT: &str = "user@host proj % ";
    const WRAPPER_TOKENS: [&str; 5] = ["base64", "RSTC", "printf", "eval", "$?"];

    /// Bytes a shell's line editor writes while echoing `line`, typed after a
    /// `prompt_cols`-wide prompt in a `cols`-wide terminal. Modelled on real
    /// captures: readline ends a full row with ` \r`; zsh (`zle`) follows that
    /// with erase-line, the next row's first character and another `\r`.
    fn echo_bytes(line: &str, prompt_cols: usize, cols: usize, zle: bool) -> String {
        let mut out = String::new();
        if zle {
            out.push_str(" \u{8}");
        }
        let mut col = prompt_cols;
        for c in line.chars() {
            if col == cols {
                out.push_str(" \r");
                if zle {
                    out.push_str("\u{1b}[K");
                    out.push(c);
                    out.push('\r');
                }
                col = 0;
            }
            out.push(c);
            col += 1;
        }
        out.push_str(if zle { "\u{1b}[?2004l\r\r\n" } else { "\r\n" });
        out
    }

    /// Raw buffer of one finished wrapped command: prompt, the host's `$ cmd`
    /// seed, the echoed wrapper, both markers around `output`, next prompt.
    fn wrapped_run(cmd: &str, nonce: u64, output: &str, prompt_cols: usize, zle: bool) -> String {
        let echo = echo_bytes(&wrap_posix_command(cmd, nonce), prompt_cols, 120, zle);
        format!(
            "{PROMPT}\n$ {cmd}\n{echo}__RSTC_B_{nonce}\r\n{output}\r\n\r\n__RSTC_E_{nonce}_0\r\n{PROMPT}"
        )
    }

    fn cmd_of_len(len: usize) -> String {
        format!("echo {}", "x".repeat(len - 5))
    }

    /// Tracker issue #17, as reproduced in the app: an 80-character command
    /// after a 35-column zsh prompt puts the wrap inside the end marker text,
    /// so the flattened echo line no longer contains it.
    #[test]
    fn wrapper_echo_split_inside_marker_text_is_hidden() {
        let cmd = "echo repro17-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456";
        let raw = wrapped_run(cmd, 414, "repro17-out", 35, true);

        // The old line filter leaks exactly what the report shows.
        let old = hide_sentinels(&sanitize_terminal_output(&raw));
        assert!(
            old.contains("| base64 -d)\"; printf '\\n\\137\\137"),
            "{old}"
        );

        assert_eq!(
            clean_terminal_scrollback(&raw),
            format!("user@host proj %\n$ {cmd}\nrepro17-out\n\nuser@host proj %")
        );
    }

    /// Whatever the command length and prompt width, and whichever line
    /// editor echoed it, nothing of the wrapper survives and the command's
    /// output is intact.
    #[test]
    fn wrapper_echo_is_hidden_wherever_the_line_wraps() {
        for zle in [false, true] {
            for prompt_cols in [10, 17, 35, 60, 86, 95, 110, 119] {
                for len in 6..=420 {
                    let cmd = cmd_of_len(len);
                    let raw = wrapped_run(&cmd, 9, "OUT", prompt_cols, zle);
                    assert_eq!(
                        clean_terminal_scrollback(&raw),
                        format!("user@host proj %\n$ {cmd}\nOUT\n\nuser@host proj %"),
                        "zle={zle} prompt_cols={prompt_cols} len={len}"
                    );
                }
            }
        }
    }

    /// The byte tail can start anywhere inside the echo.
    #[test]
    fn wrapper_echo_is_hidden_when_the_tail_starts_inside_it() {
        for zle in [false, true] {
            let cmd = cmd_of_len(300);
            let raw = wrapped_run(&cmd, 9, "OUT", 17, zle);
            let echo_from = raw.find(ECHO_BEGIN_TEXT).unwrap();
            let echo_to = raw.find("__RSTC_B_9").unwrap();
            for start in (echo_from + 1..echo_to).step_by(7) {
                let cleaned = clean_terminal_scrollback(&raw[start..]);
                assert_eq!(
                    cleaned, "OUT\n\nuser@host proj %",
                    "zle={zle} start={start}"
                );
            }
        }
    }

    /// zsh scrolls its editor for very long lines: the echo then holds a
    /// newline and cursor moves (seen with 3000+ character commands).
    #[test]
    fn wrapper_echo_with_an_inner_newline_is_hidden() {
        let cmd = cmd_of_len(400);
        let echo = echo_bytes(&wrap_posix_command(&cmd, 9), 17, 120, true);
        let mid = echo.len() / 2;
        let echo = format!("{}\r\n\u{1b}[29A\u{1b}[102D{}", &echo[..mid], &echo[mid..]);
        let raw =
            format!("{PROMPT}\n$ {cmd}\n{echo}__RSTC_B_9\r\nOUT\r\n\r\n__RSTC_E_9_0\r\n{PROMPT}");
        assert_eq!(
            clean_terminal_scrollback(&raw),
            format!("user@host proj %\n$ {cmd}\nOUT\n\nuser@host proj %")
        );
    }

    #[test]
    fn scrollback_keeps_every_command_and_its_output() {
        let first = cmd_of_len(140);
        let second = cmd_of_len(97);
        let raw = format!(
            "{}\n{}",
            wrapped_run(&first, 1, "first-out", 17, true),
            // Still running: begin marker printed, no end marker yet.
            wrapped_run(&second, 2, "second-out", 17, false)
                .split("\r\n\r\n__RSTC_E_2_")
                .next()
                .unwrap()
        );
        assert_eq!(
            clean_terminal_scrollback(&raw),
            format!(
                "user@host proj %\n$ {first}\nfirst-out\n\nuser@host proj %\nuser@host proj %\n$ {second}\nsecond-out"
            )
        );
    }

    #[test]
    fn wrapper_typed_but_not_started_is_hidden() {
        let cmd = cmd_of_len(200);
        for zle in [false, true] {
            let echo = echo_bytes(&wrap_posix_command(&cmd, 9), 17, 120, zle);
            for cut in [echo.len(), echo.len() - 3, echo.len() / 2] {
                let raw = format!("{PROMPT}\n$ {cmd}\n{}", &echo[..cut]);
                assert_eq!(
                    clean_terminal_scrollback(&raw),
                    format!("user@host proj %\n$ {cmd}"),
                    "zle={zle} cut={cut}"
                );
            }
        }
    }

    #[test]
    fn scrollback_without_a_wrapper_is_only_sanitized() {
        let raw = "PS D:\\p> \u{1b}[93mgit\u{1b}[m status\r\nOn branch main\r\nPS D:\\p> ";
        assert_eq!(
            clean_terminal_scrollback(raw),
            "PS D:\\p> git status\nOn branch main\nPS D:\\p>"
        );
        // Output that only looks like a marker line start is not an echo.
        let raw = "notes\n__RSTC_B_draft is a name\nmore";
        assert!(clean_terminal_scrollback(raw).contains("notes"));
    }

    #[test]
    fn tail_lines_cuts_at_a_line_boundary() {
        assert_eq!(tail_lines("abc\ndef", 100), "abc\ndef");
        assert_eq!(tail_lines("abc\ndef\nghi", 6), "ghi");
        assert_eq!(tail_lines("abcdefghi", 4), "fghi");
        assert_eq!(tail_lines("é\nab", 3), "ab");
    }

    /// Rows of the emulator grid after one finished wrapped command.
    fn rendered_run(cmd: &str, nonce: u64, output: &str) -> Vec<String> {
        let typed: Vec<char> = format!("{PROMPT}{}", &wrap_posix_command(cmd, nonce)[1..])
            .chars()
            .collect();
        let mut rows: Vec<String> = typed.chunks(120).map(|c| c.iter().collect()).collect();
        rows.push(format!("__RSTC_B_{nonce}"));
        rows.push(output.to_string());
        rows.push(String::new());
        rows.push(format!("__RSTC_E_{nonce}_0"));
        rows.push(PROMPT.trim_end().to_string());
        rows
    }

    /// `rendered: true` gets one grid row per wrap; the rows between the
    /// first and the last hold only base64 and used to be shown.
    #[test]
    fn rendered_screen_hides_every_row_of_a_wrapped_echo() {
        for len in 6..=420 {
            let cmd = cmd_of_len(len);
            let mut rows = vec!["earlier output".to_string()];
            rows.extend(rendered_run(&cmd, 9, "OUT"));
            let cleaned = clean_rendered_screen(&rows.join("\n"));
            assert_eq!(
                cleaned, "earlier output\nOUT\n\nuser@host proj %",
                "len={len}"
            );
        }
    }

    #[test]
    fn rendered_screen_hides_an_echo_whose_first_rows_scrolled_off() {
        for len in 100..=420 {
            let rows = rendered_run(&cmd_of_len(len), 9, "OUT");
            let echo_rows = rows.iter().position(|r| r == "__RSTC_B_9").unwrap();
            for gone in 1..echo_rows {
                let cleaned = clean_rendered_screen(&rows[gone..].join("\n"));
                assert_eq!(cleaned, "OUT\n\nuser@host proj %", "len={len} gone={gone}");
            }
        }
    }

    #[test]
    fn rendered_screen_keeps_earlier_commands_output() {
        let mut rows = rendered_run(&cmd_of_len(60), 1, "first-out");
        // The second command's echo start scrolled away with the first's echo.
        rows.extend(rendered_run(&cmd_of_len(300), 2, "second-out").split_off(1));
        let cleaned = clean_rendered_screen(&rows[1..].join("\n"));
        assert!(cleaned.contains("first-out"), "{cleaned}");
        assert!(cleaned.contains("second-out"), "{cleaned}");
        for token in WRAPPER_TOKENS {
            assert!(!cleaned.contains(token), "{token} in {cleaned}");
        }
    }

    #[test]
    fn rendered_screen_hides_a_wrapper_that_has_not_started() {
        let mut rows = vec!["earlier output".to_string()];
        let all = rendered_run(&cmd_of_len(300), 9, "OUT");
        let echo_rows = all.iter().position(|r| r == "__RSTC_B_9").unwrap();
        rows.extend(all.into_iter().take(echo_rows));
        assert_eq!(clean_rendered_screen(&rows.join("\n")), "earlier output");
        assert_eq!(clean_rendered_screen("plain\nscreen"), "plain\nscreen");
    }
}
