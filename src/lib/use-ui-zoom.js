import { useEffect } from 'react';

const ZOOM_KEY = 'rustic:ui-zoom';
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 2.5;
const STEP = 0.1;

function clamp(z) {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));
}

function clearCssZoomLeftovers() {
  // Earlier versions of this hook used `body.style.zoom`. CSS `zoom` breaks
  // Floating-UI / Radix positioning math (tooltips shift off their triggers).
  // Strip it so positioning calculations stay accurate; we now use Tauri's
  // native webview zoom instead.
  if (document.body.style.zoom) document.body.style.zoom = '';
  document.documentElement.style.removeProperty('--ui-zoom');
}

async function getWebview() {
  try {
    const mod = await import('@tauri-apps/api/webview');
    return mod.getCurrentWebview();
  } catch {
    return null;
  }
}

function read() {
  const raw = localStorage.getItem(ZOOM_KEY);
  const parsed = raw ? parseFloat(raw) : 1;
  return Number.isFinite(parsed) ? clamp(parsed) : 1;
}

function write(zoom) {
  localStorage.setItem(ZOOM_KEY, String(zoom));
}

export function useUiZoom() {
  useEffect(() => {
    clearCssZoomLeftovers();

    let current = read();
    let webview = null;
    let cancelled = false;

    async function apply(zoom) {
      if (!webview) return;
      try {
        await webview.setZoom(zoom);
      } catch (err) {
        console.warn('webview setZoom failed:', err);
      }
    }

    getWebview().then((wv) => {
      if (cancelled) return;
      webview = wv;
      apply(current);
    });

    function set(next) {
      current = clamp(Math.round(next * 100) / 100);
      write(current);
      apply(current);
    }

    function onKeyDown(e) {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === '=' || e.key === '+') {
        e.preventDefault();
        set(current + STEP);
      } else if (e.key === '-' || e.key === '_') {
        e.preventDefault();
        set(current - STEP);
      } else if (e.key === '0') {
        e.preventDefault();
        set(1);
      }
    }

    function onWheel(e) {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      set(current + (e.deltaY < 0 ? STEP : -STEP));
    }

    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      cancelled = true;
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('wheel', onWheel);
    };
  }, []);
}
