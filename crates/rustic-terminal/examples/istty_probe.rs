//! Check what Node sees for stdout/stdin.isTTY inside Rustic's PTY.
//!
//! Claude Code only writes its alt-screen (fullscreen) enter sequence when
//! `process.stdout.isTTY` is truthy. If portable-pty's ConPTY makes Node
//! report isTTY=false, fullscreen silently never engages. This isolates that.
//!
//! Usage: cargo run -p rustic-terminal --example istty_probe

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() {
    let node = "D:\\essential installs\\node\\node.exe";

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 70,
            cols: 138,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(node);
    cmd.arg("-e");
    cmd.arg(
        "console.log('stdout.isTTY=' + process.stdout.isTTY); \
         console.log('stdin.isTTY=' + process.stdin.isTTY); \
         console.log('stderr.isTTY=' + process.stderr.isTTY); \
         console.log('columns=' + process.stdout.columns + ' rows=' + process.stdout.rows);",
    );
    cmd.cwd("D:\\Programming\\Projects\\Personal\\Rustic");
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "rustic");

    let mut child = pair.slave.spawn_command(cmd).expect("spawn node");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let out: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let out_w = out.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            out_w.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });

    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(300));

    let bytes = out.lock().unwrap().clone();
    println!("{}", String::from_utf8_lossy(&bytes).trim());
}
