/**
 * Load font(s) from a URL. Supports:
 * - Google Fonts share/specimen/CSS API URLs
 * - Direct font file URLs (.woff2, .ttf, .otf, .woff)
 *
 * Returns an array of { name, url } for each loaded font family.
 */
export async function loadFontFromUrl(url) {
  try {
    const cssUrls = resolveGoogleFontsUrls(url);
    if (cssUrls.length > 0) {
      const results = [];
      for (const cssUrl of cssUrls) {
        const names = await loadGoogleFont(cssUrl);
        for (const name of names) results.push({ name, url: cssUrl });
      }
      return results;
    } else if (isDirectFontUrl(url)) {
      const name = await loadDirectFont(url);
      return name ? [{ name, url }] : [];
    } else {
      try {
        const names = await loadGoogleFont(url);
        return names.map((name) => ({ name, url }));
      } catch {
        const name = await loadDirectFont(url);
        return name ? [{ name, url }] : [];
      }
    }
  } catch (e) {
    console.error('Font loading failed:', e);
    return [];
  }
}

/**
 * Load a font from raw bytes (Uint8Array). Used for local file imports.
 * Returns the font family name, or null on failure.
 */
export async function loadFontFromBytes(name, bytes) {
  try {
    const blob = new Blob([bytes]);
    const url = URL.createObjectURL(blob);
    const face = new FontFace(name, `url(${url})`);
    await face.load();
    document.fonts.add(face);
    return name;
  } catch (e) {
    console.error('Failed to load font from bytes:', e);
    return null;
  }
}

function resolveGoogleFontsUrls(url) {
  try {
    const u = new URL(url);
    if (u.hostname !== 'fonts.google.com') return [];
    if (u.searchParams.has('selection.family')) {
      const raw = u.searchParams.get('selection.family');
      return raw.split('|').map((f) => f.trim()).filter(Boolean)
        .map((f) => `https://fonts.googleapis.com/css2?family=${f.replace(/ /g, '+')}&display=swap`);
    }
    if (u.pathname.startsWith('/specimen/')) {
      const family = decodeURIComponent(u.pathname.replace('/specimen/', ''));
      return [`https://fonts.googleapis.com/css2?family=${family.replace(/ /g, '+')}&display=swap`];
    }
  } catch { /* not a valid URL */ }
  return [];
}

function isDirectFontUrl(url) {
  return /\.(woff2?|ttf|otf)(\?.*)?$/i.test(url);
}

async function loadGoogleFont(cssUrl) {
  const res = await fetch(cssUrl, {
    headers: { 'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36' },
  });
  if (!res.ok) throw new Error(`Failed to fetch font CSS: ${res.status}`);
  const css = await res.text();
  const style = document.createElement('style');
  style.textContent = css;
  document.head.appendChild(style);
  const names = new Set();
  const re = /font-family:\s*['"]?([^;'"]+)['"]?\s*;/g;
  let m;
  while ((m = re.exec(css)) !== null) names.add(m[1].trim());
  if (names.size > 0) {
    await document.fonts.ready;
    await new Promise((r) => setTimeout(r, 500));
    await document.fonts.ready;
    return [...names];
  }
  return [];
}

async function loadDirectFont(fontUrl) {
  const path = new URL(fontUrl).pathname;
  const file = path.split('/').pop();
  const name = file.replace(/\.[^.]+$/, '').replace(/[-_]/g, ' ');
  const face = new FontFace(name, `url(${fontUrl})`);
  await face.load();
  document.fonts.add(face);
  return name;
}
