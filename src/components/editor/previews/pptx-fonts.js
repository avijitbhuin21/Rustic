import { createFontSource } from '@pptx-editor.dev/react';
import { defineFontResolver } from '@pptx-editor.dev/core/editor';
import geistRegular from '@/assets/fonts/geist/Geist-Regular.ttf?url';
import geistBold from '@/assets/fonts/geist/Geist-Bold.ttf?url';
import geistItalic from '@/assets/fonts/geist/Geist-Italic.ttf?url';
import geistBoldItalic from '@/assets/fonts/geist/Geist-BoldItalic.ttf?url';
import geistLight from '@/assets/fonts/geist/Geist-Light.ttf?url';
import geistLightItalic from '@/assets/fonts/geist/Geist-LightItalic.ttf?url';
import geistMedium from '@/assets/fonts/geist/Geist-Medium.ttf?url';
import geistMediumItalic from '@/assets/fonts/geist/Geist-MediumItalic.ttf?url';
import geistSemiBold from '@/assets/fonts/geist/Geist-SemiBold.ttf?url';
import geistSemiBoldItalic from '@/assets/fonts/geist/Geist-SemiBoldItalic.ttf?url';

// PowerPoint names each Geist weight as its own family ("Geist SemiBold"),
// so every weight is the regular face of a family, as in the fonts' name tables.
const FACES = [
  { family: 'Geist', weight: 400, style: 'normal', url: geistRegular },
  { family: 'Geist', weight: 700, style: 'normal', url: geistBold },
  { family: 'Geist', weight: 400, style: 'italic', url: geistItalic },
  { family: 'Geist', weight: 700, style: 'italic', url: geistBoldItalic },
  { family: 'Geist Light', weight: 400, style: 'normal', url: geistLight },
  { family: 'Geist Light', weight: 400, style: 'italic', url: geistLightItalic },
  { family: 'Geist Medium', weight: 400, style: 'normal', url: geistMedium },
  { family: 'Geist Medium', weight: 400, style: 'italic', url: geistMediumItalic },
  { family: 'Geist SemiBold', weight: 400, style: 'normal', url: geistSemiBold },
  { family: 'Geist SemiBold', weight: 400, style: 'italic', url: geistSemiBoldItalic },
];

const cache = new Map();

/** Fetches one bundled font file, once per session. */
function fontBytes(url) {
  let pending = cache.get(url);
  if (!pending) {
    pending = fetch(url)
      .then((res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.arrayBuffer();
      })
      .then((buffer) => new Uint8Array(buffer));
    pending.catch(() => cache.delete(url));
    cache.set(url, pending);
  }
  return pending;
}

/** Font origin for the editor: the bundled Geist faces of the Geist families a deck names. */
export const bundledFonts = defineFontResolver(async ({ families }) => {
  const named = new Map(families.map((family) => [family.toLowerCase(), family]));
  const faces = FACES.filter((face) => named.has(face.family.toLowerCase()));
  if (faces.length === 0) return undefined;
  const sources = [];
  await Promise.all(
    faces.map(async (face) => {
      try {
        const request = { family: named.get(face.family.toLowerCase()), weight: face.weight, style: face.style };
        const made = createFontSource(await fontBytes(face.url), request, { id: face.url });
        if ('source' in made) sources.push(made.source);
        else console.warn('pptx: bundled font refused', face.url, made.failure?.reason);
      } catch (e) {
        // That face stays on the editor's stand-in font.
        console.warn('pptx: bundled font did not load', face.url, e);
      }
    }),
  );
  return sources.length ? { sources } : undefined;
});
