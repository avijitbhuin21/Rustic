#!/usr/bin/env bash
# One-shot Rustic release from macOS / Linux: bump the version, sync
# Cargo.lock, commit + tag + push. The tag push triggers
# .github/workflows/release.yml, which builds Windows, macOS (arm + intel) and
# Linux and uploads them to ONE draft GitHub release with a combined
# latest.json. Publish that draft once every job is green.
#
# Unlike release.ps1 this does NOT build or publish locally: a local Mac build
# can't produce the Windows installer, and a Mac-only latest.json would break
# auto-update for every Windows user.
#
# Usage:
#   ./scripts/release.sh                  # patch bump, asks before pushing
#   ./scripts/release.sh --bump minor
#   ./scripts/release.sh --version 0.8.0 --force
#   ./scripts/release.sh --dry-run
set -euo pipefail

info() { printf '\033[36m==> %s\033[0m\n' "$*"; }
step() { printf '  - %s\n' "$*"; }
ok()   { printf '\033[32m  OK %s\033[0m\n' "$*"; }
warn() { printf '\033[33m  ! %s\033[0m\n' "$*"; }
die()  { printf '\033[31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

bump=patch
version=""
dry_run=0
force=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bump) bump="${2:-}"; shift 2 ;;
    --version) version="${2:-}"; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    --force) force=1; shift ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done
[[ "$bump" =~ ^(patch|minor|major)$ ]] || die "--bump must be patch, minor or major"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
[[ -f package.json ]] || die "package.json not found at $root"

for t in git cargo perl; do
  command -v "$t" >/dev/null || die "'$t' is not on PATH"
done

branch="$(git rev-parse --abbrev-ref HEAD)" || die "not a git repository"
[[ "$branch" != "HEAD" ]] || die "detached HEAD - check out a branch before releasing"

cur="$(perl -ne 'if (/"version"\s*:\s*"(\d+\.\d+\.\d+)"/) { print $1; exit }' package.json)"
[[ -n "$cur" ]] || die "could not parse version from package.json"
IFS=. read -r maj min pat <<<"$cur"

if [[ -n "$version" ]]; then
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be x.y.z"
  next="$version"
else
  case "$bump" in
    major) maj=$((maj + 1)); min=0; pat=0 ;;
    minor) min=$((min + 1)); pat=0 ;;
    patch) pat=$((pat + 1)) ;;
  esac
  next="$maj.$min.$pat"
fi
tag="v$next"

[[ -z "$(git tag --list "$tag")" ]] || die "tag $tag already exists locally"
[[ -z "$(git ls-remote --tags origin "$tag")" ]] || die "tag $tag already exists on origin"

info "Release plan"
step "branch : $branch"
step "version: $cur -> $next"
step "tag    : $tag"
if [[ $dry_run -eq 1 ]]; then warn "Dry run - no changes made."; exit 0; fi

info "Bumping version to $next"
"$root/scripts/bump-version.sh" "$next"

info "Verifying Cargo.lock"
cargo update -p rustic --precise "$next" || die "cargo update -p rustic failed"
cargo metadata --locked --format-version 1 >/dev/null \
  || die "Cargo.lock is out of date - run 'cargo update' and commit the lockfile first"
ok "Cargo.lock in sync"

if [[ $force -ne 1 ]]; then
  echo
  info "About to commit, tag $tag and push to '$branch' (CI then builds the draft release)."
  read -r -p "Proceed? (y/N) " ans
  if [[ "$ans" != "y" && "$ans" != "Y" ]]; then
    warn "Aborted. Version files are bumped, but nothing was committed or pushed."
    exit 0
  fi
fi

info "Committing"
git add -A
# Keep machine-local agent state out of the release commit.
git reset -q -- .claude/ 2>/dev/null || true
git commit -m "v$next" -m "Automated release v$next" || die "git commit failed (nothing to commit?)"
git tag -a "$tag" -m "Rustic $tag" || die "git tag failed"

info "Pushing $branch + $tag"
git push origin "$branch" || die "git push failed (branch behind origin? pull/rebase, then push the tag manually)"
git push origin "$tag" || die "git push tag failed"

ok "Pushed $tag"
echo "  CI is building the draft release. Publish it once every job is green:"
if command -v gh >/dev/null; then
  echo "  gh release view $tag --web"
fi
