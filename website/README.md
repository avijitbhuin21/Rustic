# Rustic website

Marketing site for Rustic. A standalone axum crate (not part of the root Cargo
workspace) that serves the hand-written static site in `static/`.

## Run locally

```powershell
cd website
cargo run
# → http://localhost:8080
```

`PORT` (default `8080`), `HOST` (default `0.0.0.0`) and `WEBSITE_STATIC_DIR`
(default `./static`) are the only knobs.

## Deploy on Railway

Create a service from this repo and set its **Root Directory** to `website`.
`railway.json` + `Dockerfile` in this folder take over from there; the
healthcheck hits `/healthz`.

## Editing

- `static/index.html` — copy and structure
- `static/styles.css` — theme tokens at the top, sections below
- `static/main.js` — split-text, reveals, cursor, magnetic buttons, tilt, OS detection

No build step: edit and refresh.
