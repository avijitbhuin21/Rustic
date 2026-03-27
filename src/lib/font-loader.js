/**
 * Font loader - handles loading fonts from URLs (Google Fonts, direct file URLs)
 */

/**
 * Load font(s) from a URL. Supports:
 * - Google Fonts share URLs (single or multiple fonts separated by |)
 * - Google Fonts specimen URLs
 * - Google Fonts CSS API URLs
 * - Direct font file URLs (.woff2, .ttf, .otf, .woff)
 *
 * Returns an array of { name, url } for each loaded font.
 * Returns empty array on failure.
 */
export async function loadFontFromUrl(url) {
  try {
    const cssUrls = resolveGoogleFontsUrls(url);

    if (cssUrls.length > 0) {
      // Google Fonts — could be multiple CSS URLs (one per font family)
      const results = [];
      for (const cssUrl of cssUrls) {
        const names = await loadGoogleFont(cssUrl);
        for (const name of names) {
          results.push({ name, url: cssUrl });
        }
      }
      return results;
    } else if (isDirectFontUrl(url)) {
      const name = await loadDirectFont(url);
      return name ? [{ name, url }] : [];
    } else {
      // Try as CSS URL first, then as direct font
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
 * Convert Google Fonts share/specimen URLs into one or more CSS API URLs.
 *
 * Share URL with multiple families:
 *   fonts.google.com/share?selection.family=Playfair+Display:ital,wght@0,400..900;1,400..900|Roboto+Slab:wght@100..900
 *   -> two CSS URLs, one per family
 *
 * Specimen URL:
 *   fonts.google.com/specimen/Playfair+Display
 *   -> one CSS URL
 *
 * Returns an array of googleapis.com CSS URLs (empty if not a Google Fonts URL).
 */
function resolveGoogleFontsUrls(url) {
  try {
    const u = new URL(url);
    if (u.hostname !== 'fonts.google.com') return [];

    // Share links: ?selection.family=Family1:opts|Family2:opts
    if (u.searchParams.has('selection.family')) {
      const raw = u.searchParams.get('selection.family');
      // Split on | to get individual families
      const families = raw.split('|').map((f) => f.trim()).filter(Boolean);
      return families.map((f) => `https://fonts.googleapis.com/css2?family=${encodeFont(f)}&display=swap`);
    }

    // Specimen links: /specimen/Playfair+Display
    if (u.pathname.startsWith('/specimen/')) {
      const family = decodeURIComponent(u.pathname.replace('/specimen/', ''));
      return [`https://fonts.googleapis.com/css2?family=${encodeFont(family)}&display=swap`];
    }
  } catch { /* not a valid URL */ }
  return [];
}

/**
 * Encode a font family string for the Google Fonts CSS API.
 * Spaces become +, colons and commas are kept, but special URL chars are encoded.
 */
function encodeFont(family) {
  // family looks like "Playfair Display:ital,wght@0,400..900;1,400..900"
  // The API expects: family=Playfair+Display:ital,wght@0,400..900;1,400..900
  // So encode spaces as + but keep :,;@. intact
  return family.replace(/ /g, '+');
}

function isGoogleFontsUrl(url) {
  return url.includes('fonts.googleapis.com') || url.includes('fonts.gstatic.com');
}

function isDirectFontUrl(url) {
  return /\.(woff2?|ttf|otf)(\?.*)?$/i.test(url);
}

/**
 * Load a Google Fonts CSS URL by fetching the CSS and extracting @font-face rules.
 * Returns an array of font family names found in the CSS.
 */
async function loadGoogleFont(cssUrl) {
  const response = await fetch(cssUrl, {
    headers: {
      'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36',
    },
  });

  if (!response.ok) throw new Error(`Failed to fetch font CSS: ${response.status}`);

  const cssText = await response.text();

  // Inject the CSS into the document
  const styleEl = document.createElement('style');
  styleEl.textContent = cssText;
  document.head.appendChild(styleEl);

  // Extract ALL unique font family names from CSS
  const familyRegex = /font-family:\s*['"]?([^;'"]+)['"]?\s*;/g;
  const names = new Set();
  let match;
  while ((match = familyRegex.exec(cssText)) !== null) {
    names.add(match[1].trim());
  }

  if (names.size > 0) {
    await document.fonts.ready;
    // Give fonts a bit more time if needed
    await new Promise((resolve) => setTimeout(resolve, 500));
    await document.fonts.ready;
    return [...names];
  }

  return [];
}

/**
 * Load a font directly from a URL (.woff2, .ttf, .otf).
 */
async function loadDirectFont(fontUrl) {
  const urlPath = new URL(fontUrl).pathname;
  const fileName = urlPath.split('/').pop();
  const fontName = fileName.replace(/\.[^.]+$/, '').replace(/[-_]/g, ' ');

  const fontFace = new FontFace(fontName, `url(${fontUrl})`);
  await fontFace.load();
  document.fonts.add(fontFace);

  return fontName;
}
