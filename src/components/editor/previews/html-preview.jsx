import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Eye, Pencil, RefreshCw } from 'lucide-react';
import { SourceCodeEditor } from './source-code-editor';
import { useEditor } from '@/state/editor';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';

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

export default function HtmlPreview({ tab }) {
  const [text, setText] = useState(null);
  const [draft, setDraft] = useState(null);
  const [error, setError] = useState(null);
  const [mode, setMode] = useState('preview');
  const [saving, setSaving] = useState(false);
  const tabSetDirty = useEditor((s) => s.setDirty);
  const iframeRef = useRef(null);
  // `inlinedHtml` is the iframe-ready HTML (relative CSS / images resolved
  // to inline content). Async because resolution itself is async, so we
  // hold it in state rather than recomputing inside an effect on every
  // keystroke. `inliningId` increments to discard stale results when the
  // user types fast.
  const [inlinedHtml, setInlinedHtml] = useState('');
  const inliningIdRef = useRef(0);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setText(null);
    setDraft(null);
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (cancelled) return;
        const body = c ?? '';
        setText(body);
        setDraft(body);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  const dirty = draft !== null && draft !== text;

  // Re-inline whenever the preview tab is active and the draft changes.
  // We don't run this in edit mode — the iframe is hidden and inlining is
  // expensive (each <link> is an IPC roundtrip), so deferring keeps the
  // editor responsive.
  useEffect(() => {
    if (mode !== 'preview' || draft == null) return;
    const id = ++inliningIdRef.current;
    inlineLocalResources(draft, tab.path)
      .then((html) => {
        if (id === inliningIdRef.current) setInlinedHtml(html);
      })
      .catch(() => {
        // Fall back to the raw draft so the user at least sees something.
        if (id === inliningIdRef.current) setInlinedHtml(draft);
      });
  }, [mode, draft, tab.path]);

  const onSave = async () => {
    if (!dirty || saving) return;
    setSaving(true);
    try {
      await writeTextFile(tab.path, draft ?? '');
      setText(draft);
      toast.success('Saved');
    } catch (e) {
      const msg = typeof e === 'string' ? e : e?.message || String(e);
      toast.error(`Save failed: ${msg}`);
    } finally {
      setSaving(false);
    }
  };

  // Mirror dirty into the tab's yellow dot + bind Ctrl+S to onSave.
  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty, tab.id]);
  useEffect(() => {
    setActiveSaver(onSave);
    return () => clearActiveSaver(onSave);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.path, dirty, draft]);

  // Intercept link clicks in the iframe and open them in the external
  // browser. Since the iframe has `allow-same-origin` sandbox flag, we can
  // access its contentDocument and attach a click handler.
  useEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe || mode !== 'preview') return;

    const handleLoad = () => {
      try {
        const doc = iframe.contentDocument;
        if (!doc) return;

        const handleClick = (e) => {
          const anchor = e.target.closest('a');
          if (!anchor) return;
          const href = anchor.getAttribute('href');
          if (!href) return;
          
          // Allow internal anchor links (same-page navigation within iframe)
          if (href.startsWith('#')) return;
          
          e.preventDefault();
          e.stopPropagation();
          
          // Open external URLs in the default browser
          openUrl(href).catch((err) => {
            toast.error(`Failed to open link: ${err}`);
          });
        };

        doc.addEventListener('click', handleClick);
        
        // Store cleanup function on the iframe element so we can call it
        // when the iframe reloads or the component unmounts
        iframe._rusticClickCleanup = () => {
          doc.removeEventListener('click', handleClick);
        };
      } catch (err) {
        // Cross-origin or sandbox violation — can't access contentDocument
        console.warn('Cannot access iframe document:', err);
      }
    };

    // Attach load listener for when the iframe loads/reloads
    iframe.addEventListener('load', handleLoad);
    
    // If already loaded, handle immediately
    if (iframe.contentDocument?.readyState === 'complete') {
      handleLoad();
    }

    return () => {
      iframe.removeEventListener('load', handleLoad);
      if (iframe._rusticClickCleanup) {
        iframe._rusticClickCleanup();
        iframe._rusticClickCleanup = null;
      }
    };
  }, [mode, inlinedHtml]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (text == null) {
    return (
      <div className="flex h-full w-full flex-col gap-2 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      {/* Preview/Edit toggle + a manual reload (for when an external CSS
          file linked from the document was edited on disk). Unsaved
          state shows as a yellow dot on the file tab; Ctrl+S saves. */}
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-muted/20 px-2">
        <Button
          size="xs"
          variant={mode === 'preview' ? 'secondary' : 'ghost'}
          onClick={() => setMode('preview')}
        >
          <Eye className="mr-1 size-3" /> Preview
        </Button>
        <Button
          size="xs"
          variant={mode === 'edit' ? 'secondary' : 'ghost'}
          onClick={() => setMode('edit')}
        >
          <Pencil className="mr-1 size-3" /> Edit
        </Button>
        {mode === 'preview' && (
          <Button
            size="icon-xs"
            variant="ghost"
            onClick={() => {
              inliningIdRef.current += 1;
              inlineLocalResources(draft ?? '', tab.path)
                .then(setInlinedHtml)
                .catch(() => setInlinedHtml(draft ?? ''));
            }}
            aria-label="Reload preview"
          >
            <RefreshCw />
          </Button>
        )}
      </div>
      <div className="relative min-h-0 flex-1">
        {mode === 'preview' ? (
          <iframe
            ref={iframeRef}
            // `sandbox` without `allow-scripts` means the preview is read-only —
            // a malicious file in the project can't run arbitrary JS in the
            // host context. We add `allow-same-origin` so inlined <style> can
            // still reference fonts and CSS variables; the iframe itself has
            // no real origin since it's a srcdoc document.
            sandbox="allow-same-origin allow-popups"
            srcDoc={inlinedHtml}
            title="HTML preview"
            className="h-full w-full border-0 bg-white"
          />
        ) : (
          <SourceCodeEditor
            value={draft ?? ''}
            onChange={setDraft}
            onSave={onSave}
            lang="html"
          />
        )}
      </div>
    </div>
  );
}
