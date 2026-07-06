//! Probe whether Claude Code engages its fullscreen (alt-screen) renderer
//! inside Rustic's exact PTY environment.
//!
//! Usage: cargo run -p rustic-terminal --example fullscreen_probe -- [plain|noflicker|vscode]
//!
//! Spawns `claude` in a ConPTY with the same env Rustic's PtySession sets,
//! captures ~20s of raw output, and reports which renderer-relevant escape
//! sequences appeared (alt-screen 1049, synchronized output 2026, focus 1004).

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rustic_terminal::emulator::TerminalEmulator;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "plain".into());

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 70,
            cols: 138,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let claude = format!(
        "{}\\.local\\bin\\claude.exe",
        std::env::var("USERPROFILE").expect("USERPROFILE")
    );
    let mut cmd = CommandBuilder::new(&claude);
    if mode == "debug" {
        cmd.arg("--debug");
    }
    // *cont modes replay an existing multi-screen conversation so we can watch
    // what claude emits when rendering content taller than the viewport.
    if mode.ends_with("cont") {
        cmd.arg("--continue");
        cmd.cwd("D:\\Programming\\Projects\\Personal\\linkedin_api");
    } else {
        cmd.cwd("D:\\Programming\\Projects\\Personal\\Rustic");
    }

    // The probe itself may run inside a Claude Code session; scrub inherited
    // CLAUDE* vars so the child doesn't see itself as nested/managed.
    for (key, _) in std::env::vars() {
        if key.starts_with("CLAUDE") {
            cmd.env_remove(&key);
        }
    }

    // Mirror PtySession::new's capability env exactly.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "rustic");
    cmd.env("TERM_PROGRAM_VERSION", "0.4.0");

    match mode.as_str() {
        "noflicker" | "debug" => {
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }
        "vscode" => {
            cmd.env("TERM_PROGRAM", "vscode");
            cmd.env("TERM_PROGRAM_VERSION", "1.100.0");
        }
        "vscode2" => {
            cmd.env("TERM_PROGRAM", "vscode");
            cmd.env("TERM_PROGRAM_VERSION", "1.100.0");
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }
        "wt" => {
            cmd.env("WT_SESSION", "4d6e9a51-0000-4b00-9a51-000000000000");
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }
        // rustic identity, but force synchronized output + fullscreen renderer.
        "forcesync" => {
            cmd.env("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1");
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }
        // rustic identity, force sync output, but CLASSIC renderer (alt screen
        // disabled) — this is the candidate fix for "no scrollback".
        "disablealt" => {
            cmd.env("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1");
            cmd.env("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1");
        }
        // Replay an existing conversation in fullscreen-believing mode.
        "fscont" => {
            cmd.env("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1");
            cmd.env("CLAUDE_CODE_NO_FLICKER", "1");
        }
        // Replay an existing conversation in CLASSIC mode.
        "classiccont" => {
            cmd.env("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1");
            cmd.env("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN", "1");
        }
        _ => {}
    }

    let mut child = pair.slave.spawn_command(cmd).expect("spawn claude");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("writer")));
    let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let queries: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_w = captured.clone();
    let queries_w = queries.clone();
    let writer_r = writer.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            captured_w.lock().unwrap().extend_from_slice(chunk);

            // Answer terminal queries the way a real xterm.js would, so
            // capability probing doesn't stall claude's renderer decision.
            let s = String::from_utf8_lossy(chunk).to_string();
            let mut log = queries_w.lock().unwrap();
            let mut reply = Vec::new();
            if s.contains("\x1b[6n") {
                log.push("DSR cursor (CSI 6n)".into());
                reply.extend_from_slice(b"\x1b[1;1R");
            }
            if s.contains("\x1b[?2026$p") {
                log.push("DECRQM 2026".into());
                reply.extend_from_slice(b"\x1b[?2026;2$y");
            }
            if s.contains("\x1b[>c") || s.contains("\x1b[>0c") {
                log.push("secondary DA".into());
                reply.extend_from_slice(b"\x1b[>41;354;0c");
            } else if s.contains("\x1b[c") || s.contains("\x1b[0c") {
                log.push("primary DA".into());
                reply.extend_from_slice(b"\x1b[?62;22c");
            }
            if s.contains("\x1b]10;?") {
                log.push("OSC 10 fg query".into());
                reply.extend_from_slice(b"\x1b]10;rgb:cccc/cccc/cccc\x1b\\");
            }
            if s.contains("\x1b]11;?") {
                log.push("OSC 11 bg query".into());
                reply.extend_from_slice(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\");
            }
            if s.contains("\x1b[?u") {
                log.push("kitty keyboard query".into());
            }
            if !reply.is_empty() {
                let mut w = writer_r.lock().unwrap();
                let _ = w.write_all(&reply);
                let _ = w.flush();
            }
        }
    });

    // After claude settles into the REPL, ask it which renderer it picked:
    // send "/tui\r"; the handler prints "Current renderer: fullscreen|default".
    std::thread::sleep(Duration::from_secs(12));
    {
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(b"/tui");
        let _ = w.flush();
    }
    std::thread::sleep(Duration::from_millis(600));
    {
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(b"\r");
        let _ = w.flush();
    }

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(6) {
        std::thread::sleep(Duration::from_millis(500));
    }
    let _ = child.kill();
    std::thread::sleep(Duration::from_millis(500));

    let bytes = captured.lock().unwrap().clone();
    let hay = String::from_utf8_lossy(&bytes);

    let alt_on = hay.contains("\x1b[?1049h");
    let sync = hay.contains("\x1b[?2026");
    let focus = hay.contains("\x1b[?1004h");
    let bracketed = hay.contains("\x1b[?2004h");

    println!("--- fullscreen_probe mode={mode} ---");
    println!("captured_bytes={}", bytes.len());
    println!(
        "claude_alive (focus 1004h or paste 2004h): {}",
        focus || bracketed
    );
    println!("alt_screen_1049h (fullscreen renderer): {alt_on}");
    println!("sync_output_2026: {sync}");

    // Render what claude actually painted, via the same headless emulator the
    // agent screen-read tool uses, so a blocking startup dialog is visible.
    let mut emu = TerminalEmulator::new(138, 70);
    emu.advance(&bytes);

    // DECISIVE metric: how many lines ended up in scrollback (above the live
    // screen)? This is exactly what xterm would let you scroll back through.
    // Classic renderer should accumulate; a fullscreen-believing renderer that
    // repaints in place should leave ~0.
    let scrollback = emu.render_scrollback_ansi();
    let screen_now = emu.render_screen();
    let sb_lines = scrollback.lines().count();
    let scr_lines = screen_now.lines().count();
    println!(
        "EMULATOR scrollback_total_lines={} (screen={}, history_above≈{})",
        sb_lines,
        scr_lines,
        sb_lines.saturating_sub(scr_lines)
    );

    let screen = emu.render_screen();
    let trimmed: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    println!(
        "--- rendered screen ({} non-empty lines) ---",
        trimmed.len()
    );
    for line in trimmed {
        println!("{line}");
    }

    for line in hay.lines() {
        if line.to_lowercase().contains("fullscreen") || line.contains("renderer") {
            println!("[claude-log] {line}");
        }
    }

    // Every distinct DEC private mode set/reset claude emitted, e.g. ?1049h.
    let mut modes: Vec<String> = Vec::new();
    let b = &bytes;
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == 0x1b && b[i + 1] == b'[' && b[i + 2] == b'?' {
            let mut j = i + 3;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
                j += 1;
            }
            if j < b.len() && (b[j] == b'h' || b[j] == b'l') {
                let seq = String::from_utf8_lossy(&b[i + 2..=j]).to_string();
                if !modes.contains(&seq) {
                    modes.push(seq);
                }
            }
            i = j;
        }
        i += 1;
    }
    println!("dec_private_modes: {}", modes.join(" "));

    // Scroll-region (DECSTBM) and scrollback-clear detection: a scroll region
    // on the NORMAL buffer traps scrolled lines so they never reach scrollback;
    // ED 3 (\x1b[3J) wipes the scrollback buffer outright. Either explains
    // "can't scroll up to old content".
    let mut scroll_regions: Vec<String> = Vec::new();
    let mut ed3_count = 0;
    {
        let b = &bytes;
        let mut i = 0;
        while i + 1 < b.len() {
            if b[i] == 0x1b && b[i + 1] == b'[' {
                let mut j = i + 2;
                while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
                    j += 1;
                }
                if j < b.len() {
                    let params = String::from_utf8_lossy(&b[i + 2..j]).to_string();
                    match b[j] {
                        b'r' => {
                            let seq = format!("CSI {}r", params);
                            if !scroll_regions.contains(&seq) {
                                scroll_regions.push(seq);
                            }
                        }
                        b'J' if params == "3" => ed3_count += 1,
                        _ => {}
                    }
                }
                i = j;
            }
            i += 1;
        }
    }
    println!("scroll_regions (DECSTBM): {:?}", scroll_regions);
    println!("scrollback_clears (ED 3J): {ed3_count}");
    println!("queries_seen: {:?}", queries.lock().unwrap());

    if let Some(idx) = hay.find("Current renderer") {
        let tail: String = hay[idx..].chars().take(40).collect();
        let line = tail.lines().next().unwrap_or("");
        println!("tui_self_report: {line}");
    } else {
        println!("tui_self_report: <not found>");
    }
}
