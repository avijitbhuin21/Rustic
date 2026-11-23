// Embeds the live theme preview (theme-preview.html) in a page. Framework-free
// and import-free on purpose: the app imports it, and the website gets a copy
// (scripts/build-theme-preview.mjs). The preview runs in an iframe because a
// theme repaints the whole document; it must be served from the same origin as
// the page, and messages from anywhere else are ignored on both sides.

export const PREVIEW_SOURCE = 'rustic-theme-preview';
export const HOST_SOURCE = 'rustic-theme-preview-host';
export const SCENES = ['workbench', 'dialog', 'menu'];
/** Size the mock window is laid out at; hosts scale it down to fit. */
export const PREVIEW_WIDTH = 1280;
export const PREVIEW_HEIGHT = 800;

/**
 * Mount the preview inside `container` and return `{ setTheme, setScene, destroy }`.
 * `src` is the preview page URL, `pack` a theme file object, `mode` "dark" or
 * "light" (only matters for themes with both), `onProblems(problems, info)`
 * reports values the preview ignored.
 */
export function mountThemePreview(container, { src, pack = null, mode = 'dark', scene = 'workbench', onProblems } = {}) {
  const state = { pack, mode, scene, ready: false, frameId: 0 };
  const wrap = document.createElement('div');
  wrap.style.cssText = `position:relative;width:100%;aspect-ratio:${PREVIEW_WIDTH}/${PREVIEW_HEIGHT};overflow:hidden`;
  const frame = document.createElement('iframe');
  frame.title = 'Rustic theme preview';
  frame.setAttribute('scrolling', 'no');
  frame.style.cssText = `position:absolute;left:0;top:0;width:${PREVIEW_WIDTH}px;height:${PREVIEW_HEIGHT}px;border:0;transform-origin:0 0`;
  frame.src = src;
  wrap.appendChild(frame);
  container.appendChild(wrap);

  const fit = () => {
    frame.style.transform = `scale(${wrap.clientWidth / PREVIEW_WIDTH})`;
  };
  const resize = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(fit);
  resize?.observe(wrap);
  fit();

  const post = (message) => {
    if (!state.ready) return;
    frame.contentWindow?.postMessage({ source: HOST_SOURCE, ...message }, window.location.origin);
  };
  const sendTheme = () => {
    cancelAnimationFrame(state.frameId);
    state.frameId = requestAnimationFrame(() => {
      if (state.pack) post({ type: 'theme', pack: state.pack, mode: state.mode });
    });
  };
  const onMessage = (e) => {
    if (e.source !== frame.contentWindow || e.origin !== window.location.origin) return;
    const data = e.data;
    if (!data || data.source !== PREVIEW_SOURCE) return;
    if (data.type === 'ready') {
      state.ready = true;
      post({ type: 'scene', scene: state.scene });
      sendTheme();
    } else if (data.type === 'problems') {
      onProblems?.(data.problems, { mode: data.mode, modes: data.modes });
    }
  };
  window.addEventListener('message', onMessage);

  return {
    /** Show `nextPack`; calls made in the same frame collapse into one repaint. */
    setTheme(nextPack, nextMode = state.mode) {
      state.pack = nextPack;
      state.mode = nextMode;
      sendTheme();
    },
    /** Switch what the mock window shows: one of SCENES. */
    setScene(nextScene) {
      state.scene = nextScene;
      post({ type: 'scene', scene: nextScene });
    },
    destroy() {
      window.removeEventListener('message', onMessage);
      cancelAnimationFrame(state.frameId);
      resize?.disconnect();
      wrap.remove();
    },
  };
}
