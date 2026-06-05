#requires -version 5.1
<#
.SYNOPSIS
  Build (if needed) and run rustic-server - the headless web build of Rustic -
  on your local machine, then open it in a browser.

.DESCRIPTION
  One command to go from a checkout to a running web server. It:
    1. builds the web frontend bundle (Vite `dist/`) unless -SkipWebBuild,
    2. builds the rustic-server binary (debug by default; -Release for optimized),
    3. resolves/persists a stable session secret so logins survive restarts,
    4. sets the required env vars and runs the server in the foreground.

  Runs against the repo root regardless of where it's invoked from. Binds to
  127.0.0.1 by default: localhost is a browser "secure context", so clipboard
  copy/paste and the microphone (audio transcription) work without HTTPS. Use
  -BindAll to listen on 0.0.0.0 (only behind a reverse proxy / VPN - see
  rustic-server/README.md).

  Stop the server with Ctrl-C.

.PARAMETER Password
  The login password gating every route. Defaults to $env:RUSTIC_AUTH_PASSWORD,
  or 'rustic' for local dev (a warning is printed - never expose that publicly).

.PARAMETER Port
  Port to listen on (default 8787).

.PARAMETER DataDir
  Application data dir (rustic.db, logs/, file-history, secrets.json,
  .session-secret). Default: <repo>/rustic-data. Provider API keys you set in
  the Settings UI are persisted here.

.PARAMETER StaticDir
  Directory containing the built web frontend. Default: <repo>/dist.

.PARAMETER Release
  Build the server in release (optimized) mode. Slower to build, faster to run.
  Default is a debug build (fast incremental rebuilds for iteration).

.PARAMETER SkipWebBuild
  Skip `bun run build:web`. Use when dist/ is already current (the web build
  takes a couple of minutes). The script also auto-skips with a warning if it
  can't find bun, as long as a dist/ already exists.

.PARAMETER BindAll
  Bind 0.0.0.0 instead of 127.0.0.1. Only do this behind HTTPS (reverse proxy)
  or a VPN - a password on a plain public port is not enough.

.EXAMPLE
  .\scripts\run-server.ps1
  .\scripts\run-server.ps1 -Password hunter2 -Port 9000
  .\scripts\run-server.ps1 -SkipWebBuild        # reuse existing dist/
  .\scripts\run-server.ps1 -Release             # optimized build
#>
[CmdletBinding()]
param(
  [string]$Password,
  [int]$Port = 8787,
  [string]$DataDir,
  [string]$StaticDir,
  [switch]$Release,
  [switch]$SkipWebBuild,
  [switch]$BindAll
)

$ErrorActionPreference = 'Stop'

function Info($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Step($m) { Write-Host "  - $m" -ForegroundColor Gray }
function Ok($m)   { Write-Host "  OK $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  ! $m" -ForegroundColor Yellow }
function Die($m)  { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

# Note: native command stderr is intentionally NOT redirected. Under Windows
# PowerShell 5.1, redirecting a native exe's stderr wraps each line in an
# ErrorRecord which, with $ErrorActionPreference='Stop', can throw mid-build.
# We rely on $LASTEXITCODE instead.

# --- Resolve repo root (script lives in <root>/scripts) ---------------------
$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path (Join-Path $RepoRoot 'package.json'))) {
  Die "package.json not found at repo root (is the script under <repo>/scripts?)"
}
Set-Location $RepoRoot

# --- Defaults relative to the repo root -------------------------------------
if (-not $DataDir)   { $DataDir   = Join-Path $RepoRoot 'rustic-data' }
if (-not $StaticDir) { $StaticDir = Join-Path $RepoRoot 'dist' }

# --- Tooling preconditions --------------------------------------------------
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Die "'cargo' is not on PATH" }
$haveBun = [bool](Get-Command bun -ErrorAction SilentlyContinue)

# --- Password ---------------------------------------------------------------
if (-not $Password) {
  if ($env:RUSTIC_AUTH_PASSWORD) {
    $Password = $env:RUSTIC_AUTH_PASSWORD
  }
  else {
    $Password = 'rustic'
    Warn "No -Password / RUSTIC_AUTH_PASSWORD set; using the dev default 'rustic'."
    Warn "NEVER expose this server publicly with that password."
  }
}

# --- Web build --------------------------------------------------------------
if ($SkipWebBuild) {
  Info "Skipping web build (-SkipWebBuild)"
  if (-not (Test-Path (Join-Path $StaticDir 'index.html'))) {
    Die "No built frontend at $StaticDir (index.html missing). Run without -SkipWebBuild first."
  }
}
elseif (-not $haveBun) {
  Warn "'bun' is not on PATH - skipping web build."
  if (-not (Test-Path (Join-Path $StaticDir 'index.html'))) {
    Die "No built frontend at $StaticDir and bun is unavailable. Install bun, or build dist/ another way."
  }
  Step "Reusing existing $StaticDir"
}
else {
  Info "Building web frontend (bun run build:web) - this takes a couple of minutes"
  bun run build:web
  if ($LASTEXITCODE -ne 0) { Die "web build failed" }
  Ok "dist/ built"
}

# --- Server build -----------------------------------------------------------
if ($Release) {
  Info "Building rustic-server (release)"
  cargo build --release -p rustic-server
  if ($LASTEXITCODE -ne 0) { Die "cargo build (release) failed" }
  $binary = Join-Path $RepoRoot 'target\release\rustic-server.exe'
}
else {
  Info "Building rustic-server (debug)"
  cargo build -p rustic-server
  if ($LASTEXITCODE -ne 0) { Die "cargo build failed" }
  $binary = Join-Path $RepoRoot 'target\debug\rustic-server.exe'
}
if (-not (Test-Path $binary)) { Die "server binary not found at $binary" }
Ok "server binary ready"

# --- Stable session secret (so logins survive restarts) --------------------
# Precedence: explicit env var > persisted file > freshly generated + saved.
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$secretFile = Join-Path $DataDir '.session-secret'
if ($env:RUSTIC_SESSION_SECRET) {
  $sessionSecret = $env:RUSTIC_SESSION_SECRET
  Step "Using RUSTIC_SESSION_SECRET from the environment"
}
elseif (Test-Path $secretFile) {
  $sessionSecret = (Get-Content $secretFile -Raw).Trim()
  Step "Reusing session secret from $secretFile"
}
else {
  $bytes = New-Object 'System.Byte[]' 32
  [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
  $sessionSecret = -join ($bytes | ForEach-Object { '{0:x2}' -f $_ })
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($secretFile, $sessionSecret, $utf8NoBom)
  Step "Generated a session secret and saved it to $secretFile"
}

# --- Bind address -----------------------------------------------------------
$bindHost = if ($BindAll) { '0.0.0.0' } else { '127.0.0.1' }
$bindAddr = "${bindHost}:${Port}"
if ($BindAll) {
  Warn "Binding 0.0.0.0 - only do this behind HTTPS (reverse proxy) or a VPN."
}

# --- Env + run --------------------------------------------------------------
$env:RUSTIC_AUTH_PASSWORD  = $Password
$env:RUSTIC_SESSION_SECRET = $sessionSecret
$env:RUSTIC_BIND_ADDR      = $bindAddr
$env:RUSTIC_DATA_DIR       = $DataDir
$env:RUSTIC_STATIC_DIR     = $StaticDir
if (-not $env:RUST_LOG) { $env:RUST_LOG = 'info,reqwest=warn,hyper=warn,tower=warn,h2=warn,rustls=warn' }

$openUrl = "http://127.0.0.1:$Port"
Write-Host ""
Info "Starting rustic-server"
Step "url      : $openUrl"
Step "bind     : $bindAddr"
Step "password : $Password"
Step "data dir : $DataDir"
Step "static   : $StaticDir"
Write-Host ""
Ok "Open $openUrl in your browser, then log in. Stop with Ctrl-C."
Write-Host ""

& $binary
exit $LASTEXITCODE
