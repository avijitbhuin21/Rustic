// Settings › Appearance › Themes: theme packs (format 2). Built-ins apply
// directly; imported / edited themes always go through the trust prompt,
// which is protected UI (theme CSS can't reach it).
import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  Check, Upload, ClipboardPaste, FolderOpen, MoreHorizontal, Download, Trash2, ShieldCheck, ShieldAlert,
  FileCode2, FileDown, Copy, Moon, Sun, MonitorSmartphone, Pencil, AlertTriangle, Ban, Globe,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { InfoTip } from '@/components/ui/info-tip';
import { cn } from '@/lib/utils';
import { IS_WEB } from '@/lib/platform';
import { useThemes } from '@/state/themes';
import { Slider } from './cloud-machines';

const errText = (e) => String(e?.message || e);

/** Small swatch strip for one mode: [bg, card, text, accent]. */
function Swatches({ colors, className }) {
  if (!colors) return null;
  return (
    <div className={cn('flex h-8 overflow-hidden rounded-md border border-ink/10', className)}>
      {colors.map((c, i) => <div key={i} className="flex-1" style={{ background: c }} />)}
    </div>
  );
}

/**
 * Review a theme before it may apply. Shows exactly what the scanner found;
 * blocked themes can't be trusted. Trusting pins the reviewed file hash.
 */
export function ThemeTrustDialog({ detail, onClose, onTrusted }) {
  const trustAndApply = useThemes((s) => s.trustAndApply);
  const [busy, setBusy] = useState(false);
  const [showFile, setShowFile] = useState(false);
  useEffect(() => { setShowFile(false); setBusy(false); }, [detail?.hash]);
  if (!detail) return null;
  const { entry, pack, scan, hash, text } = detail;
  const blocked = scan?.blocked || [];
  const warnings = scan?.warnings || [];
  const trust = async () => {
    setBusy(true);
    try {
      await trustAndApply(entry.id, hash);
      toast.success(`Applied ${entry.name}`);
      onTrusted?.();
      onClose();
    } catch (e) {
      toast.error(errText(e));
      setBusy(false);
    }
  };
  return (
    <Dialog open={!!detail} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-lg" data-rustic-protected="">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-1.5">
            {blocked.length ? <Ban className="size-4 text-danger" /> : <ShieldCheck className="size-4" />}
            Trust this theme?
          </DialogTitle>
          <DialogDescription>
            Themes can change how every part of Rustic looks. Only trust themes from people you trust — this one applies only after you confirm.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3 text-[12px]">
          <div className="rounded-lg border border-border p-3">
            <div className="text-[13px] font-semibold">{pack.name}</div>
            <div className="text-[11px] text-muted-foreground">
              {[pack.author && `by ${pack.author}`, pack.version && `v${pack.version}`, entry.modes.join(' + ')].filter(Boolean).join(' · ')}
            </div>
            {pack.description && <p className="mt-1 text-[11.5px] text-muted-foreground">{pack.description}</p>}
            <div className="mt-2 grid grid-cols-2 gap-2">
              {Object.entries(entry.swatches || {}).map(([mode, colors]) => (
                <div key={mode} className="space-y-1">
                  <div className="text-[10px] uppercase tracking-wider text-muted-foreground">{mode}</div>
                  <Swatches colors={colors} />
                </div>
              ))}
            </div>
            {entry.has_css && <div className="mt-2 flex items-center gap-1.5 text-[11px] text-muted-foreground"><FileCode2 className="size-3.5" /> Includes custom CSS ({Math.ceil((scan?.css_bytes || 0) / 1024)} KB)</div>}
          </div>

          {blocked.length > 0 && (
            <div className="space-y-1 rounded-lg border border-danger/40 bg-danger/10 p-3">
              <div className="flex items-center gap-1.5 font-semibold text-danger"><Ban className="size-3.5" /> Blocked — this theme can't be trusted</div>
              {blocked.map((f, i) => <div key={i} className="text-[11.5px]">• {f.message}</div>)}
            </div>
          )}
          {warnings.length > 0 && (
            <div className="space-y-1 rounded-lg border border-warning/40 bg-warning/10 p-3">
              <div className="flex items-center gap-1.5 font-semibold text-warning"><AlertTriangle className="size-3.5" /> Review before trusting</div>
              {warnings.map((f, i) => <div key={i} className="text-[11.5px]">• {f.message}</div>)}
            </div>
          )}
          {(scan?.remote_hosts || []).length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
              <Globe className="size-3.5" /> Connects to:
              {scan.remote_hosts.map((h) => <code key={h} className="rounded bg-muted px-1.5 py-0.5 font-mono">{h}</code>)}
            </div>
          )}
          {!blocked.length && !warnings.length && (
            <div className="flex items-center gap-1.5 text-[11.5px] text-success"><ShieldCheck className="size-3.5" /> No risky patterns found.</div>
          )}

          <button type="button" className="text-[11px] text-muted-foreground underline-offset-2 hover:underline" onClick={() => setShowFile((v) => !v)}>
            {showFile ? 'Hide file' : 'View file'}
          </button>
          {showFile && (
            <pre className="max-h-56 overflow-auto rounded-md border border-border bg-muted/40 p-2 font-mono text-[10.5px] leading-relaxed">{text}</pre>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="h-7 text-xs" disabled={busy || blocked.length > 0} onClick={trust}>
            <ShieldCheck className="size-3.5" /> Trust & apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * App-wide: when the agent saves a theme, offer to review it. Agent-written
 * themes never apply on their own — they go through the same trust prompt.
 */
export function ThemeAgentPrompt() {
  const refresh = useThemes((s) => s.refresh);
  const detail = useThemes((s) => s.detail);
  const [review, setReview] = useState(null);
  useEffect(() => {
    let unlisten = null;
    let gone = false;
    import('@tauri-apps/api/event').then(async ({ listen }) => {
      const off = await listen('theme-agent-saved', async (e) => {
        const { id, name, deleted } = e.payload || {};
        await refresh().catch(() => {});
        if (deleted || !id) return;
        toast(`The agent saved the theme “${name || id}”`, {
          description: 'It only applies after you review and trust it.',
          duration: 15000,
          action: {
            label: 'Review',
            onClick: async () => {
              try { setReview(await detail(id)); } catch (err) { toast.error(errText(err)); }
            },
          },
        });
      });
      if (gone) off(); else unlisten = off;
    }).catch(() => {});
    return () => { gone = true; unlisten?.(); };
  }, [refresh, detail]);
  return <ThemeTrustDialog detail={review} onClose={() => setReview(null)} />;
}

/** Edit a theme's file text in place. Built-ins save as a new copy; installed
 * themes overwrite their file. Either way the result needs trust to apply.
 */
function EditThemeTextDialog({ editing, onClose, onSaved }) {
  const importText = useThemes((s) => s.importText);
  const refresh = useThemes((s) => s.refresh);
  const [text, setText] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (editing) { setText(editing.text); setError(''); setBusy(false); } }, [editing]);
  if (!editing) return null;
  const { entry } = editing;
  const save = async () => {
    setBusy(true);
    setError('');
    try {
      const d = entry.builtin
        ? await importText(text)
        : await invoke('theme_write', { id: entry.id, text });
      if (!entry.builtin) await refresh();
      onSaved(d);
    } catch (e) { setError(errText(e)); setBusy(false); }
  };
  return (
    <Dialog open={!!editing} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-1.5">
            {entry.builtin ? `Edit a copy of ${entry.name}` : `Edit ${entry.name}`}
            <InfoTip>{entry.builtin
              ? 'Built-in themes stay as they are — saving creates your own copy. Change the "name" to tell them apart.'
              : 'Saving replaces the theme file. You review and trust it again before the changes apply.'}</InfoTip>
          </DialogTitle>
          <DialogDescription className="sr-only">Edit the theme file.</DialogDescription>
        </DialogHeader>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
          className="h-[55vh] w-full resize-none rounded-lg border border-border bg-muted/30 px-3 py-2.5 font-mono text-[12px] leading-relaxed focus:outline-none focus:ring-1 focus:ring-ring"
        />
        {error && <p className="break-all text-[12px] text-destructive">{error}</p>}
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="h-7 text-xs" disabled={busy || !text.trim()} onClick={save}>
            {entry.builtin ? 'Save as copy' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Paste a theme file's text. */
function PasteDialog({ open, onClose, onImported }) {
  const importText = useThemes((s) => s.importText);
  const [text, setText] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => { if (open) { setText(''); setError(''); } }, [open]);
  const go = async () => {
    setBusy(true);
    setError('');
    try {
      const d = await importText(text);
      onImported(d);
      onClose();
    } catch (e) { setError(errText(e)); } finally { setBusy(false); }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-1.5">
            Paste a theme
            <InfoTip>A <code>.rustic-theme.json</code> file's contents, or an older palette (JSON / TOML). You'll review it before it applies.</InfoTip>
          </DialogTitle>
          <DialogDescription className="sr-only">Paste a theme file's contents.</DialogDescription>
        </DialogHeader>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
          placeholder={'{\n  "format": 2,\n  "name": "My Theme",\n  "dark": { "bg": "#111317", "fg": "#e6e8eb", "primary": "#6b9bff" },\n  "light": { "bg": "#f6f7f9", "fg": "#16191d", "primary": "#2f5fd0" },\n  "style": { "radius": "0.5rem" }\n}'}
          className="h-56 w-full resize-none rounded-lg border border-border bg-muted/30 px-3 py-2.5 font-mono text-[12px] focus:outline-none focus:ring-1 focus:ring-ring"
        />
        {error && <p className="break-all text-[12px] text-destructive">{error}</p>}
        <DialogFooter>
          <Button variant="outline" size="sm" className="h-7 text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="h-7 text-xs" disabled={busy || !text.trim()} onClick={go}>Import</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const RECIPE_LABELS = {
  surface: { glass: 'Glass' },
  button: { pill: 'Pill buttons', raised: 'Raised buttons', brutal: 'Brutal buttons', glow: 'Glow buttons' },
  texture: { grain: 'Grain', grid: 'Grid', dots: 'Dots', scanlines: 'Scanlines' },
  density: { compact: 'Compact', comfortable: 'Airy' },
  shadow: { hard: 'Hard shadows', glow: 'Glow', strong: 'Deep shadows' },
};

/** Short labels for the style recipes a theme opts into. */
function recipeTags(style) {
  if (!style) return [];
  return Object.entries(RECIPE_LABELS).map(([k, m]) => m[style[k]]).filter(Boolean);
}

/** A tiny sample button drawn in the theme's own colors and shape. */
function RecipePreview({ style, colors }) {
  if (!colors) return null;
  const [bg, card, fg, primary] = colors;
  const s = style || {};
  const radius = s.button === 'pill' ? '9999px' : (s.radius || '0.5rem');
  const btn = {
    background: primary,
    color: bg,
    borderRadius: radius,
    border: s.button === 'brutal' ? `2px solid ${fg}` : '1px solid transparent',
    boxShadow: s.button === 'brutal' ? `2px 2px 0 ${fg}`
      : s.button === 'glow' ? `0 0 10px -1px ${primary}`
      : s.button === 'raised' ? '0 2px 4px -1px rgb(0 0 0 / .4)' : 'none',
    fontWeight: s.button === 'brutal' ? 700 : 500,
  };
  const panel = {
    background: card,
    color: fg,
    borderRadius: s.radius || '0.5rem',
    border: `${s.border_width || '1px'} solid ${s.shadow === 'hard' ? fg : `${fg}22`}`,
    boxShadow: s.shadow === 'hard' ? `2px 2px 0 ${fg}` : s.shadow === 'glow' ? `0 0 12px -4px ${primary}` : 'none',
  };
  return (
    <div className="flex items-center gap-2 rounded-md p-2" style={{ background: bg }}>
      <span className="px-2 py-0.5 text-[10px]" style={btn}>Button</span>
      <span className="flex-1 truncate px-2 py-0.5 text-[10px]" style={panel}>Panel</span>
    </div>
  );
}

/** One theme card. */
function ThemeCard({ entry, active, onApply, onReview, onExport, onDelete, onEdit, onCopy, onEditText }) {
  const dark = entry.swatches?.dark;
  const light = entry.swatches?.light;
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onApply(entry)}
      onKeyDown={(e) => e.key === 'Enter' && onApply(entry)}
      className={cn(
        'group relative cursor-pointer space-y-2 rounded-xl border p-2.5 transition-colors',
        active ? 'border-primary/70 bg-primary/5' : 'border-border/60 hover:bg-muted/40',
      )}
    >
      <div className="flex gap-1.5">
        {dark && <Swatches colors={dark} className="flex-1" />}
        {light && <Swatches colors={light} className="flex-1" />}
      </div>
      {recipeTags(entry.style).length > 0 && <RecipePreview style={entry.style} colors={dark || light} />}
      <div className="flex items-center gap-1.5">
        <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium" title={entry.name}>{entry.name}</span>
        {active && <Check className="size-3.5 shrink-0 text-primary" />}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" title="Theme actions" onClick={(e) => e.stopPropagation()} className="rounded p-0.5 text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground group-hover:opacity-100 data-[state=open]:opacity-100">
              <MoreHorizontal className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
            {!entry.builtin && <DropdownMenuItem onClick={() => onReview(entry)}><ShieldCheck className="size-3.5" /> Review{entry.trusted ? '' : ' & trust'}</DropdownMenuItem>}
            <DropdownMenuItem onClick={() => onEditText(entry)}><Pencil className="size-3.5" /> {entry.builtin ? 'Edit a copy' : 'Edit'}</DropdownMenuItem>
            {!IS_WEB && entry.file && <DropdownMenuItem onClick={() => onEdit(entry)}><FileCode2 className="size-3.5" /> Open in editor</DropdownMenuItem>}
            <DropdownMenuItem onClick={() => onCopy(entry)}><Copy className="size-3.5" /> Copy theme file</DropdownMenuItem>
            <DropdownMenuItem onClick={() => onExport(entry)}><Download className="size-3.5" /> Download…</DropdownMenuItem>
            {!entry.builtin && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem className="text-destructive focus:text-destructive [&_svg]:text-destructive" onClick={() => onDelete(entry)}><Trash2 className="size-3.5" /> Delete</DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <div className="flex flex-wrap gap-1 text-[10px]">
        {entry.builtin && <span className="rounded bg-muted px-1.5 py-0.5 text-muted-foreground">Built-in</span>}
        {entry.modes.length > 1 && <span className="rounded bg-muted px-1.5 py-0.5 text-muted-foreground">Light + Dark</span>}
        {entry.has_css && <span className="rounded bg-info/15 px-1.5 py-0.5 text-info">Custom CSS</span>}
        {recipeTags(entry.style).map((t) => <span key={t} className="rounded bg-special/15 px-1.5 py-0.5 text-special">{t}</span>)}
        {!entry.builtin && !entry.trusted && <span className="flex items-center gap-0.5 rounded bg-warning/15 px-1.5 py-0.5 text-warning"><ShieldAlert className="size-2.5" /> Not trusted</span>}
      </div>
    </div>
  );
}

/** Settings › Appearance › Themes. */
export function ThemesSection() {
  const { list, active, refresh, detail, importFile, importText, apply, setMode, remove } = useThemes();
  const [review, setReview] = useState(null);
  const [paste, setPaste] = useState(false);
  const fileInput = useRef(null);
  const [settingsActive, setSettingsActive] = useState(null);
  const [editing, setEditing] = useState(null);

  useEffect(() => { refresh().catch((e) => toast.error(errText(e))); }, [refresh]);
  useEffect(() => {
    invoke('get_settings').then((s) => setSettingsActive(s?.theme?.active_theme || null)).catch(() => {});
  }, [active]);

  const openReview = async (entry) => {
    try { setReview(await detail(entry.id)); } catch (e) { toast.error(errText(e)); }
  };
  const onApply = async (entry) => {
    if (!entry.builtin && !entry.trusted) { openReview(entry); return; }
    try { await apply(entry.id); toast.success(`Applied ${entry.name}`); } catch (e) { toast.error(errText(e)); }
  };
  const onImported = (d) => { setReview(d); };
  const pickFile = async () => {
    if (IS_WEB) { fileInput.current?.click(); return; }
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const path = await open({ multiple: false, title: 'Import a theme', filters: [{ name: 'Rustic themes', extensions: ['json', 'toml'] }] });
      if (!path) return;
      onImported(await importFile(Array.isArray(path) ? path[0] : path));
    } catch (e) { toast.error(errText(e)); }
  };
  const onBrowserFile = async (e) => {
    const f = e.target.files?.[0];
    e.target.value = '';
    if (!f) return;
    try { onImported(await importText(await f.text())); } catch (err) { toast.error(errText(err)); }
  };
  const onExport = async (entry) => {
    try {
      const d = await detail(entry.id);
      const fileName = `${entry.id}.rustic-theme.json`;
      if (IS_WEB) {
        const url = URL.createObjectURL(new Blob([d.text], { type: 'application/json' }));
        const a = Object.assign(document.createElement('a'), { href: url, download: fileName });
        a.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        return;
      }
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({ defaultPath: fileName, filters: [{ name: 'Rustic theme', extensions: ['json'] }] });
      if (!path) return;
      await invoke('theme_export', { id: entry.id, path });
      toast.success(`Exported ${entry.name}`);
    } catch (e) { toast.error(errText(e)); }
  };
  const downloadTemplate = async () => {
    const fileName = 'my-theme.rustic-theme.json';
    try {
      if (IS_WEB) {
        const text = await invoke('theme_template', {});
        const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
        const a = Object.assign(document.createElement('a'), { href: url, download: fileName });
        a.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        return;
      }
      const { save } = await import('@tauri-apps/plugin-dialog');
      const path = await save({ defaultPath: fileName, title: 'Save theme template', filters: [{ name: 'Rustic theme', extensions: ['json'] }] });
      if (!path) return;
      await invoke('theme_template', { path });
      toast.success('Template saved — edit it, then use Import file.');
    } catch (e) { toast.error(errText(e)); }
  };
  const onDelete = async (entry) => {
    try { await remove(entry.id); toast.success(`Deleted ${entry.name}`); } catch (e) { toast.error(errText(e)); }
  };
  const onEdit = async (entry) => {
    try {
      const { useEditor } = await import('@/state/editor');
      useEditor.getState().openFile(entry.file);
      toast.info('After saving, review and trust the theme again to apply your changes.');
    } catch (e) { toast.error(errText(e)); }
  };
  const onCopy = async (entry) => {
    try {
      const d = await detail(entry.id);
      await navigator.clipboard.writeText(d.text);
      toast.success(`Copied ${entry.name} theme file`);
    } catch (e) { toast.error(errText(e)); }
  };
  const onEditText = async (entry) => {
    try {
      const d = await detail(entry.id);
      setEditing({ entry, text: d.text });
    } catch (e) { toast.error(errText(e)); }
  };
  const openFolder = async () => {
    try {
      const dir = await invoke('theme_folder');
      await invoke('reveal_in_file_manager', { path: dir });
    } catch (e) { toast.error(errText(e)); }
  };

  const mode = active?.mode || 'dark';
  const activeId = active?.id;
  const effectiveMode = mode === 'system'
    ? (window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
    : mode;
  const activeEntry = list.find((t) => t.id === activeId);
  const missingMode = activeEntry && activeEntry.modes.length && !activeEntry.modes.includes(effectiveMode) ? effectiveMode : null;
  const needsReview = active?.fallback && settingsActive && settingsActive !== activeId
    ? list.find((t) => t.id === settingsActive)
    : null;

  return (
    <section data-settings-anchor="themes" className="mb-6">
      <div className="mb-2 flex items-center gap-2 px-1">
        <h3 className="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70">
          Themes
          <InfoTip>Themes set colors, fonts and corner style, and can include custom CSS. Imported or edited themes only apply after you review and trust them. Theme files live in Rustic's themes folder.</InfoTip>
        </h3>
        <div className="ml-auto flex items-center gap-1">
          <Button size="sm" variant="ghost" className="h-6 gap-1 px-2 text-[11px]" onClick={pickFile}><Upload className="size-3" /> Import file</Button>
          <Button size="sm" variant="ghost" className="h-6 gap-1 px-2 text-[11px]" onClick={() => setPaste(true)}><ClipboardPaste className="size-3" /> Paste</Button>
          <Button size="sm" variant="ghost" className="h-6 gap-1 px-2 text-[11px]" title="Download an editable starter theme file" onClick={downloadTemplate}><FileDown className="size-3" /> Template</Button>
          {!IS_WEB && <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground" title="Open themes folder" onClick={openFolder}><FolderOpen className="size-3.5" /></Button>}
        </div>
      </div>
      <input ref={fileInput} type="file" accept=".json,.toml,application/json" className="hidden" onChange={onBrowserFile} />

      <Slider
        className="mb-3"
        value={mode}
        onChange={(m) => setMode(m).catch((e) => toast.error(errText(e)))}
        options={[
          { id: 'dark', label: 'Dark', icon: Moon },
          { id: 'light', label: 'Light', icon: Sun },
          { id: 'system', label: 'Match system', icon: MonitorSmartphone },
        ]}
      />

      {missingMode && (
        <p className="-mt-1.5 mb-3 px-1 text-[11px] text-muted-foreground">
          {activeEntry.name} has no {missingMode} version — showing Rustic's default {missingMode} palette.
        </p>
      )}

      {activeEntry && (
        <div className="mb-3 flex items-center gap-2 rounded-lg border border-border/60 bg-muted/20 px-3 py-1.5">
          <span className="text-[11px] text-muted-foreground">Active</span>
          <span className="min-w-0 flex-1 truncate text-[12px] font-medium">{activeEntry.name}</span>
          <Button size="sm" variant="ghost" className="h-6 gap-1 px-2 text-[11px]" title="Copy the theme file to the clipboard" onClick={() => onCopy(activeEntry)}><Copy className="size-3" /> Copy</Button>
          <Button size="sm" variant="ghost" className="h-6 gap-1 px-2 text-[11px]" title="Download the theme file" onClick={() => onExport(activeEntry)}><Download className="size-3" /> Download</Button>
          <Button size="sm" variant="ghost" className="h-6 gap-1 px-2 text-[11px]" title={activeEntry.builtin ? 'Edit a copy of this built-in theme' : 'Edit this theme'} onClick={() => onEditText(activeEntry)}><Pencil className="size-3" /> Edit</Button>
        </div>
      )}

      {needsReview && (
        <div className="mb-3 flex items-center gap-2 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-[11.5px]">
          <ShieldAlert className="size-4 shrink-0 text-warning" />
          <span className="min-w-0 flex-1">“{needsReview.name}” changed or isn’t trusted, so the default theme is showing.</span>
          <Button size="sm" variant="outline" className="h-6 text-[11px]" onClick={() => openReview(needsReview)}>Review</Button>
        </div>
      )}

      <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2.5">
        {list.map((t) => (
          <ThemeCard
            key={t.id}
            entry={t}
            active={t.id === activeId}
            onApply={onApply}
            onReview={openReview}
            onExport={onExport}
            onDelete={onDelete}
            onEdit={onEdit}
            onCopy={onCopy}
            onEditText={onEditText}
          />
        ))}
      </div>

      <PasteDialog open={paste} onClose={() => setPaste(false)} onImported={onImported} />
      <EditThemeTextDialog editing={editing} onClose={() => setEditing(null)} onSaved={(d) => { setEditing(null); setReview(d); }} />
      <ThemeTrustDialog detail={review} onClose={() => setReview(null)} />
    </section>
  );
}

export default ThemesSection;
