// Projects whose pages may run their own scripts in the film view. Asked once per
// project: a film page is sandboxed, but it could still send the files it can read
// out over WebRTC, so the user decides per project. The live HTML preview keeps its
// own list because its pages may also reach the internet, so its prompt says more.
const KEY = 'rustic.film.allowedProjects';
const PAGE_KEY = 'rustic.htmlPreview.scriptProjects';

const norm = (p) => String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();

/** Stored list of allowed project roots. */
function read(key = KEY) {
  try {
    const list = JSON.parse(localStorage.getItem(key) || '[]');
    return Array.isArray(list) ? list.filter((p) => typeof p === 'string') : [];
  } catch {
    return [];
  }
}

/** Add a project root to a stored allow list. */
function allow(key, projectRoot) {
  const root = norm(projectRoot);
  if (!root) return;
  const list = read(key);
  if (list.includes(root)) return;
  try {
    localStorage.setItem(key, JSON.stringify([...list, root]));
  } catch { /* storage unavailable: the prompt shows again next time */ }
}

/** True when the user has allowed film pages in the project at `projectRoot`. */
export function isFilmAllowed(projectRoot) {
  const key = norm(projectRoot);
  return !!key && read().includes(key);
}

/** Remember that film pages in the project at `projectRoot` may run their scripts. */
export function allowFilm(projectRoot) {
  allow(KEY, projectRoot);
}

/** True when HTML pages in the project at `projectRoot` may run their scripts in the preview. */
export function isPageScriptsAllowed(projectRoot) {
  const key = norm(projectRoot);
  return !!key && read(PAGE_KEY).includes(key);
}

/** Remember that HTML pages in the project at `projectRoot` may run their scripts in the preview. */
export function allowPageScripts(projectRoot) {
  allow(PAGE_KEY, projectRoot);
}
