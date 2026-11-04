// Bumps the Rustic app version across every source of truth in one command
// (Windows / macOS / Linux). Replaces bump-version.ps1 + bump-version.sh.
//
// Updates package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml to
// the given semver, then refreshes Cargo.lock. The status bar reads the version
// at runtime via Tauri getVersion(), so no UI edit is ever needed.
//
// Usage: node scripts/bump-version.mjs <x.y.z[-pre]> [--dry-run]
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { ROOT, die, isMain, ok, parseArgs, run, step, warn } from './_common.mjs';

const USAGE = `Usage: node scripts/bump-version.mjs <version> [--dry-run]

  <version>   semver like 0.3.8 or 1.0.0-beta.1
  --dry-run   print the files and lines that would change; write nothing`;

const SEMVER = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;

/** The files bumped, and the FIRST-match pattern + replacement for each. */
export const VERSION_FILES = [
  { file: 'package.json', re: /"version":\s*"[^"]*"/, to: (v) => `"version": "${v}"` },
  { file: 'src-tauri/tauri.conf.json', re: /"version":\s*"[^"]*"/, to: (v) => `"version": "${v}"` },
  { file: 'src-tauri/Cargo.toml', re: /^version\s*=\s*"[^"]*"/m, to: (v) => `version = "${v}"` },
];

/**
 * Rewrite the version in every VERSION_FILES entry. Fails (before writing
 * anything) if a file has no version field, or — when `expectCurrent` is set —
 * if the field found isn't that version. `syncLock` runs `cargo update -p rustic`.
 */
export function bumpVersion(version, { dryRun = false, expectCurrent, syncLock = true } = {}) {
  if (!SEMVER.test(version || '')) {
    die(`Invalid version '${version ?? ''}'. Expected semver like 0.3.8 or 1.0.0-beta.1.`);
  }
  const edits = VERSION_FILES.map(({ file, re, to }) => {
    const full = path.join(ROOT, file);
    const text = readFileSync(full, 'utf8');
    const m = text.match(re);
    if (!m) die(`No version field replaced in ${file}.`);
    if (expectCurrent && !m[0].includes(`"${expectCurrent}"`)) {
      die(`${file} has ${m[0]}, expected version ${expectCurrent}`);
    }
    return { file, full, before: m[0], text: text.replace(re, to(version)) };
  });
  for (const e of edits) {
    if (dryRun) { step(`[dry-run] ${e.file}: ${e.before} -> ${VERSION_FILES.find((f) => f.file === e.file).to(version)}`); continue; }
    writeFileSync(e.full, e.text);
    console.log(`Updated ${e.file} -> ${version}`);
  }
  if (!syncLock) return;
  if (dryRun) { step(`[dry-run] cargo update -p rustic --precise ${version}  (refresh Cargo.lock)`); return; }
  if (run('cargo', ['update', '-p', 'rustic', '--precise', version], { quiet: true }).code === 0) {
    console.log('Refreshed Cargo.lock');
  } else {
    warn("Run 'cargo build' to refresh Cargo.lock.");
  }
}

if (isMain(import.meta.url)) {
  const args = parseArgs(process.argv.slice(2), { dryrun: { key: 'dryRun' } });
  if (args.help) { console.log(USAGE); process.exit(0); }
  if (args._.length !== 1) { console.error(USAGE); process.exit(1); }
  const version = args._[0];
  bumpVersion(version, { dryRun: !!args.dryRun });
  if (args.dryRun) warn('Dry run - no changes made.');
  else ok(`Done. Rustic is now at v${version}.`);
}
