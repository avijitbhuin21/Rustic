import { settingsStore, updateSetting } from '../state/settings.js';

const MIN_SCALE = 0.5;
const MAX_SCALE = 2.0;
const STEP = 0.1;

let zoomNotifEl = null;
let zoomNotifTimer = null;

function showZoomNotification(scale) {
  if (!zoomNotifEl) {
    zoomNotifEl = document.createElement('div');
    zoomNotifEl.className = 'zoom-notification';
    document.body.appendChild(zoomNotifEl);
  }
  zoomNotifEl.textContent = `Zoom: ${Math.round(scale * 100)}%`;
  zoomNotifEl.classList.add('visible');

  clearTimeout(zoomNotifTimer);
  zoomNotifTimer = setTimeout(() => {
    zoomNotifEl.classList.remove('visible');
  }, 1200);
}

function getScale() {
  const settings = settingsStore.getState('settings');
  return settings?.general?.ui_scale ?? 1.0;
}

function applyZoom(scale) {
  const app = document.getElementById('app');
  if (!app) return;

  // Zoom only #app — the top bar is a sibling on body, unaffected.
  app.style.zoom = scale;

  // At zoom X the app's CSS-px map to fewer visual pixels, so we
  // need (viewport / scale) CSS-px to fill the remaining space.
  // 35px is the fixed top-bar height, status-bar-height is the status bar.
  app.style.height =
    `calc(${100 / scale}vh - ${35 / scale}px - calc(var(--status-bar-height) / ${scale}))`;

  showZoomNotification(scale);
}

export async function zoomIn() {
  const next = Math.min(MAX_SCALE, Math.round((getScale() + STEP) * 10) / 10);
  applyZoom(next);
  await updateSetting('general.ui_scale', next);
}

export async function zoomOut() {
  const next = Math.max(MIN_SCALE, Math.round((getScale() - STEP) * 10) / 10);
  applyZoom(next);
  await updateSetting('general.ui_scale', next);
}

export async function resetZoom() {
  applyZoom(1.0);
  await updateSetting('general.ui_scale', 1.0);
}

/** Apply saved zoom level on startup and listen for changes */
export function initZoom() {
  applyZoom(getScale());

  settingsStore.subscribe('settings', (settings) => {
    if (settings?.general?.ui_scale != null) {
      applyZoom(settings.general.ui_scale);
    }
  });
}
