#Requires -Version 5.1
<#
.SYNOPSIS
    Bumps the Rustic app version across every source of truth in one command.
.DESCRIPTION
    Updates package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml to the
    given semver, then refreshes Cargo.lock. The status bar reads the version at
    runtime via Tauri getVersion() (sourced from tauri.conf.json), so no UI edit
    is ever needed.
.EXAMPLE
    ./scripts/bump-version.ps1 0.3.8
#>
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
    throw "Invalid version '$Version'. Expected semver like 0.3.8 or 1.0.0-beta.1."
}

$root = Split-Path -Parent $PSScriptRoot

function Set-JsonVersion([string]$path, [string]$version) {
    $full = Join-Path $root $path
    $text = Get-Content $full -Raw
    $updated = [regex]::Replace($text, '"version":\s*"[^"]*"', "`"version`": `"$version`"", 1)
    if ($updated -eq $text) { throw "No version field replaced in $path." }
    Set-Content $full $updated -NoNewline
    Write-Host "Updated $path -> $version"
}

function Set-CargoVersion([string]$path, [string]$version) {
    $full = Join-Path $root $path
    $text = Get-Content $full -Raw
    $updated = [regex]::Replace($text, '(?m)^version\s*=\s*"[^"]*"', "version = `"$version`"", 1)
    if ($updated -eq $text) { throw "No version field replaced in $path." }
    Set-Content $full $updated -NoNewline
    Write-Host "Updated $path -> $version"
}

Set-JsonVersion 'package.json' $Version
Set-JsonVersion 'src-tauri/tauri.conf.json' $Version
Set-CargoVersion 'src-tauri/Cargo.toml' $Version

Push-Location $root
try {
    cargo update -p rustic --precise $Version 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Run 'cargo build' to refresh Cargo.lock." -ForegroundColor Yellow
    } else {
        Write-Host "Refreshed Cargo.lock"
    }
} finally {
    Pop-Location
}

Write-Host "Done. Rustic is now at v$Version." -ForegroundColor Green
