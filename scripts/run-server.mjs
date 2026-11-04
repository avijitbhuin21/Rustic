// Build (if needed) and run rustic-server - the headless web build of Rustic -
// locally (Windows / macOS / Linux). Replaces run-server.ps1 + run-server.sh.
//
// 1. builds the web frontend (`bun run build:web` -> dist/) unless --skip-web-build,
// 2. builds the rustic-server binary (debug by default; --release for optimized),
// 3. resolves/persists a stable session secret so logins survive restarts,
// 4. sets the required env vars and runs the server in the foreground.
//
// Binds 127.0.0.1 by default: localhost is a browser "secure context", so
// clipboard and microphone work without HTTPS. --bind-all listens on 0.0.0.0
// (only behind a reverse proxy / VPN - see rustic-server/README.md).
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { IS_WIN, ROOT, die, exists, info, ok, parseArgs, run, step, warn, which } from './_common.mjs';

const USAGE = `Usage: node scripts/run-server.mjs [options]

  --password P               login password (default: $RUSTIC_AUTH_PASSWORD, else dev default 'rustic')
  --port N                   port to listen on (default 8787)
  --data-dir D               app data dir (default <repo>/rustic-data)
  --static-dir D             built web frontend (default <repo>/dist)
  --release                  optimized server build (default: debug)
  --skip-web-build           reuse the existing dist/ (skips the ~2 min web build)
  --bind-all                 bind 0.0.0.0 - only behind HTTPS (reverse proxy) or a VPN
  --allow-insecure-password  allow --bind-all with the default password 'rustic'

Examples:
  node scripts/run-server.mjs
  node scripts/run-server.mjs --password hunter2 --port 9000
  node scripts/run-server.mjs --skip-web-build --release
(PowerShell-style flags such as -SkipWebBuild / -Port 9000 are also accepted.)`;

const args = parseArgs(process.argv.slice(2), {
  password: { key: 'password', value: true },
  port: { key: 'port', value: true },
  datadir: { key: 'dataDir', value: true },
  staticdir: { key: 'staticDir', value: true },
  release: { key: 'release' },
  skipwebbuild: { key: 'skipWebBuild' },
  bindall: { key: 'bindAll' },
  allowinsecurepassword: { key: 'allowInsecure' },
});
if (args.help) { console.log(USAGE); process.exit(0); }
if (args._.length) die(`unknown argument: ${args._[0]}`);

const port = Number(args.port ?? 8787);
if (!Number.isInteger(port) || port < 1 || port > 65535) die(`--port must be 1-65535, got '${args.port}'`);
if (!exists(path.join(ROOT, 'package.json'))) die('package.json not found at repo root');
const dataDir = path.resolve(args.dataDir || path.join(ROOT, 'rustic-data'));
const staticDir = path.resolve(args.staticDir || path.join(ROOT, 'dist'));

if (!which('cargo')) die("'cargo' is not on PATH");

// --- Password ----------------------------------------------------------------
let password = args.password;
if (!password) {
  if (process.env.RUSTIC_AUTH_PASSWORD) password = process.env.RUSTIC_AUTH_PASSWORD;
  else {
    password = 'rustic';
    warn("No --password / RUSTIC_AUTH_PASSWORD set; using the dev default 'rustic'.");
    warn('NEVER expose this server publicly with that password.');
  }
}
// --bind-all with the well-known default password is an open door, not a warning.
if (args.bindAll && password === 'rustic' && !args.allowInsecure) {
  die("--bind-all with the default password 'rustic' is refused: anyone who can reach the port can log in. Pass --password <something-strong>, or --allow-insecure-password on a trusted network.");
}

// --- Web build ---------------------------------------------------------------
const haveIndex = () => exists(path.join(staticDir, 'index.html'));
if (args.skipWebBuild) {
  info('Skipping web build (--skip-web-build)');
  if (!haveIndex()) die(`No built frontend at ${staticDir} (index.html missing). Run without --skip-web-build first.`);
} else if (!which('bun')) {
  warn("'bun' is not on PATH - skipping web build.");
  if (!haveIndex()) die(`No built frontend at ${staticDir} and bun is unavailable. Install bun, or build dist/ another way.`);
  step(`Reusing existing ${staticDir}`);
} else {
  info('Building web frontend (bun run build:web) - this takes a couple of minutes');
  if (run('bun', ['run', 'build:web']).code !== 0) die('web build failed');
  ok('dist/ built');
}

// --- Server build ------------------------------------------------------------
const profile = args.release ? 'release' : 'debug';
info(`Building rustic-server (${profile})`);
if (run('cargo', ['build', ...(args.release ? ['--release'] : []), '-p', 'rustic-server']).code !== 0) {
  die(`cargo build${args.release ? ' (release)' : ''} failed`);
}
const binary = path.join(ROOT, 'target', profile, IS_WIN ? 'rustic-server.exe' : 'rustic-server');
if (!exists(binary)) die(`server binary not found at ${binary}`);
ok('server binary ready');

// --- Stable session secret: env > persisted file > freshly generated + saved --
mkdirSync(dataDir, { recursive: true });
const secretFile = path.join(dataDir, '.session-secret');
let sessionSecret;
if (process.env.RUSTIC_SESSION_SECRET) {
  sessionSecret = process.env.RUSTIC_SESSION_SECRET;
  step('Using RUSTIC_SESSION_SECRET from the environment');
} else if (exists(secretFile)) {
  sessionSecret = readFileSync(secretFile, 'utf8').replace(/\s+/g, '');
  step(`Reusing session secret from ${secretFile}`);
} else {
  sessionSecret = randomBytes(32).toString('hex');
  writeFileSync(secretFile, sessionSecret, { mode: 0o600 });
  step(`Generated a session secret and saved it to ${secretFile}`);
}

// --- Bind address ------------------------------------------------------------
const bindAddr = `${args.bindAll ? '0.0.0.0' : '127.0.0.1'}:${port}`;
if (args.bindAll) warn('Binding 0.0.0.0 - only do this behind HTTPS (reverse proxy) or a VPN.');

// --- Env + run ---------------------------------------------------------------
const env = {
  ...process.env,
  RUSTIC_AUTH_PASSWORD: password,
  RUSTIC_SESSION_SECRET: sessionSecret,
  RUSTIC_BIND_ADDR: bindAddr,
  RUSTIC_DATA_DIR: dataDir,
  RUSTIC_STATIC_DIR: staticDir,
  RUST_LOG: process.env.RUST_LOG || 'info,reqwest=warn,hyper=warn,tower=warn,h2=warn,rustls=warn',
};
const url = `http://127.0.0.1:${port}`;
console.log('');
info('Starting rustic-server');
step(`url      : ${url}`);
step(`bind     : ${bindAddr}`);
step(`password : ${password}`);
step(`data dir : ${dataDir}`);
step(`static   : ${staticDir}`);
console.log('');
ok(`Open ${url} in your browser, then log in. Stop with Ctrl-C.`);
console.log('');

// Foreground child; Ctrl-C reaches it via the shared console/process group, and
// we just mirror its exit status.
const child = spawn(binary, [], { cwd: ROOT, env, stdio: 'inherit' });
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => child.kill(sig));
child.on('exit', (code, signal) => process.exit(code ?? (signal ? 130 : 1)));
child.on('error', (e) => die(`failed to start ${binary}: ${e.message}`));
