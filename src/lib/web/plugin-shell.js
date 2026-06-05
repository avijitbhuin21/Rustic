// Web shim for `@tauri-apps/plugin-shell`.
// The only API the frontend uses is `open(url)` to launch a link/path in the
// OS default handler. In a browser that's just a new tab.
export async function open(target) {
  // Bare filesystem paths (no scheme) can't be opened by the browser; only
  // open things that look like URLs. Anything else is a no-op (the desktop
  // behavior of "reveal in OS" has no browser equivalent).
  if (/^[a-z]+:\/\//i.test(target) || target.startsWith('mailto:')) {
    window.open(target, '_blank', 'noopener,noreferrer');
  } else {
    console.warn('[web] shell.open ignored for non-URL target:', target);
  }
}

// `Command` (spawning processes) has no browser equivalent. Stub that throws so
// any accidental use surfaces loudly rather than silently no-op'ing.
export class Command {
  constructor() {
    throw new Error('shell.Command is not available in the web build');
  }
}
