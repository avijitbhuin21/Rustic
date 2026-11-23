import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Hand, MousePointerClick, RefreshCw, ShieldAlert } from 'lucide-react';
import { basename } from '@/state/editor';
import { isPageScriptsAllowed, allowPageScripts } from '@/lib/film-consent';
import {
  CanvasControls,
  DEVICE_PRESETS,
  PreviewCanvas,
  useCanvasView,
} from './preview-canvas';

function parentDir(path) {
  const norm = path.replace(/\\/g, '/');
  const i = norm.lastIndexOf('/');
  return i < 0 ? '' : norm.slice(0, i);
}

function isAbsoluteUrl(href) {
  if (!href) return false;
  return /^(?:[a-z]+:)?\/\//i.test(href) || href.startsWith('data:') || href.startsWith('blob:') || href.startsWith('#');
}

function joinPath(dir, rel) {
  if (!dir) return rel;
  // Strip any "./" prefix, collapse "../" by walking up `dir`.
  const segs = rel.replace(/\\/g, '/').split('/');
  const baseSegs = dir.replace(/\\/g, '/').split('/');
  for (const s of segs) {
    if (s === '' || s === '.') continue;
    if (s === '..') { baseSegs.pop(); continue; }
    baseSegs.push(s);
  }
  return baseSegs.join('/');
}

const IMG_MIME = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  svg: 'image/svg+xml',
  avif: 'image/avif',
};

function extOf(p) {
  const i = p.lastIndexOf('.');
  return i < 0 ? '' : p.slice(i + 1).toLowerCase();
}

// Inline every reachable local resource into the HTML so an iframe srcdoc
// can render it without needing the Tauri asset protocol or a custom
// scheme. Returns the rewritten HTML. Anything we can't resolve is left
// as-is so external CDNs (and absolute URLs) still work via CSP.
async function inlineLocalResources(html, htmlPath) {
  const baseDir = parentDir(htmlPath);
  const parser = new DOMParser();
  const doc = parser.parseFromString(html, 'text/html');

  // <link rel="stylesheet" href="..."> — read the CSS and convert to a
  // <style> tag. We do this for relative refs only; CDN stylesheets stay
  // as <link> so the iframe fetches them like any browser would.
  const links = Array.from(doc.querySelectorAll('link[rel~="stylesheet"]'));
  await Promise.all(
    links.map(async (link) => {
      const href = link.getAttribute('href') || '';
      if (!href || isAbsoluteUrl(href)) return;
      try {
        const target = joinPath(baseDir, href);
        const css = await invoke('read_file_content', { path: target });
        const style = doc.createElement('style');
        style.setAttribute('data-rustic-inlined-from', href);
        style.textContent = String(css ?? '');
        link.replaceWith(style);
      } catch {
        // Leave the link in place; the iframe will simply 404 it.
      }
    }),
  );

  // <img src="..."> and similar — convert relative paths to data URLs so
  // the iframe doesn't try to fetch from the bare file path (which it
  // can't from a srcdoc context). Skip data:/blob:/http(s):/absolute.
  const imgs = Array.from(doc.querySelectorAll('img[src]'));
  await Promise.all(
    imgs.map(async (img) => {
      const src = img.getAttribute('src') || '';
      if (!src || isAbsoluteUrl(src)) return;
      try {
        const target = joinPath(baseDir, src);
        const res = await invoke('read_file_base64', { path: target });
        const mime = IMG_MIME[extOf(target)] || 'application/octet-stream';
        img.setAttribute('src', `data:${mime};base64,${res.data}`);
      } catch {
        // Leave src as-is.
      }
    }),
  );

  // We deliberately skip <script src="..."> rewriting. Inlining arbitrary
  // local JS into a srcdoc iframe is the wrong default — the user might be
  // previewing a malicious file. Authors who want their script to run can
  // inline it themselves.
  return '<!DOCTYPE html>\n' + doc.documentElement.outerHTML;
}

const MAX_FRAME_HEIGHT = 60000;
const RELOAD_FILES = /\.(m?js|css|json|html?)$/i;
const IGNORED_DIRS = /\/(node_modules|dist|build|out|target|\.[^/]+)\//;
const normPath = (p) => String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
const num = (v, lim) => {
  const n = Number(v);
  return Number.isFinite(n) ? Math.max(-lim, Math.min(lim, n)) : 0;
};

// Rendered preview. Editing is handled by the Monaco editor via the shared
// Edit ⇄ Preview toggle in editor-pane.jsx. Pages run live (their own scripts,
// sandboxed in a preview session, frames grown to the full page height) once the
// project is allowed; otherwise, or where sessions aren't available, they render
// script-free from inlined HTML at the device height.
export default function HtmlPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);
  // `inlinedHtml` is the iframe-ready HTML (relative CSS / images resolved
  // to inline content). Async because resolution itself is async, so we
  // hold it in state. `inliningId` discards stale results.
  const [inlinedHtml, setInlinedHtml] = useState('');
  const inliningIdRef = useRef(0);

  const [activeIds, setActiveIds] = useState(['desktop']);
  const [customSize, setCustomSize] = useState({ width: 1280, height: 720 });
  // Interact (page gets clicks; wheel still scrolls the canvas) vs. drag-to-pan.
  const [interactive, setInteractive] = useState(true);

  // Live session: { status: 'pending' | 'ready' | 'off', session?, reason? }.
  const [live, setLive] = useState({ status: 'pending' });
  const [allowed, setAllowed] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [heights, setHeights] = useState({});
  const session = live.status === 'ready' ? live.session : null;
  const running = !!session && allowed;

  const frames = useMemo(() => {
    const list = DEVICE_PRESETS.filter((p) => activeIds.includes(p.id)).map((p) => ({ ...p }));
    if (activeIds.includes('custom')) {
      list.push({ id: 'custom', label: 'Custom', ...customSize });
    }
    const base = list.length ? list : [{ id: 'desktop', label: 'Desktop', width: 1440, height: 900 }];
    return base.map((f) => {
      const key = `${session?.id ?? 'static'}:${f.id}:${f.width}x${f.height}`;
      return { ...f, key, viewportHeight: f.height, height: running ? heights[key] ?? f.height : f.height };
    });
  }, [activeIds, customSize, session, running, heights]);

  const canvas = useCanvasView(frames, { wheel: 'scroll', fit: 'width' });
  const canvasRef = useRef(canvas);
  canvasRef.current = canvas;
  const frameEls = useRef(new Map());

  const toggleFrame = (id) =>
    setActiveIds((prev) => {
      const next = prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id];
      return next.length ? next : prev;
    });

  const reloadVersion = useFileReloadVersion(tab.path);

  // One preview session per (page, reload). Frames remount at device height on a new
  // session so the bridge reads the right viewport height before growing them again.
  useEffect(() => {
    let cancelled = false;
    let id = null;
    invoke('page_preview_open', { path: tab.path })
      .then((s) => {
        if (cancelled) {
          invoke('film_close', { id: s.id }).catch(() => {});
          return;
        }
        id = s.id;
        setAllowed(isPageScriptsAllowed(s.project));
        setLive({ status: 'ready', session: s });
      })
      .catch((e) => {
        if (!cancelled) setLive({ status: 'off', reason: String(e?.message || e) });
      });
    return () => {
      cancelled = true;
      if (id) invoke('film_close', { id }).catch(() => {});
    };
  }, [tab.path, reloadVersion, nonce]);

  // Messages from the page bridge. The page is untrusted: accept only messages from
  // the window directly inside one of our frames, and validate every field.
  useEffect(() => {
    if (!running) return undefined;
    const onMessage = (e) => {
      const d = e.data;
      if (!d || d.source !== 'rustic-page' || !e.source) return;
      let frame = null;
      for (const f of frameEls.current.values()) {
        const shell = f.el?.contentWindow;
        let inside = false;
        try { inside = !!shell && e.source !== shell && e.source.parent === shell; } catch { inside = false; }
        if (inside) { frame = f; break; }
      }
      if (!frame) return;
      const c = canvasRef.current;
      if (d.type === 'size') {
        const h = Math.round(num(d.height, MAX_FRAME_HEIGHT));
        if (h < 64) return;
        setHeights((prev) => (prev[frame.key] === h ? prev : { ...prev, [frame.key]: h }));
      } else if (d.type === 'wheel') {
        c.wheelFromFrame(frame.id, {
          dx: num(d.dx, 5000), dy: num(d.dy, 5000), x: num(d.x, 1e5), y: num(d.y, 1e5),
          ctrl: d.ctrl === true, meta: d.meta === true, shift: d.shift === true,
        });
      } else if (d.type === 'anchor') {
        c.scrollToFrameY(frame.id, Math.max(0, num(d.y, MAX_FRAME_HEIGHT)));
      } else if (d.type === 'link') {
        const url = typeof d.url === 'string' ? d.url.slice(0, 2000) : '';
        if (!/^(https?:|mailto:)/i.test(url)) return;
        // The page can post this without a click, so the user confirms every open.
        toast(`Open link from the preview?`, {
          id: 'html-preview-link',
          description: url,
          action: { label: 'Open', onClick: () => openUrl(url).catch((err) => toast.error(`Failed to open link: ${err}`)) },
        });
      }
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [running]);

  // Reload when the page's styles or scripts change on disk, not just the page itself.
  const root = running ? session.root : null;
  useEffect(() => {
    if (!root) return undefined;
    const base = normPath(root);
    let timer = null;
    let unlisten = null;
    let disposed = false;
    listen('rustic:fs-change', (e) => {
      const paths = Array.isArray(e.payload?.changed_paths) ? e.payload.changed_paths : [];
      const hit = paths.some((p) => {
        const n = normPath(p);
        return n.startsWith(`${base}/`) && RELOAD_FILES.test(n) && !IGNORED_DIRS.test(n.slice(base.length));
      });
      if (!hit) return;
      clearTimeout(timer);
      timer = setTimeout(() => setNonce((v) => v + 1), 400);
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    }).catch(() => {});
    return () => {
      disposed = true;
      clearTimeout(timer);
      if (typeof unlisten === 'function') unlisten();
    };
  }, [root]);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setText(null);
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (!cancelled) setText(c ?? '');
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path, reloadVersion]);

  // Re-inline whenever the file content changes.
  useEffect(() => {
    if (text == null) return;
    const id = ++inliningIdRef.current;
    inlineLocalResources(text, tab.path)
      .then((html) => {
        if (id === inliningIdRef.current) setInlinedHtml(html);
      })
      .catch(() => {
        // Fall back to the raw text so the user at least sees something.
        if (id === inliningIdRef.current) setInlinedHtml(text);
      });
  }, [text, tab.path]);

  const reload = () => {
    if (running) {
      setNonce((v) => v + 1);
      return;
    }
    if (text == null) return;
    inliningIdRef.current += 1;
    inlineLocalResources(text, tab.path)
      .then(setInlinedHtml)
      .catch(() => setInlinedHtml(text));
  };

  // Intercept link clicks inside a frame and open them in the external
  // browser. Only reachable if the sandbox ever regains `allow-same-origin`;
  // with an opaque origin `contentDocument` throws and we skip it.
  const attachLinkHandler = useCallback((iframe) => {
    if (!iframe) return;
    const handleLoad = () => {
      try {
        const doc = iframe.contentDocument;
        if (!doc) return;
        const handleClick = (e) => {
          const anchor = e.target.closest('a');
          const href = anchor?.getAttribute('href');
          if (!href || href.startsWith('#')) return;
          e.preventDefault();
          e.stopPropagation();
          openUrl(href).catch((err) => toast.error(`Failed to open link: ${err}`));
        };
        doc.addEventListener('click', handleClick);
        iframe._rusticClickCleanup = () => doc.removeEventListener('click', handleClick);
      } catch {
        // Opaque origin — nothing to attach to.
      }
    };
    iframe.addEventListener('load', handleLoad);
  }, []);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (text == null || live.status === 'pending') {
    return (
      <div className="flex h-full w-full flex-col gap-2 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }

  const registerFrame = (frame) => (el) => {
    if (el) frameEls.current.set(frame.key, { el, id: frame.id, key: frame.key });
    else frameEls.current.delete(frame.key);
  };

  const toolbar = (
    <>
      <div className="flex min-w-0 items-center gap-1">
        <Button
          size="icon-xs"
          variant="ghost"
          onClick={reload}
          aria-label="Reload preview"
          title="Reload preview"
        >
          <RefreshCw />
        </Button>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => setInteractive((v) => !v)}
          title={
            interactive
              ? 'Interact: the page gets clicks and typing. Wheel scrolls, Cmd/Ctrl+wheel zooms, Space+drag pans. Click to switch to drag-to-pan.'
              : 'Pan: drag anywhere to move the canvas. Click to interact with the page again.'
          }
        >
          {interactive ? <MousePointerClick /> : <Hand />}
          <span className="ml-1">{interactive ? 'Interact' : 'Pan'}</span>
        </Button>
        <CanvasControls
          canvas={canvas}
          activeIds={activeIds}
          onToggle={toggleFrame}
          custom={customSize}
          onCustomChange={setCustomSize}
        />
        <span className="ml-1 hidden truncate text-xs text-muted-foreground xl:inline">
          {basename(tab.path)}
        </span>
      </div>
    </>
  );

  let notice = null;
  if (session && !allowed) {
    notice = (
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-1 text-xs text-muted-foreground">
        <ShieldAlert className="size-3.5 shrink-0 text-primary" />
        <span className="min-w-0 truncate">Scripts are off. The page is shown without JavaScript at device height.</span>
        <Popover>
          <PopoverTrigger asChild>
            <Button size="xs" variant="outline" className="ml-auto shrink-0">Run scripts…</Button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-96 space-y-2 text-xs">
            <div className="flex items-center gap-2 text-sm font-medium text-foreground">
              <ShieldAlert className="size-4 text-primary" /> Run this project&apos;s page scripts?
            </div>
            <p className="text-muted-foreground">
              The preview will run the page&apos;s own JavaScript in a sandbox and show the full page. It cannot
              reach Rustic, hidden files such as .env and .git, or anything outside this project:
            </p>
            <p className="break-all font-mono text-[11px] text-foreground">{session.root}</p>
            <p className="text-muted-foreground">
              It can read the other files in the project, and it can load from and send data to the internet
              (CDNs, fonts, APIs). A page could send those files out. Allow this only for projects whose code
              you trust.
            </p>
            <div className="flex items-center gap-2 pt-1">
              <Button size="sm" onClick={() => { allowPageScripts(session.project); setAllowed(true); }}>
                Allow for this project
              </Button>
              <span className="text-[11px] text-muted-foreground">Asked once per project.</span>
            </div>
          </PopoverContent>
        </Popover>
      </div>
    );
  } else if (live.status === 'off') {
    notice = (
      <div className="shrink-0 truncate border-b border-border px-3 py-1 text-xs text-muted-foreground" title={live.reason}>
        Scripts are off: {live.reason}
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-border bg-muted/20 px-2">
        {toolbar}
      </div>
      {notice}
      <PreviewCanvas
        canvas={canvas}
        interactive={interactive}
        renderFrame={(frame) => (running ? (
          <iframe
            key={frame.key}
            ref={registerFrame(frame)}
            // The page's own scripts run here. No `allow-same-origin`: the frame has an
            // opaque origin, so it cannot read Rustic's storage or cookies, and the
            // session's CSP keeps it to the project and `https:`.
            sandbox="allow-scripts"
            referrerPolicy="no-referrer"
            src={session.url}
            title={`HTML preview — ${frame.label}`}
            className="block border-0 bg-white"
            style={{ width: frame.width, height: frame.height }}
          />
        ) : (
          <iframe
            ref={attachLinkHandler}
            // `sandbox` without `allow-scripts` means the preview is read-only —
            // a malicious file in the project can't run arbitrary JS in the
            // host context. `allow-same-origin` is deliberately ABSENT: styles,
            // fonts and CSS variables render fine in an opaque-origin srcdoc
            // document, and keeping the origin opaque means the frame can never
            // touch the host's cookies/storage even if scripts were ever enabled.
            sandbox="allow-popups"
            srcDoc={inlinedHtml}
            title={`HTML preview — ${frame.label}`}
            className="block border-0 bg-white"
            style={{ width: frame.width, height: frame.height }}
          />
        ))}
      />
    </div>
  );
}
