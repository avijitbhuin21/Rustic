import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { toast } from 'sonner';
import {
  Check, X, Download, RefreshCw, Trash2, ExternalLink, Pencil, Plus, Search, Loader2,
} from 'lucide-react';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import { Label } from '@/components/ui/label';
import { ScrollArea } from '@/components/ui/scroll-area';
import { cn } from '@/lib/utils';

// ─── Status helpers ───────────────────────────────────────────────────────────

function StatusBadge({ status }) {
  if (status.kind === 'bundled') {
    return (
      <Badge variant="outline" className="h-5 gap-1 border-violet-500/40 bg-violet-500/10 px-1.5 text-[10px] text-violet-400">
        <Check className="size-3" /> Bundled
      </Badge>
    );
  }
  if (status.kind === 'installed') {
    return (
      <Badge variant="outline" className="h-5 gap-1 border-emerald-500/40 bg-emerald-500/10 px-1.5 text-[10px] text-emerald-500">
        <Check className="size-3" /> Installed{status.installed_version ? ` ${status.installed_version}` : ''}
      </Badge>
    );
  }
  if (status.kind === 'detected') {
    return (
      <Badge variant="outline" className="h-5 gap-1 border-sky-500/40 bg-sky-500/10 px-1.5 text-[10px] text-sky-500">
        <Check className="size-3" /> Detected on PATH
      </Badge>
    );
  }
  return (
    <Badge variant="outline" className="h-5 gap-1 border-muted-foreground/30 px-1.5 text-[10px] text-muted-foreground">
      Not installed
    </Badge>
  );
}

// Synthetic entry for the in-process Prettier worker. Inserted into the
// modal's entry list so users see what handles js/ts/css/html/md/yaml on
// save, without it pretending to be a PATH-installed binary.
const BUNDLED_PRETTIER_ENTRY = {
  builtin: {
    id: 'prettier-bundled',
    display_name: 'Prettier',
    languages: ['javascript', 'typescript', 'json', 'css', 'scss', 'less', 'html', 'markdown', 'yaml', 'vue'],
    description: 'Bundled with Rustic. Runs in a worker — no install, no PATH binary, ~4 MB on disk.',
    install_kind: 'bundled',
    binary: 'prettier/standalone',
    stdin: false,
    install_url: null,
  },
  custom: null,
  status: { id: 'prettier-bundled', kind: 'bundled', resolved_path: null, installed_version: null },
};

// ─── Custom-formatter add/edit dialog ─────────────────────────────────────────

function CustomFormatterDialog({ open, initial, onClose, onSaved }) {
  const [form, setForm] = useState(() => ({
    id: '', display_name: '', languages: '', command: '', args: '', stdin: true, description: '',
  }));
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const isEdit = !!initial;

  useEffect(() => {
    if (!open) return;
    setError('');
    if (initial) {
      setForm({
        id: initial.id,
        display_name: initial.display_name,
        languages: initial.languages.join(', '),
        command: initial.command,
        args: initial.args.join(' '),
        stdin: initial.stdin,
        description: initial.description ?? '',
      });
    } else {
      setForm({
        id: '', display_name: '', languages: '', command: '', args: '', stdin: true, description: '',
      });
    }
  }, [open, initial]);

  async function handleSave() {
    setError('');
    const id = form.id.trim();
    const display_name = form.display_name.trim();
    const command = form.command.trim();
    const langs = form.languages.split(',').map((s) => s.trim()).filter(Boolean);
    if (!id) return setError('ID is required.');
    if (!/^[a-z0-9._-]+$/i.test(id)) return setError('ID may only contain letters, digits, dots, dashes, underscores.');
    if (!display_name) return setError('Display name is required.');
    if (!command) return setError('Command is required.');
    if (langs.length === 0) return setError('At least one language is required.');

    // Shell-style arg split: respects double quotes so users can pass things
    // like `--config "C:\Program Files\foo\bar"`.
    const args = splitArgs(form.args);

    const payload = {
      id, display_name,
      languages: langs,
      command,
      args,
      stdin: form.stdin,
      description: form.description.trim(),
    };
    setSaving(true);
    try {
      if (isEdit) await invoke('formatter_update_custom', { formatter: payload });
      else await invoke('formatter_add_custom', { formatter: payload });
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
      <DialogContent aria-describedby={undefined} className="w-[520px] sm:max-w-[520px] gap-0 p-0 overflow-hidden">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">
            {isEdit ? `Edit "${initial.display_name}"` : 'Add custom formatter'}
          </DialogTitle>
          <p className="text-[12px] text-muted-foreground mt-1">
            Define how to invoke an external formatter. Use <code className="text-[11px]">{'{file}'}</code> in args to substitute the file path; the source is piped via stdin when enabled.
          </p>
        </DialogHeader>

        <div className="px-5 py-4 space-y-3">
          <div className="grid grid-cols-2 gap-3">
            <Field label="ID" hint="lowercase identifier, e.g. stylua" disabled={isEdit}>
              <Input value={form.id} disabled={isEdit}
                onChange={(e) => setForm((f) => ({ ...f, id: e.target.value }))}
                className="h-7 text-xs" placeholder="stylua" />
            </Field>
            <Field label="Display name">
              <Input value={form.display_name}
                onChange={(e) => setForm((f) => ({ ...f, display_name: e.target.value }))}
                className="h-7 text-xs" placeholder="StyLua" />
            </Field>
          </div>
          <Field label="Languages" hint="Comma-separated, e.g. lua, fennel">
            <Input value={form.languages}
              onChange={(e) => setForm((f) => ({ ...f, languages: e.target.value }))}
              className="h-7 text-xs" placeholder="lua" />
          </Field>
          <Field label="Command" hint="Binary on PATH or absolute path">
            <Input value={form.command}
              onChange={(e) => setForm((f) => ({ ...f, command: e.target.value }))}
              className="h-7 text-xs" placeholder="stylua" />
          </Field>
          <Field label="Arguments" hint='Space-separated. Use "quotes" around args containing spaces.'>
            <Input value={form.args}
              onChange={(e) => setForm((f) => ({ ...f, args: e.target.value }))}
              className="h-7 text-xs" placeholder="--stdin-filepath {file} -" />
          </Field>
          <div className="flex items-center justify-between py-1">
            <div className="flex flex-col">
              <Label className="text-[13px] font-normal">Pipe source via stdin</Label>
              <span className="text-[11px] text-muted-foreground">Disable if the formatter only reads from disk.</span>
            </div>
            <Switch checked={form.stdin}
              onCheckedChange={(v) => setForm((f) => ({ ...f, stdin: v }))} />
          </div>
          <Field label="Description (optional)">
            <Input value={form.description}
              onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
              className="h-7 text-xs" placeholder="Short note shown next to the row" />
          </Field>
          {error && <p className="text-[12px] text-destructive">{error}</p>}
        </div>

        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60 flex-row justify-end gap-2">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button size="sm" className="text-xs" onClick={handleSave} disabled={saving}>
            {saving ? <Loader2 className="size-3 animate-spin" /> : (isEdit ? 'Save' : 'Add')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Field({ label, hint, children, disabled }) {
  return (
    <label className={cn('flex flex-col gap-1', disabled && 'opacity-60')}>
      <span className="text-[12px] text-muted-foreground">{label}</span>
      {children}
      {hint && <span className="text-[10px] text-muted-foreground/70">{hint}</span>}
    </label>
  );
}

function splitArgs(str) {
  const out = [];
  let cur = '';
  let q = null;
  for (let i = 0; i < str.length; i++) {
    const c = str[i];
    if (q) {
      if (c === q) q = null;
      else cur += c;
    } else if (c === '"' || c === "'") {
      q = c;
    } else if (/\s/.test(c)) {
      if (cur) { out.push(cur); cur = ''; }
    } else {
      cur += c;
    }
  }
  if (cur) out.push(cur);
  return out;
}

// ─── Row actions ──────────────────────────────────────────────────────────────

function BuiltinRow({ entry, onChanged, onEdit }) {
  const { builtin: b, status } = entry;
  const [busy, setBusy] = useState(false);
  const isDownload = b.install_kind === 'download';
  const isInstalled = status.kind === 'installed' || status.kind === 'detected';

  async function handleInstall() {
    setBusy(true);
    try {
      await invoke('formatter_install', { id: b.id });
      toast.success(`Installed ${b.display_name}`);
      onChanged?.();
    } catch (e) {
      toast.error(`Install failed: ${e}`);
    } finally { setBusy(false); }
  }

  async function handleUpdate() {
    setBusy(true);
    try {
      // Fetch the latest version first, only re-download if newer or if we
      // can't tell (no installed_version recorded — happens for entries
      // installed under older versions of the app).
      const info = await invoke('formatter_check_update', { id: b.id });
      if (status.installed_version && info.latest_version === status.installed_version) {
        toast.message(`${b.display_name} is already on ${info.latest_version}`);
        return;
      }
      await invoke('formatter_update', { id: b.id });
      toast.success(`Updated ${b.display_name} to ${info.latest_version}`);
      onChanged?.();
    } catch (e) {
      toast.error(`Update failed: ${e}`);
    } finally { setBusy(false); }
  }

  async function handleUninstall() {
    setBusy(true);
    try {
      await invoke('formatter_uninstall', { id: b.id });
      toast.success(`Removed ${b.display_name}`);
      onChanged?.();
    } catch (e) {
      toast.error(`Remove failed: ${e}`);
    } finally { setBusy(false); }
  }

  async function handleOpenUrl() {
    if (!b.install_url) return;
    try { await openUrl(b.install_url); } catch (e) { toast.error(`Open failed: ${e}`); }
  }

  return (
    <Row entry={entry}>
      <div className="flex items-center gap-1.5">
        {isDownload ? (
          <>
            {isInstalled ? (
              <>
                <Button variant="ghost" size="sm" disabled={busy}
                  onClick={handleUpdate}
                  className="h-6 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground">
                  {busy ? <Loader2 className="size-3 animate-spin" /> : <RefreshCw className="size-3" />}
                  Update
                </Button>
                <Button variant="ghost" size="sm" disabled={busy}
                  onClick={handleUninstall}
                  className="h-6 gap-1 px-2 text-[11px] text-muted-foreground hover:text-destructive">
                  <Trash2 className="size-3" /> Remove
                </Button>
              </>
            ) : (
              <Button size="sm" disabled={busy}
                onClick={handleInstall}
                className="h-6 gap-1 px-2.5 text-[11px]">
                {busy ? <Loader2 className="size-3 animate-spin" /> : <Download className="size-3" />}
                Install
              </Button>
            )}
          </>
        ) : (
          <>
            {!isInstalled && b.install_url && (
              <Button variant="secondary" size="sm" onClick={handleOpenUrl}
                className="h-6 gap-1 px-2.5 text-[11px]">
                <ExternalLink className="size-3" /> Install guide
              </Button>
            )}
          </>
        )}
      </div>
    </Row>
  );
}

function CustomRow({ entry, onChanged, onEdit }) {
  const { custom: c, status } = entry;
  const [busy, setBusy] = useState(false);

  async function handleRemove() {
    setBusy(true);
    try {
      await invoke('formatter_remove_custom', { id: c.id });
      toast.success(`Removed ${c.display_name}`);
      onChanged?.();
    } catch (e) {
      toast.error(`Remove failed: ${e}`);
    } finally { setBusy(false); }
  }

  return (
    <Row entry={entry}>
      <div className="flex items-center gap-1.5">
        <Button variant="ghost" size="sm" onClick={() => onEdit?.(c)}
          className="h-6 gap-1 px-2 text-[11px] text-muted-foreground hover:text-foreground">
          <Pencil className="size-3" /> Edit
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={handleRemove}
          className="h-6 gap-1 px-2 text-[11px] text-muted-foreground hover:text-destructive">
          <Trash2 className="size-3" /> Remove
        </Button>
      </div>
    </Row>
  );
}

function Row({ entry, children }) {
  const meta = entry.builtin ?? entry.custom;
  const languages = meta.languages.join(', ');
  return (
    <div
      className="flex items-center justify-between gap-3 px-4 py-2.5 hover:bg-muted/20 transition-colors"
      title={entry.status.resolved_path || ''}
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-[13px] font-medium truncate">{meta.display_name}</span>
          <span className="text-[11px] text-muted-foreground truncate">{languages}</span>
          {entry.custom && (
            <Badge variant="outline" className="h-4 px-1 text-[10px] text-muted-foreground border-border/60">
              Custom
            </Badge>
          )}
        </div>
        {meta.description && (
          <p className="text-[11px] text-muted-foreground/80 leading-snug truncate mt-0.5">
            {meta.description}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2 shrink-0">
        <StatusBadge status={entry.status} />
        {children}
      </div>
    </div>
  );
}

// ─── Main modal ───────────────────────────────────────────────────────────────

export function FormattersModal({ open, onClose }) {
  const [entries, setEntries] = useState([]);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState('');
  const [addOpen, setAddOpen] = useState(false);
  const [editTarget, setEditTarget] = useState(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke('formatter_list');
      // Splice the bundled Prettier row in right after Ruff/shfmt and before
      // the toolchain-only entries so users see in-app formatters together.
      const downloadCount = list.filter((e) => e.builtin?.install_kind === 'download').length;
      const merged = [
        ...list.slice(0, downloadCount),
        BUNDLED_PRETTIER_ENTRY,
        ...list.slice(downloadCount),
      ];
      setEntries(merged);
    } catch (e) {
      toast.error(`Failed to load formatters: ${e}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { if (open) refresh(); }, [open, refresh]);

  const filtered = entries.filter((e) => {
    if (!query) return true;
    const q = query.toLowerCase();
    const meta = e.builtin ?? e.custom;
    return (
      meta.display_name.toLowerCase().includes(q) ||
      meta.languages.some((l) => l.toLowerCase().includes(q)) ||
      (meta.description ?? '').toLowerCase().includes(q)
    );
  });

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent
        aria-describedby={undefined}
        showCloseButton={false}
        className="w-[680px] sm:max-w-[680px] gap-0 p-0 overflow-hidden flex flex-col max-h-[80vh]"
      >
        <DialogHeader className="px-5 pt-4 pb-3 border-b border-border/60 shrink-0 space-y-0">
          <div className="flex items-center gap-3">
            <DialogTitle className="text-[14px] flex-1">Formatters</DialogTitle>
            <div className="relative w-64">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input value={query} onChange={(e) => setQuery(e.target.value)}
                placeholder="Filter by name or language..."
                className="h-7 pl-7 text-[12px]" />
            </div>
            <Button size="sm" variant="secondary" className="h-7 gap-1.5 px-2.5 text-[11px]"
              onClick={() => setAddOpen(true)}>
              <Plus className="size-3" /> Add custom
            </Button>
          </div>
        </DialogHeader>

        <ScrollArea className="flex-1 min-h-0">
          <div className="divide-y divide-border/40">
            {loading && entries.length === 0 && (
              <div className="px-3 py-6 text-center text-[12px] text-muted-foreground">
                <Loader2 className="mx-auto mb-2 size-4 animate-spin" /> Loading formatters…
              </div>
            )}
            {!loading && filtered.length === 0 && (
              <div className="px-3 py-6 text-center text-[12px] text-muted-foreground">
                No formatters match "{query}".
              </div>
            )}
            {filtered.map((entry) => (
              <React.Fragment key={entry.builtin?.id ?? entry.custom?.id}>
                {entry.builtin
                  ? <BuiltinRow entry={entry} onChanged={refresh} />
                  : <CustomRow entry={entry} onChanged={refresh} onEdit={setEditTarget} />}
              </React.Fragment>
            ))}
          </div>
        </ScrollArea>

        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60 flex-row justify-between sm:justify-between gap-2 shrink-0">
          <span className="text-[11px] text-muted-foreground">
            {entries.length} formatter{entries.length === 1 ? '' : 's'} configured
          </span>
          <Button size="sm" variant="secondary" className="text-xs" onClick={onClose}>
            Close
          </Button>
        </DialogFooter>

        <CustomFormatterDialog
          open={addOpen || !!editTarget}
          initial={editTarget}
          onClose={() => { setAddOpen(false); setEditTarget(null); }}
          onSaved={refresh}
        />
      </DialogContent>
    </Dialog>
  );
}
