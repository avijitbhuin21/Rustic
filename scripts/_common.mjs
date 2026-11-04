// Shared helpers for the cross-platform Node scripts in scripts/ (Node built-ins only).
import { spawnSync } from 'node:child_process';
import { existsSync, statSync } from 'node:fs';
import { createInterface } from 'node:readline';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const IS_WIN = process.platform === 'win32';
export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const color = (code, s) => (process.stdout.isTTY ? `\x1b[${code}m${s}\x1b[0m` : s);
export const info = (m) => console.log(color(36, `==> ${m}`));
export const step = (m) => console.log(`  - ${m}`);
export const ok = (m) => console.log(color(32, `  OK ${m}`));
export const warn = (m) => console.log(color(33, `  ! ${m}`));
export function die(m) {
  console.error(process.stderr.isTTY ? `\x1b[31mERROR: ${m}\x1b[0m` : `ERROR: ${m}`);
  process.exit(1);
}

/** True when the script file was invoked directly (`node scripts/x.mjs`). */
export const isMain = (metaUrl) =>
  !!process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(metaUrl);

/** Resolve an executable on PATH (honours PATHEXT on Windows). */
export function which(cmd) {
  const exts = IS_WIN ? (process.env.PATHEXT || '.EXE;.CMD;.BAT;.COM').split(';') : [''];
  for (const dir of (process.env.PATH || '').split(path.delimiter)) {
    if (!dir) continue;
    for (const ext of exts) {
      const p = path.join(dir, cmd + ext.toLowerCase());
      const P = path.join(dir, cmd + ext);
      for (const c of new Set([p, P])) {
        try { if (statSync(c).isFile()) return c; } catch { /* not here */ }
      }
    }
  }
  return null;
}

/**
 * Run a command synchronously from the repo root. Never goes through a shell
 * except for Windows .cmd/.bat shims, whose args are then quoted explicitly.
 * Options: quiet (discard output), capture (return stdout), input (stdin text), env.
 */
export function run(cmd, args = [], { quiet = false, capture = false, input, env } = {}) {
  const exe = which(cmd) || cmd;
  const shim = IS_WIN && /\.(cmd|bat)$/i.test(exe);
  const stdout = capture ? 'pipe' : quiet ? 'ignore' : 'inherit';
  const stdio = [input !== undefined ? 'pipe' : 'inherit', stdout, quiet ? 'ignore' : 'inherit'];
  const r = shim
    ? spawnSync(`"${exe}" ${args.map((a) => `"${String(a).replace(/"/g, '""')}"`).join(' ')}`, {
        cwd: ROOT, stdio, input, env, shell: true, encoding: 'utf8',
      })
    : spawnSync(exe, args, { cwd: ROOT, stdio, input, env, encoding: 'utf8' });
  return { code: r.error ? -1 : r.status ?? 1, stdout: (r.stdout || '').trim() };
}

/**
 * Parse argv into { name: value|true }. Accepts GNU style (`--skip-web-build`,
 * `--port 9000`, `--port=9000`) and the old PowerShell style (`-SkipWebBuild`,
 * `-Port 9000`): names are lower-cased with dashes removed before lookup.
 * `spec` maps normalized names to { key, value: bool }.
 */
export function parseArgs(argv, spec) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('-') || /^-\d/.test(a)) { out._.push(a); continue; }
    const [rawName, inline] = a.split(/=(.*)/s, 2);
    const norm = rawName.replace(/-/g, '').toLowerCase();
    if (norm === 'h' || norm === 'help' || norm === '?') { out.help = true; continue; }
    const s = spec[norm];
    if (!s) die(`unknown argument: ${a}`);
    if (s.value) {
      const v = inline !== undefined ? inline : argv[++i];
      if (v === undefined) die(`${rawName} needs a value`);
      out[s.key] = v;
    } else out[s.key] = true;
  }
  return out;
}

/** Ask a y/N question on the terminal; resolves true only for y/Y. */
export function confirm(question) {
  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout });
    rl.question(`${question} (y/N) `, (ans) => { rl.close(); resolve(/^y$/i.test(ans.trim())); });
    rl.on('close', () => resolve(false));
  });
}

export const exists = (p) => existsSync(p);
