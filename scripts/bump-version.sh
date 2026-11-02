#!/usr/bin/env bash
# Bumps the Rustic app version across every source of truth in one command.
# bash port of bump-version.ps1 (macOS / Linux).
#
# Updates package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml to
# the given semver, then refreshes Cargo.lock. The status bar reads the version
# at runtime via Tauri getVersion(), so no UI edit is ever needed.
#
# Usage: ./scripts/bump-version.sh 0.3.8
set -euo pipefail

version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "Invalid version '$version'. Expected semver like 0.3.8 or 1.0.0-beta.1." >&2
  exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Replace the FIRST match of a perl regex in a file; fail if nothing changed.
replace_first() {
  local file="$1" pattern="$2" replacement="$3"
  local full="$root/$file"
  local before after
  before="$(cat "$full")"
  V="$replacement" perl -0pi -e "s/$pattern/\$ENV{V}/" "$full"
  after="$(cat "$full")"
  if [[ "$before" == "$after" ]]; then
    echo "No version field replaced in $file." >&2
    exit 1
  fi
  echo "Updated $file -> $version"
}

replace_first package.json '"version":\s*"[^"]*"' "\"version\": \"$version\""
replace_first src-tauri/tauri.conf.json '"version":\s*"[^"]*"' "\"version\": \"$version\""
replace_first src-tauri/Cargo.toml '(?m)^version\s*=\s*"[^"]*"' "version = \"$version\""

cd "$root"
if cargo update -p rustic --precise "$version" 2>/dev/null; then
  echo "Refreshed Cargo.lock"
else
  echo "Run 'cargo build' to refresh Cargo.lock."
fi

echo "Done. Rustic is now at v$version."
