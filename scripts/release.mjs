// One-shot Rustic release (Windows / macOS / Linux). Replaces release.ps1 + release.sh.
//
// Default (every OS): bump the version, sync + verify Cargo.lock, commit, tag
// and push. The tag push triggers .github/workflows/release.yml, which builds
// Windows, macOS (arm + intel) and Linux into ONE draft GitHub release with a
// combined latest.json. Publish that draft once every job is green.
//
// --local-build (Windows only, the old release.ps1 flow): additionally wipe
// stale build output, build + sign the NSIS/MSI installers locally, write a
// Windows-only latest.json and publish with `gh release create`. Note the tag
// push still triggers CI, so only use this when CI is unavailable.
import { readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { bumpVersion } from './bump-version.mjs';
import { IS_WIN, ROOT, confirm, die, exists, info, ok, parseArgs, run, step, warn, which } from './_common.mjs';

const USAGE = `Usage: node scripts/release.mjs [options]

  --bump patch|minor|major   semver part to increment (default patch)
  --version x.y.z            explicit version, overrides --bump
  --dry-run                  print the plan (branch, version, tag) and exit
  --force                    skip the confirmation prompt before push/publish
  --local-build              Windows only: build, sign and publish locally (old release.ps1)

Examples:
  node scripts/release.mjs                    # patch bump, asks before pushing
  node scripts/release.mjs --bump minor
  node scripts/release.mjs --version 0.8.0 --force
  node scripts/release.mjs --dry-run
(PowerShell-style flags such as -Bump minor / -DryRun are also accepted.)`;

const args = parseArgs(process.argv.slice(2), {
  bump: { key: 'bump', value: true },
  version: { key: 'version', value: true },
  dryrun: { key: 'dryRun' },
  force: { key: 'force' },
  localbuild: { key: 'localBuild' },
});
if (args.help) { console.log(USAGE); process.exit(0); }
if (args._.length) die(`unknown argument: ${args._[0]}`);
const bump = (args.bump || 'patch').toLowerCase();
if (!['patch', 'minor', 'major'].includes(bump)) die('--bump must be patch, minor or major');
if (args.localBuild && !IS_WIN) {
  die('--local-build is Windows only (a local Mac/Linux build cannot produce the Windows installer, and a single-platform latest.json breaks auto-update). Push the tag and let CI build.');
}

const pkgPath = path.join(ROOT, 'package.json');
if (!exists(pkgPath)) die(`package.json not found at ${pkgPath}`);

// --- Tooling preconditions ---------------------------------------------------
const tools = args.localBuild ? ['git', 'cargo', 'bun', 'gh'] : ['git', 'cargo'];
for (const t of tools) if (!which(t)) die(`'${t}' is not on PATH`);
if (args.localBuild && run('gh', ['auth', 'status']).code !== 0) die('gh is not authenticated. Run: gh auth login');

// --- Current branch (must not be detached) -----------------------------------
const br = run('git', ['rev-parse', '--abbrev-ref', 'HEAD'], { capture: true });
if (br.code !== 0) die('not a git repository');
const branch = br.stdout;
if (branch === 'HEAD') die('detached HEAD - check out a branch before releasing');

// --- Read + compute version --------------------------------------------------
const m = readFileSync(pkgPath, 'utf8').match(/"version"\s*:\s*"(\d+)\.(\d+)\.(\d+)"/);
if (!m) die('could not parse version from package.json');
const cur = `${m[1]}.${m[2]}.${m[3]}`;
let [maj, min, pat] = [Number(m[1]), Number(m[2]), Number(m[3])];
let next;
if (args.version) {
  if (!/^\d+\.\d+\.\d+$/.test(args.version)) die('--version must be x.y.z');
  next = args.version;
} else {
  if (bump === 'major') { maj++; min = 0; pat = 0; } else if (bump === 'minor') { min++; pat = 0; } else pat++;
  next = `${maj}.${min}.${pat}`;
}
const tag = `v${next}`;

// Refuse to clobber an existing tag (local or remote).
if (run('git', ['tag', '--list', tag], { capture: true }).stdout) die(`tag ${tag} already exists locally`);
const remote = run('git', ['ls-remote', '--tags', 'origin', tag], { capture: true });
if (remote.code !== 0) die('git ls-remote origin failed - check network / remote');
if (remote.stdout) die(`tag ${tag} already exists on origin`);

info('Release plan');
step(`branch : ${branch}`);
step(`version: ${cur} -> ${next}`);
step(`tag    : ${tag}`);
step(`mode   : ${args.localBuild ? 'local build + gh release (Windows)' : 'push tag; CI builds the draft release'}`);
if (args.dryRun) {
  bumpVersion(next, { dryRun: true, expectCurrent: cur, syncLock: false });
  warn('Dry run - no changes made.');
  process.exit(0);
}

// --- Bump + sync Cargo.lock --------------------------------------------------
info(`Bumping version to ${next}`);
bumpVersion(next, { expectCurrent: cur, syncLock: false });
info('Syncing Cargo.lock');
if (run('cargo', ['update', '-p', 'rustic', '--precise', next]).code !== 0) {
  die(`cargo update -p rustic failed - could not sync Cargo.lock to ${next}`);
}
// Only the exit code matters; `cargo metadata` prints several MB of JSON, which
// overflowed spawnSync's 1 MB capture buffer and read as a false "out of date".
if (run('cargo', ['metadata', '--locked', '--format-version', '1'], { quiet: true }).code !== 0) {
  die("Cargo.lock is out of date - run 'cargo update' and commit the lockfile before releasing");
}
ok('Cargo.lock in sync');

const assets = args.localBuild ? localBuild(next, tag) : [];

// --- Confirmation gate before anything irreversible --------------------------
if (!args.force) {
  console.log('');
  if (args.localBuild) {
    info(`About to commit, push to '${branch}', and publish GitHub release ${tag} with:`);
    for (const a of assets) step(path.basename(a));
  } else {
    info(`About to commit, tag ${tag} and push to '${branch}' (CI then builds the draft release).`);
  }
  if (!(await confirm('Proceed?'))) {
    warn('Aborted. Version files are bumped, but nothing was committed or pushed.');
    process.exit(0);
  }
}

// --- Commit, tag, push -------------------------------------------------------
info('Committing');
if (run('git', ['add', '-A']).code !== 0) die('git add failed');
// Keep machine-local agent state out of the release commit.
run('git', ['reset', '-q', '--', '.claude/'], { quiet: true });
if (run('git', ['commit', '-m', `v${next}`, '-m', `Automated release v${next}`]).code !== 0) {
  die('git commit failed (nothing to commit?)');
}
if (run('git', ['tag', '-a', tag, '-m', `Rustic ${tag}`]).code !== 0) die('git tag failed');

info(`Pushing ${branch} + ${tag}`);
if (run('git', ['push', 'origin', branch]).code !== 0) {
  die('git push failed (branch behind origin? pull/rebase, then push the tag manually)');
}
if (run('git', ['push', 'origin', tag]).code !== 0) die('git push tag failed');

if (!args.localBuild) {
  ok(`Pushed ${tag}`);
  console.log('  CI is building the draft release. Publish it once every job is green:');
  if (which('gh')) console.log(`  gh release view ${tag} --web`);
  process.exit(0);
}

// --- Publish GitHub release (local-build mode) -------------------------------
info(`Creating GitHub release ${tag}`);
if (run('gh', ['release', 'create', tag, '--title', `Rustic ${tag}`, '--generate-notes', ...assets]).code !== 0) {
  die('gh release create failed');
}
const url = run('gh', ['release', 'view', tag, '--json', 'url', '--jq', '.url'], { capture: true }).stdout;
console.log('');
ok(`Released ${tag}`);
console.log(`  ${url}`);

/** Old release.ps1 build path: clean, sign, build, locate installers, write latest.json. */
function localBuild(next, tag) {
  // Stale-output guard: dist/ once grew to 585 MB and bloated the installer.
  info('Cleaning stale build output');
  for (const d of ['dist', 'target/release/bundle']) {
    const p = path.join(ROOT, d);
    if (exists(p)) { rmSync(p, { recursive: true, force: true }); step(`removed ${d}`); }
  }
  const keyPath = path.join(ROOT, '.updater-signing.key');
  if (!exists(keyPath)) die(`updater signing key not found at ${keyPath} - the build must be signed or installed apps cannot auto-update`);
  const env = { ...process.env, TAURI_SIGNING_PRIVATE_KEY: readFileSync(keyPath, 'utf8') };

  // Blank stdin lines answer an empty-password `Password:` prompt (a non-empty
  // TAURI_SIGNING_PRIVATE_KEY_PASSWORD still wins). Inside an agent pty the
  // prompt reads the tty instead and can hang - see memory release-pipeline.md.
  info('Building production bundle (this takes a while)');
  if (run('bun', ['run', 'tauri', 'build', '--', '--no-default-features'], { input: '\n\n', env }).code !== 0) {
    die('tauri build failed');
  }

  const bundle = path.join(ROOT, 'target', 'release', 'bundle');
  const nsis = path.join(bundle, 'nsis', `Rustic_${next}_x64-setup.exe`);
  const msi = path.join(bundle, 'msi', `Rustic_${next}_x64_en-US.msi`);
  if (!exists(nsis)) die(`NSIS installer not found: ${nsis}`);
  const out = [nsis];
  if (exists(msi)) out.push(msi); else warn('MSI not found - releasing NSIS only');
  ok(`NSIS installer: ${(statSync(nsis).size / 1048576).toFixed(1)} MB`);

  // The installed app polls releases/latest/download/latest.json, so every
  // release MUST ship it or auto-update silently stops working.
  const sig = `${nsis}.sig`;
  if (!exists(sig)) die(`updater signature not found: ${sig} (is bundle.createUpdaterArtifacts enabled in tauri.conf.json?)`);
  info('Generating updater manifest (latest.json)');
  const manifest = {
    version: next,
    notes: `Rustic v${next} - see the GitHub release notes for details.`,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    platforms: {
      'windows-x86_64': {
        signature: readFileSync(sig, 'utf8').trim(),
        url: `https://github.com/avijitbhuin21/Rustic/releases/download/${tag}/Rustic_${next}_x64-setup.exe`,
      },
    },
  };
  const latest = path.join(bundle, 'latest.json');
  writeFileSync(latest, JSON.stringify(manifest, null, 2));
  out.push(latest);
  ok('latest.json written');
  return out;
}
