import React, { useState, useEffect, useCallback } from 'react';
import { Plus, X, Trash2, Check, FolderOpen, Pencil, Copy, Zap } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import { open as openFilePicker } from '@tauri-apps/plugin-dialog';
import { readFile } from '@tauri-apps/plugin-fs';
import { useSettings } from '@/state/settings';
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
                        ? 'text-amber-500 border-amber-500/40 bg-amber-500/10'
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

// ─── Theme Card & Color Palette (unchanged) ────────────────────────────────────

// Only show swatches for fields the theme bridge actually paints into the
// chrome. Other fields (bright_green/yellow/blue/purple/aqua/orange, token_*,
// extra bg/fg shades) are stored on the Theme struct but nothing in the
// current UI reads them, so showing them was misleading — users were seeing
// lavender / aqua swatches that never appear anywhere in the app.
const SWATCH_KEYS = ['bg', 'bg1', 'bg2', 'fg', 'fg2', 'accent', 'border', 'bright_red'];

function ThemeCard({ info, fullTheme, isActive, onActivate, onDelete, onEdit }) {
  const swatches = fullTheme ? SWATCH_KEYS.map((k) => fullTheme[k]).filter(Boolean) : [];

  return (
    <div className={cn(
      'rounded-xl border p-3 transition-colors',
      isActive ? 'border-primary/50 bg-primary/5' : 'border-border/50 bg-muted/20 hover:border-border',
    )}>
      <div className="flex items-center gap-2 mb-2.5">
        <span className="flex-1 text-[13px] font-medium">{info.name}</span>
        <Badge variant="outline" className="h-5 px-1.5 text-[10px] text-muted-foreground border-border/60">
          {info.is_builtin ? 'Built-in' : 'Custom'}
        </Badge>
        <Button variant="ghost" size="icon-sm" onClick={() => onEdit?.(info)}
          className="size-6 cursor-pointer text-muted-foreground hover:text-foreground"
          title="Edit / copy JSON">
          <Pencil className="size-3" />
        </Button>
        {!info.is_builtin && onDelete && (
          <Button variant="ghost" size="icon-sm" onClick={() => onDelete(info.name)}
            className="size-6 cursor-pointer text-muted-foreground hover:text-destructive">
            <Trash2 className="size-3" />
          </Button>
        )}
      </div>
      <div className="flex flex-wrap gap-1 mb-3">
        {swatches.length > 0
          ? swatches.map((color, i) => (
              <span key={i} className="inline-block size-5 rounded-sm border border-black/10"
                style={{ backgroundColor: color }} title={color} />
            ))
          : <span className="text-[11px] text-muted-foreground">Loading colors…</span>
        }
      </div>
      <div className="flex items-center gap-2">
        <Button size="sm" variant={isActive ? 'default' : 'secondary'}
          className="h-6 px-3 text-[11px] cursor-pointer"
          onClick={() => !isActive && onActivate(info.name)} disabled={isActive}>
          {isActive ? <><Check className="size-3 mr-1" />Active</> : 'Activate'}
        </Button>
      </div>
    </div>
  );
}

// ─── Edit Theme Dialog ────────────────────────────────────────────────────────

function EditThemeDialog({ open, theme, isBuiltin, onClose, onSaved }) {
  const importThemeJson = useSettings((s) => s.importThemeJson);
  const [json, setJson]       = useState('');
  const [error, setError]     = useState('');
  const [saving, setSaving]   = useState(false);
  const [copied, setCopied]   = useState(false);

  useEffect(() => {
    if (!open || !theme) return;
    // Built-in names are reserved (getTheme prefers builtin), so editing a
    // built-in must save under a different name or the new version is
    // invisible. Pre-suffix " Copy" — user can rename further if they want.
    const seed = isBuiltin ? { ...theme, name: `${theme.name} Copy` } : theme;
    setJson(JSON.stringify(seed, null, 2));
    setError('');
    setCopied(false);
  }, [open, theme?.name, isBuiltin]);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(json);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      setError(`Copy failed: ${e}`);
    }
  }

  async function handleSave() {
    setSaving(true);
    setError('');
    try {
      await importThemeJson(json);
      onSaved?.();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[560px] sm:max-w-[560px] gap-0 p-0 overflow-hidden">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">
            {isBuiltin ? `Edit "${theme?.name}" (copy)` : `Edit "${theme?.name}"`}
          </DialogTitle>
          <p className="text-[12px] text-muted-foreground mt-1">
            {isBuiltin
              ? 'Built-in themes can’t be modified in place — saving creates a new custom theme. Rename freely.'
              : 'Edit the JSON below and Save to update this theme.'}
          </p>
        </DialogHeader>
        <div className="px-5 py-4 space-y-2">
          <textarea
            value={json}
            onChange={(e) => setJson(e.target.value)}
            className={cn(
              'w-full h-80 resize-none rounded-lg border border-border/50 bg-muted/30',
              'px-3 py-2.5 text-[12px] font-mono text-foreground',
              'focus:outline-none focus:ring-1 focus:ring-ring',
            )}
            spellCheck={false}
          />
          {error && <p className="text-[12px] text-destructive">{error}</p>}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60 flex-row justify-between sm:justify-between gap-2">
          <Button variant="ghost" size="sm" className="gap-1.5 text-xs cursor-pointer" onClick={handleCopy}>
            {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
            {copied ? 'Copied' : 'Copy JSON'}
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" className="text-xs cursor-pointer" onClick={onClose}>Cancel</Button>
            <Button size="sm" className="text-xs cursor-pointer" onClick={handleSave} disabled={saving || !json.trim()}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function AddThemeModal({ open, onClose, onImported }) {
  const importTheme     = useSettings((s) => s.importTheme);
  const importThemeJson = useSettings((s) => s.importThemeJson);

  const [json, setJson]           = useState('');
  const [importing, setImporting] = useState(false);
  const [error, setError]         = useState('');

  async function handleImportJson() {
    if (!json.trim()) return;
    setImporting(true);
    setError('');
    try {
      await importThemeJson(json.trim());
      setJson('');
      onClose();
      onImported?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setImporting(false);
    }
  }

  async function handleBrowseFile() {
    setError('');
    try {
      const path = await openFilePicker({
        title: 'Select a theme file',
        filters: [{ name: 'Theme files', extensions: ['json', 'toml'] }],
      });
      if (!path) return;
      setImporting(true);
      await importTheme(path);
      onClose();
      onImported?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setImporting(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[520px] sm:max-w-[520px] gap-0 p-0 overflow-hidden">
        <DialogHeader className="px-5 pt-5 pb-4 border-b border-border/60">
          <DialogTitle className="text-[14px]">Add Color Palette</DialogTitle>
          <p className="text-[12px] text-muted-foreground mt-1">
            Paste a theme JSON below, or browse for a <code className="text-[11px]">.json</code> / <code className="text-[11px]">.toml</code> file.
          </p>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          <textarea
            value={json}
            onChange={(e) => setJson(e.target.value)}
            placeholder={'{\n  "name": "My Theme",\n  "kind": "dark",\n  "bg": "#1a1a2e",\n  ...\n}'}
            className={cn(
              'w-full h-48 resize-none rounded-lg border border-border/50 bg-muted/30',
              'px-3 py-2.5 text-[12px] font-mono text-foreground placeholder:text-muted-foreground/50',
              'focus:outline-none focus:ring-1 focus:ring-ring',
            )}
            spellCheck={false}
          />
          {error && <p className="text-[12px] text-destructive">{error}</p>}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60 flex-row justify-between sm:justify-between gap-2">
          <Button variant="ghost" size="sm" className="gap-1.5 text-xs cursor-pointer"
            onClick={handleBrowseFile} disabled={importing}>
            <FolderOpen className="size-3.5" />
            Browse file
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" className="text-xs cursor-pointer" onClick={onClose}>Cancel</Button>
            <Button size="sm" className="text-xs cursor-pointer"
              onClick={handleImportJson} disabled={importing || !json.trim()}>
              {importing ? 'Importing…' : 'Import'}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ColorPaletteSection() {
  const themes         = useSettings((s) => s.themes);
  const activeTheme    = useSettings((s) => s.activeTheme);
  const setActiveTheme = useSettings((s) => s.setActiveTheme);
  const deleteTheme    = useSettings((s) => s.deleteTheme);
  const getTheme       = useSettings((s) => s.getTheme);

  const [fullThemes, setFullThemes] = useState({});
  const [addOpen, setAddOpen]       = useState(false);
  const [editTarget, setEditTarget] = useState(null); // { info, theme } or null

  async function fetchFullThemes(list) {
    const results = {};
    await Promise.all(list.map(async (info) => {
      try { results[info.name] = await getTheme(info.name); } catch { /* skip */ }
    }));
    setFullThemes(results);
  }

  useEffect(() => { if (themes.length > 0) fetchFullThemes(themes); }, [themes]);

  function handleEdit(info) {
    const t = fullThemes[info.name];
    if (!t) return;
    setEditTarget({ info, theme: t });
  }

  return (
    <section data-settings-anchor="color-palette" className="mb-6">
      <div className="flex items-center justify-between mb-2 px-1">
        <h3 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70">
          Color Palette
        </h3>
        <Button size="sm" variant="ghost" className="h-6 px-2 text-[11px] gap-1 cursor-pointer"
          onClick={() => setAddOpen(true)}>
          <Plus className="size-3" />Import
        </Button>
      </div>
      <div className="grid grid-cols-2 gap-3">
        {themes.map((info) => (
          <ThemeCard key={info.name} info={info} fullTheme={fullThemes[info.name]}
            isActive={activeTheme?.name === info.name}
            onActivate={setActiveTheme}
            onDelete={deleteTheme}
            onEdit={handleEdit}
          />
        ))}
      </div>
      <AddThemeModal open={addOpen} onClose={() => setAddOpen(false)}
        onImported={() => fetchFullThemes(themes)} />
      <EditThemeDialog
        open={!!editTarget}
        theme={editTarget?.theme}
        isBuiltin={!!editTarget?.info?.is_builtin}
        onClose={() => setEditTarget(null)}
        onSaved={() => fetchFullThemes(themes)}
      />
    </section>
  );
}

// ─── Root export ───────────────────────────────────────────────────────────────

export function AppearanceSettings() {
  return (
    <>
      <FontsSection />
      <ColorPaletteSection />
    </>
  );
}
