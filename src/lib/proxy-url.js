/**
 * Map an embedded-browser tab URL to a URL the user can open in their OWN
 * browser. Loopback dev-server URLs (localhost:3000/…) are rewritten to either
 * the subdomain tunnel (`https://3000.<previewDomain>/…`, when configured) or
 * the same-origin path tunnel (`/proxy/3000/…`); public URLs are returned
 * as-is. Returns null for things that can't be opened (about:blank,
 * unparseable, non-http schemes).
 */
const LOOPBACK_HOSTS = new Set([
  'localhost',
  '127.0.0.1',
  '0.0.0.0',
  '::1',
  '[::1]',
]);

export function tabExternalUrl(rawUrl, previewDomain = null) {
  if (!rawUrl || rawUrl === 'about:blank') return null;

  let u;
  try {
    u = new URL(rawUrl);
  } catch {
    return null;
  }

  if (u.protocol !== 'http:' && u.protocol !== 'https:') return null;

  if (LOOPBACK_HOSTS.has(u.hostname)) {
    const port = u.port || (u.protocol === 'https:' ? '443' : '80');
    const tail = `${u.pathname}${u.search}${u.hash}`;
    if (previewDomain) {
      return `https://${port}.${previewDomain}${tail}`;
    }
    return `${window.location.origin}/proxy/${port}${tail}`;
  }

  return rawUrl;
}
