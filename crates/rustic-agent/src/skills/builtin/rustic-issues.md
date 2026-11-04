---
name: rustic-issues
description: Search, read and file issues on Rustic's public issue tracker (rustic.avijitbhuin.com/issues) — no GitHub account needed. Use when you hit a bug in Rustic itself while working (a tool misbehaving, a crash, wrong UI state), when the user wants to report a bug or request a feature for Rustic, or asks what known issues exist. Always get the user's OK before filing.
---

# Rustic issue tracker

Rustic has a public tracker at **https://rustic.avijitbhuin.com/issues/** with a JSON API at `https://rustic.avijitbhuin.com/api/issues`. Anyone can read and file issues; no account or token is needed. Issues are **public**.

Only file issues about **Rustic itself** (the editor, its agent, tools, terminals, sync, server). Bugs in the user's own project don't belong here.

## Read

- List open issues: `GET /api/issues?status=open` (`status` = `open` | `closed` | `all`; optional `q` = search text, `page`, `per_page` ≤ 100). Returns `{ issues: [{id, title, kind, status, source, excerpt, created_at, closed_at, attachments}], total, counts }`.
- One issue: `GET /api/issues/<id>`. Returns the full `body`, `status`, `resolution` (when closed) and `attachments` (`url` paths you can fetch).

Use `web_fetch` or `curl -s` for reads. Times are unix seconds.

## File an issue — workflow

1. **Search first** for duplicates: `GET /api/issues?status=all&q=<key words>`. If one matches, show it to the user instead of filing a new one.
2. **Draft** it: a specific title (4–160 chars); a body with what happened, what was expected, steps to reproduce, and the relevant error text; the Rustic version and OS if known. Kind is `bug`, `feature` or `question`.
3. **Scrub** the draft: no API keys, tokens, passwords, emails or private source code. Trim long logs to the relevant lines. Replace private paths/usernames with placeholders.
4. **Ask before posting.** Call `ask_user` with the full drafted title, kind, body and the list of files you'd attach, and options to post, edit or cancel. Never file without an explicit OK. If the user edits the draft, use their version.
5. **Post** with `curl` (multipart, so files can ride along), then give the user the issue link from the response.

```bash
curl -sS -X POST https://rustic.avijitbhuin.com/api/issues \
  -F "title=Terminal stays 'running' after the command exits on macOS" \
  -F "kind=bug" \
  -F "source=agent" \
  -F "app_version=0.7.9" \
  -F "os=macOS 15.1 (arm64)" \
  -F "body=<<.rustic/tmp/issue-body.md" \
  -F "files=@.rustic/tmp/terminal-log.txt"
```

- On **Windows** call `curl.exe` explicitly (in PowerShell, `curl` is an alias for `Invoke-WebRequest`).
- For a long body, write it to a file under `.rustic/tmp/` and pass `-F "body=<<path"` (`<` reads the file as the field's text). Delete the scratch file afterwards.
- Text-only issues can also be posted as JSON: `-H "Content-Type: application/json" -d '{"title": "...", "body": "...", "kind": "bug", "source": "agent"}'`.
- A success is HTTP 201 with `{ "id": 42, "url": "/issues/?id=42" }` — the link is `https://rustic.avijitbhuin.com/issues/?id=42`.

## Limits (server-enforced)

- Up to **5 attachments**, **10 MB each**. Accepted: png, jpg/jpeg, gif, webp, pdf, and text files (txt, log, md, json, csv, yaml/yml, toml). The content must match the extension.
- Body up to 20,000 characters.
- **10 new issues per hour** per network. HTTP 429 means wait; don't retry in a loop.
- Leave the `website` field out — it's a spam trap and anything sent in it is silently discarded.

## Don'ts

- Don't file without the user's approval, and don't file several issues for one problem.
- Don't attach the user's project files unless they asked for that specific file to be included.
- Issues can't be closed or edited through the API; maintainers handle that.
