import React, { useState, useEffect } from 'react';
import { X, Check, FolderOpen, Zap } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { open as openFilePicker } from '@tauri-apps/plugin-dialog';
import { readFile } from '@tauri-apps/plugin-fs';
import { useSettings } from '@/state/settings';
import { ThemesSection } from './themes-section';
import { cn } from '@/lib/utils';

// ─── Font target definitions ──────────────────────────────────────────────────

// Terminal is intentionally excluded — xterm's fixed-grid renderer requires a
// monospace font, and there's no meaningful UX for letting users pick a custom
// terminal font in this dialog. The terminal always uses its built-in
// monospace stack (Consolas, JetBrains Mono, etc.).
const FONT_TARGETS = [
  { id: 'editor',        label: 'Editor',         cssVar: '--font-editor',       monospace: true },
  { id: 'folderNames',   label: 'Folder Names',    cssVar: '--font-folder-names'  },
  { id: 'fileNames',     label: 'File Names',      cssVar: '--font-file-names'    },
  { id: 'agentChat',     label: 'Agent Chat',      cssVar: '--font-agent-chat'    },
  { id: 'tabLabels',     label: 'Tab Labels',      cssVar: '--font-tabs'          },
  { id: 'searchResults', label: 'Search Results',  cssVar: '--font-search'        },
];

// Detect whether a loaded font has equal-width glyphs. We compare the canvas
// width of a string of narrow `i`s against the same length string of wide
// `W`s. If they're (close to) equal the font is monospace. Returns true when
// the font isn't actually loaded — in that case the browser falls back to
// the UA monospace, which would falsely look monospace. So callers should
// only trust the result when document.fonts.check confirms the font is loaded.
function isMonospaceFont(name) {
  try {
    if (!document.fonts?.check(`16px "${name}"`)) return null;
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    ctx.font = `16px "${name}"`;
    const narrow = ctx.measureText('iiiiiiiiii').width;
    const wide   = ctx.measureText('WWWWWWWWWW').width;
    return Math.abs(narrow - wide) < 1;
  } catch { return null; }
}

const FONT_APP_KEY = 'rustic_font_applications';

function getFontApplications() {
  try { return JSON.parse(localStorage.getItem(FONT_APP_KEY) || '{}'); }
  catch { return {}; }
}

function saveFontApplications(apps) {
  localStorage.setItem(FONT_APP_KEY, JSON.stringify(apps));
}

// Apply / remove a font for a given target. FontBridge / terminal / Monaco
// pick this up via the dispatched event, so callers MUST persist localStorage
// before calling this — otherwise listeners re-read stale data.
function applyFontToDOM(targetId, fontName) {
  const target = FONT_TARGETS.find((t) => t.id === targetId);
  if (!target) return;
  if (fontName) {
    document.documentElement.style.setProperty(target.cssVar, `"${fontName}", monospace`);
  } else {
    document.documentElement.style.removeProperty(target.cssVar);
  }
  window.dispatchEvent(new CustomEvent('rustic:font-applied', { detail: { targetId, fontName } }));
}

// Re-apply all saved font applications on mount
function rehydrateFontApplications() {
  const apps = getFontApplications();
  for (const [targetId, fontName] of Object.entries(apps)) {
    if (fontName) applyFontToDOM(targetId, fontName);
  }
}

// ─── Font Application Dialog ──────────────────────────────────────────────────

function FontApplicationDialog({ font, open, onClose }) {
  const updateSettings = useSettings((s) => s.update);
  const settings       = useSettings((s) => s.settings);

  const [checked, setChecked] = useState({});
  // null = unknown (font not loaded yet), true = monospace, false = proportional
  const [monoOk, setMonoOk] = useState(null);

  // Pre-fill checkboxes based on what this font is already applied to
  useEffect(() => {
    if (!open) return;
    const apps = getFontApplications();
    const initial = {};
    for (const t of FONT_TARGETS) {
      initial[t.id] = apps[t.id] === font?.name;
    }
    setChecked(initial);
  }, [open, font?.name]);

  // Probe whether the font is monospace once it's actually loaded. The check
  // requires loaded glyph data so we trigger document.fonts.load first.
  useEffect(() => {
    if (!open || !font?.name) return;
    let cancelled = false;
    (async () => {
      try { await document.fonts.load(`16px "${font.name}"`); } catch {}
      if (cancelled) return;
      setMonoOk(isMonospaceFont(font.name));
    })();
    return () => { cancelled = true; };
  }, [open, font?.name]);

  function toggle(id) {
    setChecked((prev) => ({ ...prev, [id]: !prev[id] }));
  }

  async function handleApply() {
    const apps = getFontApplications();
    // Collect the changes first so we can persist localStorage BEFORE dispatching
    // any events. FontBridge re-reads localStorage when it hears
    // `rustic:font-applied`, so dispatching before saving causes it to rebuild
    // with stale data and the new font silently doesn't apply.
    const changes = [];
    let editorChange = null; // 'set' | 'clear' | null
    for (const t of FONT_TARGETS) {
      if (checked[t.id]) {
        apps[t.id] = font.name;
        changes.push({ targetId: t.id, fontName: font.name });
        if (t.id === 'editor') editorChange = 'set';
      } else if (apps[t.id] === font.name) {
        delete apps[t.id];
        changes.push({ targetId: t.id, fontName: null });
        if (t.id === 'editor') editorChange = 'clear';
      }
    }
    saveFontApplications(apps);
    for (const c of changes) applyFontToDOM(c.targetId, c.fontName);

    // Sync editor font to backend settings. We have to handle both setting AND
    // clearing — without the clear branch, unchecking Editor would remove the
    // CSS-var mapping but leave settings.editor.font_family pointing at the old
    // font, so Monaco would keep rendering in it.
    if (editorChange && settings) {
      const nextFont = editorChange === 'set' ? font.name : '';
      await updateSettings({ editor: { ...settings.editor, font_family: nextFont } });
    }

    onClose();
  }

  if (!font) return null;

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[340px] sm:max-w-[340px] gap-0 p-0 overflow-hidden flex flex-col max-h-[90vh]">
        <DialogHeader className="px-5 pt-5 pb-3 shrink-0">
          <DialogTitle className="text-[14px]">Apply "{font.name}"</DialogTitle>
          <p className="text-[12px] text-muted-foreground mt-0.5">
            Select where to apply this font:
          </p>
        </DialogHeader>

        <div className="px-5 py-1 space-y-0.5 overflow-y-auto flex-1">
          {FONT_TARGETS.map((t) => {
            const monoMismatch = t.monospace && monoOk === false;
            return (
              <label
                key={t.id}
                onClick={() => toggle(t.id)}
                className="flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 hover:bg-muted/50 transition-colors"
              >
                <div className={cn(
                  'flex size-4 shrink-0 items-center justify-center rounded-sm border transition-colors',
                  checked[t.id]
                    ? 'border-primary bg-primary text-primary-foreground'
                    : 'border-border bg-transparent'
                )}>
                  {checked[t.id] && <Check className="size-3" strokeWidth={3} />}
                </div>
                <div className="flex flex-1 items-center gap-2 min-w-0">
                  <span className="text-[13px]">{t.label}</span>
                  {t.monospace && (
                    <span className={cn(
                      'text-[10px] px-1.5 py-0.5 rounded border',
                      monoMismatch
                        ? 'text-warning border-warning/40 bg-warning/10'
                        : 'text-muted-foreground/70 border-border/60'
                    )}>
                      monospace
                    </span>
                  )}
                </div>
              </label>
            );
          })}
        </div>

        <div className="px-5 py-3 border-t border-border/60 flex gap-2 shrink-0">
          <Button size="sm" className="cursor-pointer px-6 text-xs" onClick={handleApply}>
            Apply
          </Button>
          <Button size="sm" variant="secondary" className="cursor-pointer text-xs" onClick={onClose}>
            Cancel
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ─── Font Row ─────────────────────────────────────────────────────────────────

function FontRow({ font, onRemove }) {
  const [applyOpen, setApplyOpen] = useState(false);
  const [appliedTargets, setAppliedTargets] = useState([]);

  function refreshApplied() {
    const apps = getFontApplications();
    setAppliedTargets(
      FONT_TARGETS.filter((t) => apps[t.id] === font.name).map((t) => t.label)
    );
  }

  useEffect(() => { refreshApplied(); }, [font.name]);

  function handleClose() {
    setApplyOpen(false);
    refreshApplied();
  }

  return (
    <>
      <div className="flex items-center justify-between px-3 py-2.5 gap-3">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-[13px] font-medium truncate" style={{ fontFamily: font.name }}>
            {font.name}
          </span>
          <span className="text-[11px] text-muted-foreground shrink-0">
            {font.type === 'file' ? 'Local file' : 'URL'}
          </span>
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          {/* Applied target badges */}
          {appliedTargets.length > 0 && (
            <div className="flex gap-1">
              {appliedTargets.slice(0, 3).map((label) => (
                <Badge
                  key={label}
                  variant="outline"
                  className="h-4 px-1.5 text-[10px] text-primary border-primary/40 bg-primary/10"
                >
                  {label}
                </Badge>
              ))}
              {appliedTargets.length > 3 && (
                <Badge variant="outline" className="h-4 px-1.5 text-[10px] text-muted-foreground">
                  +{appliedTargets.length - 3}
                </Badge>
              )}
            </div>
          )}

          <Button
            variant="ghost"
            size="sm"
            onClick={() => setApplyOpen(true)}
            className="h-6 px-2 text-[11px] gap-1 cursor-pointer text-muted-foreground hover:text-foreground"
          >
            <Zap className="size-3" />
            Set active
          </Button>

          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => onRemove(font.name)}
            className="size-6 cursor-pointer text-muted-foreground hover:text-destructive"
          >
            <X className="size-3.5" />
          </Button>
        </div>
      </div>

      <FontApplicationDialog
        font={font}
        open={applyOpen}
        onClose={handleClose}
      />
    </>
  );
}

// ─── Fonts Section ────────────────────────────────────────────────────────────

function FontsSection() {
  const loadedFonts    = useSettings((s) => s.loadedFonts);
  const addFontFromUrl  = useSettings((s) => s.addFontFromUrl);
  const addFontFromFile = useSettings((s) => s.addFontFromFile);
  const removeFont     = useSettings((s) => s.removeFont);

  const [urlInput, setUrlInput] = useState('');
  const [loading, setLoading]   = useState(false);
  const [error, setError]       = useState('');

  useEffect(() => { rehydrateFontApplications(); }, []);

  async function handleLoad() {
    const url = urlInput.trim();
    if (!url) return;
    setLoading(true);
    setError('');
    try {
      await addFontFromUrl(url);
      setUrlInput('');
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  async function handleBrowse() {
    setError('');
    try {
      const path = await openFilePicker({
        title: 'Select a font file',
        filters: [{ name: 'Font files', extensions: ['ttf', 'otf', 'woff', 'woff2'] }],
      });
      if (!path) return;
      setLoading(true);
      const bytes = await readFile(path);
      await addFontFromFile(path, bytes);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  function handleRemove(name) {
    // Same save-before-dispatch ordering as handleApply.
    const apps = getFontApplications();
    const cleared = [];
    for (const [targetId, fontName] of Object.entries(apps)) {
      if (fontName === name) {
        delete apps[targetId];
        cleared.push(targetId);
      }
    }
    saveFontApplications(apps);
    for (const targetId of cleared) applyFontToDOM(targetId, null);
    removeFont(name);
  }

  return (
    <section data-settings-anchor="fonts" className="mb-6">
      <h3 className="mb-2 px-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70">
        Fonts
      </h3>
      <div className="rounded-xl border border-border/50 bg-muted/20 overflow-hidden">
        <div className="flex items-center gap-2 px-3 py-3 border-b border-border/40">
          <Input
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleLoad()}
            placeholder="Paste a Google Fonts URL or direct font URL"
            className="h-7 flex-1 text-xs"
          />
          <Button
            size="sm"
            variant="secondary"
            className="h-7 px-3 text-xs cursor-pointer"
            onClick={handleLoad}
            disabled={loading || !urlInput.trim()}
          >
            {loading ? 'Loading…' : 'Load'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs gap-1.5 cursor-pointer"
            onClick={handleBrowse}
            disabled={loading}
          >
            <FolderOpen className="size-3.5" />
            Browse
          </Button>
        </div>

        {error && (
          <div className="px-3 py-2 text-[12px] text-destructive border-b border-border/40">{error}</div>
        )}

        {loadedFonts.length === 0 ? (
          <div className="px-3 py-5 text-[13px] text-muted-foreground text-center">
            No fonts loaded yet. Paste a URL or browse for a file above.
          </div>
        ) : (
          <div className="divide-y divide-border/40">
            {loadedFonts.map((f) => (
              <FontRow key={f.name} font={f} onRemove={handleRemove} />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

// ─── Root export ───────────────────────────────────────────────────────────────

export function AppearanceSettings() {
  return (
    <>
      <FontsSection />
      <ThemesSection />
    </>
  );
}
