#requires -version 5.1
<#
.SYNOPSIS
  One-shot Rustic release: bump the version, wipe stale build output, build the
  production bundle, then commit + tag + push and publish a GitHub release.

.DESCRIPTION
  Codifies the release runbook so a release is a single command. Runs against the
  repo root regardless of where it's invoked from, and releases from the CURRENT
  git branch. Reads the current version from package.json, bumps it (patch by
  default), and keeps package.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json
  in sync.

  Stale-output guard: deletes dist/ and target/release/bundle/ before building, so
  the installer can't accumulate dead assets (dist had once grown to 585 MB,
  bloating the installer to 135 MB; a clean build is ~19 MB).

.PARAMETER Bump
  Which semver part to increment: patch (default), minor, or major.
  Ignored when -Version is supplied.

.PARAMETER Version
  Explicit version (x.y.z) to release, overriding -Bump.

.PARAMETER DryRun
  Print the plan (current -> next version, branch, tag) and exit. No changes.

.PARAMETER Force
  Skip the confirmation prompt shown before the irreversible push + publish step.

.EXAMPLE
  .\scripts\release.ps1                 # patch bump, asks before publishing
  .\scripts\release.ps1 -Bump minor
  .\scripts\release.ps1 -Version 0.4.0 -Force
  .\scripts\release.ps1 -DryRun
#>
[CmdletBinding()]
param(
  [ValidateSet('patch', 'minor', 'major')]
  [string]$Bump = 'patch',
  [string]$Version,
  [switch]$DryRun,
  [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Info($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Step($m) { Write-Host "  - $m" -ForegroundColor Gray }
function Ok($m)   { Write-Host "  OK $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  ! $m" -ForegroundColor Yellow }
function Die($m)  { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

# Note: native command stderr is intentionally NOT redirected anywhere below.
# Under Windows PowerShell 5.1, redirecting a native exe's stderr (2>&1 / 2>$null)
# wraps each line in an ErrorRecord which, with $ErrorActionPreference='Stop',
# can throw mid-build. We rely on $LASTEXITCODE instead.

# --- Resolve repo root (script lives in <root>/scripts) --------------------
$RepoRoot = Split-Path -Parent $PSScriptRoot
$pkgPath   = Join-Path $RepoRoot 'package.json'
$cargoToml = Join-Path $RepoRoot 'src-tauri\Cargo.toml'
$tauriConf = Join-Path $RepoRoot 'src-tauri\tauri.conf.json'
if (-not (Test-Path $pkgPath)) { Die "package.json not found at $pkgPath (is the script under <repo>/scripts?)" }
Set-Location $RepoRoot

# --- Tooling preconditions --------------------------------------------------
foreach ($t in 'git', 'bun', 'gh') {
  if (-not (Get-Command $t -ErrorAction SilentlyContinue)) { Die "'$t' is not on PATH" }
}
gh auth status
if ($LASTEXITCODE -ne 0) { Die "gh is not authenticated. Run: gh auth login" }

# --- Current branch (must not be detached) ----------------------------------
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
if ($LASTEXITCODE -ne 0) { Die "not a git repository" }
if ($branch -eq 'HEAD') { Die "detached HEAD - check out a branch before releasing" }

# --- Read + compute version -------------------------------------------------
$pkgRaw = [System.IO.File]::ReadAllText($pkgPath)
$m = [regex]::Match($pkgRaw, '"version"\s*:\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $m.Success) { Die "could not parse version from package.json" }
$cur = "$($m.Groups[1].Value).$($m.Groups[2].Value).$($m.Groups[3].Value)"
$maj = [int]$m.Groups[1].Value
$min = [int]$m.Groups[2].Value
$pat = [int]$m.Groups[3].Value

if ($Version) {
  if ($Version -notmatch '^\d+\.\d+\.\d+$') { Die "-Version must be x.y.z" }
  $next = $Version
}
else {
  switch ($Bump) {
    'major' { $maj++; $min = 0; $pat = 0 }
    'minor' { $min++; $pat = 0 }
    'patch' { $pat++ }
  }
  $next = "$maj.$min.$pat"
}
$tag = "v$next"

# Refuse to clobber an existing tag (local or remote).
if (git tag --list $tag) { Die "tag $tag already exists locally" }
if (git ls-remote --tags origin $tag) { Die "tag $tag already exists on origin" }

Info "Release plan"
Step "branch : $branch"
Step "version: $cur -> $next"
Step "tag    : $tag"
if ($DryRun) { Warn "DryRun - no changes made."; exit 0 }

# --- Bump version in the three sources (minimal, targeted text edits) -------
$curEsc = [regex]::Escape($cur)
function Update-VersionFile($path, $pattern, $replacement) {
  $raw = [System.IO.File]::ReadAllText($path)
  $rx = [regex]$pattern
  if (-not $rx.IsMatch($raw)) { Die "version pattern not found in $path" }
  $out = $rx.Replace($raw, $replacement, 1)  # first occurrence only
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($path, $out, $utf8NoBom)
}
Info "Bumping version to $next"
Update-VersionFile $pkgPath   "`"version`"\s*:\s*`"$curEsc`"" "`"version`": `"$next`""
Update-VersionFile $tauriConf "`"version`"\s*:\s*`"$curEsc`"" "`"version`": `"$next`""
Update-VersionFile $cargoToml "version\s*=\s*`"$curEsc`""       "version = `"$next`""
Ok "package.json, tauri.conf.json, Cargo.toml -> $next"

# --- Clean stale build output ----------------------------------------------
Info "Cleaning stale build output"
foreach ($d in @('dist', 'target\release\bundle')) {
  $p = Join-Path $RepoRoot $d
  if (Test-Path $p) { Remove-Item -Recurse -Force $p; Step "removed $d" }
}

# --- Updater signing key (required for in-app auto-updates) -----------------
$sigKeyPath = Join-Path $RepoRoot '.updater-signing.key'
if (-not (Test-Path $sigKeyPath)) { Die "updater signing key not found at $sigKeyPath - the build must be signed or installed apps cannot auto-update" }
$env:TAURI_SIGNING_PRIVATE_KEY = [System.IO.File]::ReadAllText($sigKeyPath)
# Only default the key password when the caller hasn't provided one — a key
# generated WITH a password would otherwise fail to sign because we clobbered
# the env var with an empty string.
if ($null -eq $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''
}

# --- Production build (devtools stripped via --no-default-features) ----------
# --locked (forwarded to cargo after the second `--`; bun eats the first one):
# refuse to build if Cargo.lock is out of date — a release must be built from
# the exact dependency set that was committed.
Info "Building production bundle (this takes a while)"
bun run tauri build -- --no-default-features -- --locked
if ($LASTEXITCODE -ne 0) { Die "tauri build failed" }

# --- Locate the produced installers -----------------------------------------
$nsis = Join-Path $RepoRoot "target\release\bundle\nsis\Rustic_${next}_x64-setup.exe"
$msi  = Join-Path $RepoRoot "target\release\bundle\msi\Rustic_${next}_x64_en-US.msi"
if (-not (Test-Path $nsis)) { Die "NSIS installer not found: $nsis" }
$assets = @($nsis)
if (Test-Path $msi) { $assets += $msi } else { Warn "MSI not found - releasing NSIS only" }
$nsisMb = [math]::Round((Get-Item $nsis).Length / 1MB, 1)
Ok "NSIS installer: $nsisMb MB"

# --- Updater manifest (latest.json) ------------------------------------------
# The installed app polls https://github.com/<owner>/<repo>/releases/latest/
# download/latest.json (see tauri.conf.json plugins.updater.endpoints), so every
# release MUST ship this asset or auto-update silently stops working.
$nsisSig = "$nsis.sig"
if (-not (Test-Path $nsisSig)) { Die "updater signature not found: $nsisSig (is bundle.createUpdaterArtifacts enabled in tauri.conf.json?)" }
Info "Generating updater manifest (latest.json)"
$manifest = [ordered]@{
  version   = $next
  notes     = "Rustic v$next - see the GitHub release notes for details."
  pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'")
  platforms = [ordered]@{
    'windows-x86_64' = [ordered]@{
      signature = ([System.IO.File]::ReadAllText($nsisSig)).Trim()
      url       = "https://github.com/avijitbhuin21/Rustic/releases/download/$tag/Rustic_${next}_x64-setup.exe"
    }
  }
}
$latestJson = Join-Path $RepoRoot 'target\release\bundle\latest.json'
$enc = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($latestJson, ($manifest | ConvertTo-Json -Depth 5), $enc)
$assets += $latestJson
Ok "latest.json written"

# --- Confirmation gate before anything irreversible -------------------------
if (-not $Force) {
  Write-Host ""
  Info "About to commit, push to '$branch', and publish GitHub release $tag with:"
  foreach ($a in $assets) { Step (Split-Path -Leaf $a) }
  $ans = Read-Host "Proceed? (y/N)"
  if ($ans -ne 'y' -and $ans -ne 'Y') {
    Warn "Aborted. Version files are bumped and a fresh build exists, but nothing was committed or pushed."
    exit 0
  }
}

# --- Commit, tag, push ------------------------------------------------------
Info "Committing"
git add -A
if ($LASTEXITCODE -ne 0) { Die "git add failed" }
# Keep machine-local .claude state (settings.local.json, worktrees) out of the release commit.
git reset -q -- .claude/
git commit -m "v$next" -m "Automated release v$next"
if ($LASTEXITCODE -ne 0) { Die "git commit failed (nothing to commit?)" }
git tag -a $tag -m "Rustic $tag"
if ($LASTEXITCODE -ne 0) { Die "git tag failed" }

Info "Pushing $branch + $tag"
git push origin $branch
if ($LASTEXITCODE -ne 0) { Die "git push failed (is the branch behind origin? pull/rebase, then re-run from the publish step)" }
git push origin $tag
if ($LASTEXITCODE -ne 0) { Die "git push tag failed" }

# --- Publish GitHub release -------------------------------------------------
Info "Creating GitHub release $tag"
gh release create $tag --title "Rustic $tag" --generate-notes @assets
if ($LASTEXITCODE -ne 0) { Die "gh release create failed" }

$url = (gh release view $tag --json url --jq .url)
Write-Host ""
Ok "Released $tag"
Write-Host "  $url" -ForegroundColor Green
