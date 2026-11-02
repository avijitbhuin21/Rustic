#!/usr/bin/env bash
# Build (if needed) and run rustic-server - the headless web build of Rustic -
# locally, then open it in a browser. bash port of run-server.ps1.
#
# Binds 127.0.0.1 by default: localhost is a browser "secure context", so
# clipboard and microphone work without HTTPS. --bind-all listens on 0.0.0.0
# (only behind a reverse proxy / VPN - see rustic-server/README.md).
#
# Usage:
#   ./scripts/run-server.sh
#   ./scripts/run-server.sh --password hunter2 --port 9000
#   ./scripts/run-server.sh --skip-web-build      # reuse existing dist/
#   ./scripts/run-server.sh --release             # optimized build
# Options: --password P  --port N  --data-dir D  --static-dir D  --release
#          --skip-web-build  --bind-all  --allow-insecure-password
set -euo pipefail

info() { printf '\033[36m==> %s\033[0m\n' "$*"; }
step() { printf '  - %s\n' "$*"; }
ok()   { printf '\033[32m  OK %s\033[0m\n' "$*"; }
warn() { printf '\033[33m  ! %s\033[0m\n' "$*"; }
die()  { printf '\033[31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

password="" port=8787 data_dir="" static_dir=""
release=0 skip_web=0 bind_all=0 allow_insecure=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --password) password="${2:-}"; shift 2 ;;
    --port) port="${2:-}"; shift 2 ;;
    --data-dir) data_dir="${2:-}"; shift 2 ;;
    --static-dir) static_dir="${2:-}"; shift 2 ;;
    --release) release=1; shift ;;
    --skip-web-build) skip_web=1; shift ;;
    --bind-all) bind_all=1; shift ;;
    --allow-insecure-password) allow_insecure=1; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
[[ -f package.json ]] || die "package.json not found at repo root"
data_dir="${data_dir:-$root/rustic-data}"
static_dir="${static_dir:-$root/dist}"

command -v cargo >/dev/null || die "'cargo' is not on PATH"
have_bun=0; command -v bun >/dev/null && have_bun=1

if [[ -z "$password" ]]; then
  if [[ -n "${RUSTIC_AUTH_PASSWORD:-}" ]]; then
    password="$RUSTIC_AUTH_PASSWORD"
  else
    password=rustic
    warn "No --password / RUSTIC_AUTH_PASSWORD set; using the dev default 'rustic'."
    warn "NEVER expose this server publicly with that password."
  fi
fi
if [[ $bind_all -eq 1 && "$password" == "rustic" && $allow_insecure -ne 1 ]]; then
  die "--bind-all with the default password 'rustic' is refused. Pass --password <strong>, or --allow-insecure-password on a trusted network."
fi

if [[ $skip_web -eq 1 ]]; then
  info "Skipping web build (--skip-web-build)"
  [[ -f "$static_dir/index.html" ]] || die "No built frontend at $static_dir. Run without --skip-web-build first."
elif [[ $have_bun -eq 0 ]]; then
  warn "'bun' is not on PATH - skipping web build."
  [[ -f "$static_dir/index.html" ]] || die "No built frontend at $static_dir and bun is unavailable."
  step "Reusing existing $static_dir"
else
  info "Building web frontend (bun run build:web) - this takes a couple of minutes"
  bun run build:web || die "web build failed"
  ok "dist/ built"
fi

if [[ $release -eq 1 ]]; then
  info "Building rustic-server (release)"
  cargo build --release -p rustic-server || die "cargo build (release) failed"
  binary="$root/target/release/rustic-server"
else
  info "Building rustic-server (debug)"
  cargo build -p rustic-server || die "cargo build failed"
  binary="$root/target/debug/rustic-server"
fi
[[ -x "$binary" ]] || die "server binary not found at $binary"
ok "server binary ready"

# Stable session secret so logins survive restarts: env > persisted file > new.
mkdir -p "$data_dir"
secret_file="$data_dir/.session-secret"
if [[ -n "${RUSTIC_SESSION_SECRET:-}" ]]; then
  session_secret="$RUSTIC_SESSION_SECRET"
  step "Using RUSTIC_SESSION_SECRET from the environment"
elif [[ -f "$secret_file" ]]; then
  session_secret="$(tr -d '[:space:]' <"$secret_file")"
  step "Reusing session secret from $secret_file"
else
  session_secret="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  (umask 077; printf '%s' "$session_secret" >"$secret_file")
  step "Generated a session secret and saved it to $secret_file"
fi

bind_host=127.0.0.1
if [[ $bind_all -eq 1 ]]; then
  bind_host=0.0.0.0
  warn "Binding 0.0.0.0 - only do this behind HTTPS (reverse proxy) or a VPN."
fi

export RUSTIC_AUTH_PASSWORD="$password"
export RUSTIC_SESSION_SECRET="$session_secret"
export RUSTIC_BIND_ADDR="$bind_host:$port"
export RUSTIC_DATA_DIR="$data_dir"
export RUSTIC_STATIC_DIR="$static_dir"
export RUST_LOG="${RUST_LOG:-info,reqwest=warn,hyper=warn,tower=warn,h2=warn,rustls=warn}"

url="http://127.0.0.1:$port"
echo
info "Starting rustic-server"
step "url      : $url"
step "bind     : $RUSTIC_BIND_ADDR"
step "password : $password"
step "data dir : $data_dir"
step "static   : $static_dir"
echo
ok "Open $url in your browser, then log in. Stop with Ctrl-C."
echo
exec "$binary"
