# Rustic work checklist

Open work only. Completed items are removed (git history keeps the record). Mark `[x]` when done, then delete the line in the next cleanup.

## v0.8.1 (security patch)

- [ ] Push and confirm CI green before tagging (locally green: `cargo audit` 0, `cargo deny check licenses sources` 0, `cargo fmt --all --check` 0, workspace check warning-free, app/agent/core/terminal/server/tauri tests pass, `vite build` ok)
- [ ] Drop the zip ignores in `.cargo/audit.toml` once docx-rs and tauri-plugin-updater move to zip >= 9

## Security follow-ups

- [ ] `bun audit`: 2 left, both with no upstream fix and only in the dev-only shadcn CLI — deepmerge 4.3.1 (latest release) and zod 3.25.76 (fix is zod 4, which breaks shadcn's `deepPartial`). Revisit when shadcn moves to zod 4

## Needs real-hardware verification

- [ ] Windows: agent terminals in Git Bash (detection, prompt stripping, completion)
- [ ] Two devices: peer sync over Wi-Fi switch, Cloudflare path, pairing pop-up bringing the window forward
- [ ] rustic-server: mid-run send keeps the in-flight turn (live check)
- [ ] Windows: a finished Git Bash command no longer counts as running (launcher → `usr\bin\bash.exe` resolution in `crates/rustic-terminal/src/pty.rs`)
- [ ] Anthropic: live auto-repair run where history ends on an assistant turn (no more "assistant message prefill" 400)
