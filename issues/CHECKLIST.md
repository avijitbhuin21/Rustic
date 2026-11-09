# Rustic work checklist

Open work only. Completed items are removed (git history keeps the record). Mark `[x]` when done, then delete the line in the next cleanup.

## Needs real-hardware verification

Logic for these is unit-tested, cross-compiled for `x86_64-pc-windows-msvc` and covered by `rustic-server/tests/peer_e2e.rs` (two local servers); only the physical-device parts are left.

- [ ] Windows: agent terminals in Git Bash (detection, prompt stripping, completion)
- [ ] Two devices: peer sync over Wi-Fi switch, Cloudflare path, pairing pop-up bringing the window forward
- [ ] Windows: a finished Git Bash command no longer counts as running (launcher → `usr\bin\bash.exe` resolution in `crates/rustic-terminal/src/pty.rs`)
