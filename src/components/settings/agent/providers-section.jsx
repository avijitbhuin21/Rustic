// Providers section: native provider cards, compatible endpoints, FreeBuff, model dialogs.
// Split out of agent-settings.jsx (A4).
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronRight, ChevronDown, Plus, Eye, EyeOff, Pencil, Trash2, Info, RefreshCw,
  ClipboardEdit, X, Check, FileText, Copy, List, Loader2,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter,
} from '@/components/ui/dialog';
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem, SelectGroup, SelectLabel,
} from '@/components/ui/select';
import { ScrollArea } from '@/components/ui/scroll-area';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';
import { useExplorer } from '@/state/explorer';
import { useLayout } from '@/state/layout';
import { useLiveModels } from '@/state/live-models';
import { IS_WEB } from '@/lib/platform';
import { Section, isTauri, prettyProviderError, slugify, useAiConfig, validateProviderKey } from './shared';

// ─── AI Providers ─────────────────────────────────────────────────────────────

export const NATIVE_PROVIDERS = [
  { type: 'Claude',   label: 'Anthropic',     defaultModel: 'claude-sonnet-4-5',  keyPlaceholder: 'sk-ant-…' },
  { type: 'OpenAi',   label: 'OpenAI',        defaultModel: 'gpt-5-mini',         keyPlaceholder: 'sk-…' },
  { type: 'Gemini',   label: 'Google Gemini', defaultModel: 'gemini-2.5-flash',   keyPlaceholder: 'AIza…' },
  { type: 'OpenRouter', label: 'OpenRouter',  defaultModel: 'openrouter/auto',    keyPlaceholder: 'sk-or-…' },
];
export function ModelsDialog({ open, onClose, title, providerType, baseUrl }) {
  const [models, setModels] = useState(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState('');

  const load = async (force = false) => {
    if (!isTauri()) return;
    setLoading(true); setError('');
    try {
      const list = await invoke('fetch_ai_models', {
        providerType,
        apiKey: '__STORED__',
        baseUrl: baseUrl || null,
        forceRefresh: force,
        // "View models" is a discovery panel — show everything the provider
        // reports (chat, image, video, audio). The chat-only NON_CHAT_KEYWORDS
        // filter is for the subagent picker, not this dialog.
        includeAll: true,
      });
      setModels(Array.isArray(list) ? list : []);
      // A manual refresh here busts the backend's 5-minute cache; drop the chat
      // picker's frontend cache too so both views stay in sync.
      if (force) useLiveModels.getState().resetAll();
    } catch (e) {
      setError(String(e));
      setModels([]);
    } finally { setLoading(false); }
  };

  useEffect(() => {
    if (open) { setQuery(''); setModels(null); setError(''); load(false); }
    // eslint-disable-next-line
  }, [open, providerType, baseUrl]);

  const filtered = (models || []).filter((m) => !query || m.toLowerCase().includes(query.toLowerCase()));

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[560px] sm:max-w-[560px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">{title} — Models</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-3 border-b border-border/60 flex items-center gap-2">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter models…"
            className="h-8 text-xs flex-1"
          />
          <span className="text-[11px] text-muted-foreground">
            {loading ? 'Loading…' : `${filtered.length}${models && filtered.length !== models.length ? ` / ${models.length}` : ''}`}
          </span>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => load(true)} title="Refresh">
            <RefreshCw className={cn('size-3.5', loading && 'animate-spin')} />
          </Button>
        </div>
        <div>
          {error ? (
            <div className="px-5 py-4 text-[12px] text-destructive break-all space-y-2">
              <div>{error}</div>
            </div>
          ) : models && filtered.length === 0 && !loading ? (
            <div className="px-5 py-4 text-[12px] text-muted-foreground">
              {query ? 'No models match the filter.' : 'No models returned.'}
            </div>
          ) : (
            // shadcn ScrollArea needs a definite height (not max-h) on the root
            // for its internal Viewport to overflow. The previous max-h-[55vh]
            // wrapper clipped tall lists without making them scrollable.
            <ScrollArea className="h-[55vh]">
              <ul className="divide-y divide-border/30">
                {filtered.map((m) => (
                  <li
                    key={m}
                    className="flex items-center gap-2 px-5 py-2 text-[12px] font-mono text-foreground/90 hover:bg-muted/40 group"
                  >
                    <span className="flex-1 truncate">{m}</span>
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      className="size-6 opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-foreground"
                      onClick={() => { navigator.clipboard.writeText(m); toast.success('Copied'); }}
                      title="Copy"
                    >
                      <Copy className="size-3" />
                    </Button>
                  </li>
                ))}
              </ul>
            </ScrollArea>
          )}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button size="sm" className="text-xs" onClick={onClose}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function EditProviderDialog({ open, onClose, onSaved, providerType, providerLabel, entry, allowBaseUrl = false, allowName = false }) {
  const [apiKey, setApiKey] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [name, setName] = useState('');
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!open) return;
    setApiKey('');
    setShowKey(false);
    setBaseUrl(entry?.base_url || '');
    setName(entry?.name || '');
    setError('');
  }, [open, entry]);

  const save = async () => {
    setSaving(true);
    setError('');
    const newKey = apiKey.trim();
    // Only verify when the user typed a replacement key — a blank field keeps
    // the existing stored key, which was already validated when it was added.
    if (newKey) {
      const verr = await validateProviderKey({
        providerType,
        apiKey: newKey,
        baseUrl: allowBaseUrl ? (baseUrl.trim() || null) : (entry?.base_url || null),
      });
      if (verr) { setError(verr); setSaving(false); return; }
    }
    try {
      await invoke('set_ai_provider', {
        providerType,
        // Sentinel keeps the stored key when the user didn't enter a new one.
        apiKey: newKey || '__STORED__',
        // Default model is no longer user-facing — pass through whatever is
        // already stored so backend validation (which still requires the
        // field) keeps the existing value.
        model: entry?.default_model || '',
        baseUrl: allowBaseUrl ? (baseUrl.trim() || null) : null,
        name: allowName ? (name.trim() || null) : null,
      });
      onSaved?.();
      onClose();
    } catch (e) { setError(prettyProviderError(e)); }
    finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Edit {providerLabel}</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          {allowName && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Name</div>
              <Input value={name} onChange={(e) => setName(e.target.value)} className="h-8 text-xs" />
            </div>
          )}
          {allowBaseUrl && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Base URL</div>
              <Input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} className="h-8 text-xs" />
            </div>
          )}
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">API Key</div>
            <div className="relative">
              <Input
                type={showKey ? 'text' : 'password'}
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="Leave blank to keep the existing key"
                className="h-8 pr-8 text-xs"
              />
              <button
                type="button"
                onClick={() => setShowKey((s) => !s)}
                className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
              >
                {showKey ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
              </button>
            </div>
            <p className="mt-1 text-[11px] text-muted-foreground">
              Existing key stays in your OS keychain. Type a new one only if you want to replace it.
            </p>
          </div>
          {error && (
            <div className="text-[11px] text-destructive break-all">{error}</div>
          )}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={saving}>
            {saving ? (apiKey.trim() ? 'Verifying…' : 'Saving…') : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function ConnectCard({ provider, configured, onSaved }) {
  const [apiKey, setApiKey] = useState('');
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [editOpen, setEditOpen] = useState(false);
  const [modelsOpen, setModelsOpen] = useState(false);

  if (configured) {
    return (
      <>
        <div className="flex items-center gap-2 rounded-lg border border-border/60 bg-muted/30 px-3 py-2.5">
          <span className="size-2 rounded-sm bg-emerald-500" />
          <span className="text-[13px] font-medium flex-1">{provider.label}</span>
          <Badge variant="outline" className="h-5 text-[10px]">connected</Badge>
          <Button
            variant="ghost"
            size="icon-sm"
            className="size-7 text-muted-foreground hover:text-foreground"
            onClick={() => setEditOpen(true)}
            title="Edit API key / model"
          >
            <Pencil className="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            className="size-7 text-muted-foreground hover:text-foreground"
            onClick={() => setModelsOpen(true)}
            title="View models"
          >
            <List className="size-3.5" />
          </Button>
        </div>
        <EditProviderDialog
          open={editOpen}
          onClose={() => setEditOpen(false)}
          onSaved={onSaved}
          providerType={provider.type}
          providerLabel={provider.label}
          entry={configured}
        />
        <ModelsDialog
          open={modelsOpen}
          onClose={() => setModelsOpen(false)}
          title={provider.label}
          providerType={provider.type}
          baseUrl={configured.base_url}
        />
      </>
    );
  }

  return (
    <div className="rounded-lg border border-border/60 bg-muted/20 px-3 py-3">
      <div className="mb-2 flex items-center gap-2">
        <span className="size-2 rounded-sm bg-muted-foreground/40" />
        <span className="text-[13px] font-medium flex-1">{provider.label}</span>
      </div>
      <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">API Key</div>
      <div className="flex items-center gap-1.5">
        <div className="relative flex-1">
          <Input
            type={showKey ? 'text' : 'password'}
            value={apiKey}
            onChange={(e) => { setApiKey(e.target.value); if (error) setError(''); }}
            placeholder={provider.keyPlaceholder}
            className="h-8 pr-8 text-xs"
          />
          <button
            type="button"
            onClick={() => setShowKey((s) => !s)}
            className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
          >
            {showKey ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
          </button>
        </div>
      </div>
      {error && (
        <div className="mt-2 text-[11px] text-destructive break-all">{error}</div>
      )}
      <div className="mt-2 flex items-center justify-end">
        <Button
          size="sm"
          className="h-7 text-xs"
          disabled={saving || !apiKey.trim()}
          onClick={async () => {
            setSaving(true);
            setError('');
            const key = apiKey.trim();
            // Verify the key against the live provider before storing it, so an
            // invalid key reports the real reason here instead of going
            // "connected" and then erroring under "View models".
            const verr = await validateProviderKey({ providerType: provider.type, apiKey: key, baseUrl: null });
            if (verr) { setError(verr); setSaving(false); return; }
            try {
              await invoke('set_ai_provider', {
                providerType: provider.type,
                apiKey: key,
                model: provider.defaultModel || '',
                baseUrl: null,
                name: null,
              });
              setApiKey('');
              onSaved?.();
            } catch (e) { setError(prettyProviderError(e)); }
            finally { setSaving(false); }
          }}
        >
          {saving ? 'Verifying…' : 'Connect'}
        </Button>
      </div>
    </div>
  );
}

export function CompatibleAddDialog({ open, onClose, onSaved }) {
  const [name, setName] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (open) { setName(''); setBaseUrl(''); setApiKey(''); setError(''); }
  }, [open]);

  const save = async () => {
    if (!name.trim() || !baseUrl.trim() || !apiKey.trim()) return;
    setSaving(true);
    setError('');
    // Verify the endpoint + key actually answer before storing them, so a bad
    // base URL or key reports the real reason here instead of failing later.
    const verr = await validateProviderKey({ providerType: 'Compatible', apiKey: apiKey.trim(), baseUrl: baseUrl.trim() });
    if (verr) { setError(verr); setSaving(false); return; }
    try {
      await invoke('set_ai_provider', {
        providerType: 'Compatible',
        apiKey: apiKey.trim(),
        // No default model is stored at provider-add time — model picking
        // happens per-tool (sub-agent, image_create, etc.). Backend still
        // requires the field, so we send a generic placeholder.
        model: 'gpt-4o-mini',
        baseUrl: baseUrl.trim(),
        name: name.trim(),
      });
      onSaved?.();
      onClose();
    } catch (e) { setError(prettyProviderError(e)); }
    finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[460px] sm:max-w-[460px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Add OpenAI-Compatible Provider</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Name</div>
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Groq, DeepSeek, Bifrost…" className="h-8 text-xs" />
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Base URL</div>
            <Input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.groq.com/openai/v1" className="h-8 text-xs" />
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">API Key</div>
            <div className="relative">
              <Input
                type={showKey ? 'text' : 'password'}
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="sk-…"
                className="h-8 pr-8 text-xs"
              />
              <button
                type="button"
                onClick={() => setShowKey((s) => !s)}
                className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
              >
                {showKey ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
              </button>
            </div>
          </div>
          {error && (
            <div className="text-[11px] text-destructive break-all">{error}</div>
          )}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={saving || !name.trim() || !baseUrl.trim() || !apiKey.trim()}>
            {saving ? 'Verifying…' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function CompatibleEntryCard({ entry, onChanged }) {
  const [editOpen, setEditOpen] = useState(false);
  const [modelsOpen, setModelsOpen] = useState(false);
  const label = `OpenAI-Compatible${entry.name ? ` — ${entry.name}` : ''}`;
  return (
    <>
      <div className="flex items-center gap-2 rounded-lg border border-border/60 bg-muted/30 px-3 py-2.5">
        <span className="size-2 rounded-sm bg-emerald-500" />
        <span className="text-[13px] font-medium">
          OpenAI-Compatible {entry.name ? <span className="text-muted-foreground">— {entry.name}</span> : null}
        </span>
        <Badge variant="outline" className="h-5 text-[10px] text-muted-foreground">
          {entry.default_model || 'configured'}
        </Badge>
        <div className="flex-1" />
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-7 text-muted-foreground hover:text-foreground"
          onClick={() => setEditOpen(true)}
          title="Edit API key / model"
        >
          <Pencil className="size-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-7 text-muted-foreground hover:text-foreground"
          onClick={() => setModelsOpen(true)}
          title="View models"
        >
          <List className="size-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="size-7 text-muted-foreground hover:text-destructive"
          onClick={async () => {
            const key = entry.name ? `Compatible:${slugify(entry.name)}` : 'Compatible';
            try {
              await invoke('remove_ai_provider', { providerKey: key });
              onChanged?.();
            } catch (e) { toast.error(String(e)); }
          }}
          title="Remove"
        >
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      <EditProviderDialog
        open={editOpen}
        onClose={() => setEditOpen(false)}
        onSaved={onChanged}
        providerType="Compatible"
        providerLabel={label}
        entry={entry}
        allowBaseUrl
        allowName
      />
      <ModelsDialog
        open={modelsOpen}
        onClose={() => setModelsOpen(false)}
        title={label}
        providerType="Compatible"
        baseUrl={entry.base_url}
      />
    </>
  );
}


// FreeBuff is a keyless native provider: the token comes from the local
// `freebuff` CLI login (`~/.config/manicode/credentials.json`), not a typed
// key. The card auto-detects that login and toggles the provider on/off rather
// than asking for credentials.
export function FreeBuffCard({ configured, onSaved }) {
  const [detect, setDetect] = useState(null); // { available, email, reason }
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [modelsOpen, setModelsOpen] = useState(false);

  const probe = useCallback(async () => {
    if (!isTauri()) return null;
    try {
      const info = await invoke('detect_freebuff');
      setDetect(info);
      return info;
    } catch (e) {
      const info = { available: false, email: null, reason: prettyProviderError(e) };
      setDetect(info);
      return info;
    }
  }, []);

  useEffect(() => { probe(); }, [probe, configured]);

  // Account-keys popup (paste/edit the multi-account token pool).
  const [keysOpen, setKeysOpen] = useState(false);

  const enable = async () => {
    setBusy(true); setError('');
    const info = await probe();
    if (!info?.available) {
      setError(info?.reason || 'FreeBuff not detected — run `freebuff login`.');
      setBusy(false);
      return;
    }
    try {
      await invoke('set_ai_provider', {
        providerType: 'FreeBuff',
        apiKey: '',
        model: 'deepseek/deepseek-v4-pro',
        baseUrl: null,
        name: null,
      });
      onSaved?.();
      // Prompt for pool keys right after enabling.
      setKeysOpen(true);
    } catch (e) { setError(prettyProviderError(e)); }
    finally { setBusy(false); }
  };

  const disable = async () => {
    setBusy(true); setError('');
    try {
      await invoke('remove_ai_provider', { providerKey: 'FreeBuff' });
      onSaved?.();
    } catch (e) { setError(prettyProviderError(e)); }
    finally { setBusy(false); }
  };

  const isOn = !!configured;
  const account = detect?.email;

  return (
    <>
      <div className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2.5">
        <div className="flex items-center gap-2">
          <span className={cn('size-2 rounded-sm', isOn ? 'bg-emerald-500' : 'bg-muted-foreground/40')} />
          <span className="text-[13px] font-medium flex-1">FreeBuff</span>
          {isOn && (
            <Button
              variant="ghost"
              size="icon-sm"
              className="size-7 text-muted-foreground hover:text-foreground"
              onClick={() => setKeysOpen(true)}
              title="Manage account keys"
            >
              <Pencil className="size-3.5" />
            </Button>
          )}
          {isOn && (
            <Button
              variant="ghost"
              size="icon-sm"
              className="size-7 text-muted-foreground hover:text-foreground"
              onClick={() => setModelsOpen(true)}
              title="View models"
            >
              <List className="size-3.5" />
            </Button>
          )}
          <Switch checked={isOn} disabled={busy} onCheckedChange={(v) => (v ? enable() : disable())} />
        </div>
        <div className="mt-1.5 text-[11px] text-muted-foreground break-words">
          {isOn
            ? (account ? `Logged in as ${account}` : 'Connected — using your FreeBuff CLI login.')
            : (detect && !detect.available
              ? (detect.reason || 'FreeBuff not detected — run `freebuff login`.')
              : 'Uses your FreeBuff CLI login. No API key needed.')}
        </div>
        {error && <div className="mt-1.5 text-[11px] text-destructive break-all">{error}</div>}
      </div>
      <ModelsDialog
        open={modelsOpen}
        onClose={() => setModelsOpen(false)}
        title="FreeBuff"
        providerType="FreeBuff"
        baseUrl={null}
      />
      <FreeBuffKeysDialog open={keysOpen} onClose={() => setKeysOpen(false)} />
    </>
  );
}

// Popup for managing the FreeBuff multi-account token pool: paste keys (comma /
// newline separated) and/or snapshot the current CLI login. Pooled accounts
// round-robin with automatic failover so coverage survives one account's daily
// limit. Opened on first enable and via the card's edit icon.
export function FreeBuffKeysDialog({ open, onClose }) {
  const [tokens, setTokens] = useState([]);
  const [busy, setBusy] = useState(false);
  const [rawKeys, setRawKeys] = useState('');
  const [err, setErr] = useState('');

  const load = useCallback(async () => {
    if (!isTauri()) return;
    try { setTokens(await invoke('freebuff_list_tokens')); } catch { /* non-fatal */ }
  }, []);
  useEffect(() => { if (open) load(); }, [open, load]);

  const run = async (fn) => {
    setBusy(true); setErr('');
    try { setTokens(await fn()); }
    catch (e) { setErr(prettyProviderError(e)); }
    finally { setBusy(false); }
  };
  const addPasted = async () => {
    if (!rawKeys.trim()) return;
    await run(() => invoke('freebuff_add_tokens', { raw: rawKeys }));
    setRawKeys('');
  };
  const addCurrent = () => run(() => invoke('freebuff_add_current_login'));
  const remove = (id) => run(() => invoke('freebuff_remove_token', { id }));

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">FreeBuff account keys</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          <p className="text-[12px] italic leading-snug text-muted-foreground">
            Paste one or more FreeBuff auth keys (comma or newline separated) to pool multiple
            accounts. When one hits its daily limit, FreeBuff automatically fails over to the next.
          </p>
          <textarea
            value={rawKeys}
            onChange={(e) => setRawKeys(e.target.value)}
            placeholder="key1, key2, key3…"
            rows={3}
            className="w-full resize-y rounded-md border border-border/60 bg-background px-2 py-1.5 text-[12px] outline-none focus:border-border"
          />
          <div className="flex items-center gap-2">
            <Button size="sm" className="h-7 gap-1 text-[12px]" disabled={busy || !rawKeys.trim()} onClick={addPasted}>
              {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Plus className="size-3.5" />}
              Add keys
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 text-[12px]"
              disabled={busy}
              onClick={addCurrent}
              title="Snapshot the current freebuff CLI login into the pool"
            >
              Add current CLI login
            </Button>
          </div>
          {tokens.length > 0 && (
            <ul className="space-y-1 border-t border-border/40 pt-3">
              {tokens.map((t) => (
                <li key={t.id} className="flex items-center gap-2 text-[12px]">
                  <span
                    className={cn('size-1.5 shrink-0 rounded-full', t.valid ? 'bg-emerald-500' : 'bg-destructive')}
                    title={t.valid ? 'Valid' : 'Revoked / invalid'}
                  />
                  <span className="flex-1 truncate">{t.email || t.id}</span>
                  {t.is_default && (
                    <span className="rounded bg-muted px-1 text-[9px] uppercase tracking-wide text-muted-foreground">live</span>
                  )}
                  {!t.valid && <span className="text-[10px] text-destructive">revoked</span>}
                  <button
                    type="button"
                    className="text-muted-foreground transition-colors hover:text-destructive"
                    onClick={() => remove(t.id)}
                    title="Remove from pool"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          {err && <div className="text-[11px] text-destructive break-all">{err}</div>}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button size="sm" onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function ProvidersSection() {
  const { aiConfig: config, refreshAiConfig } = useAiConfig();
  const [addOpen, setAddOpen] = useState(false);

  // Every provider add/edit/remove flows through this `refresh` (the cards'
  // `onSaved`/`onChanged`). Besides re-reading the config, drop the chat model
  // picker's cached `/v1/models` lists so newly-available models show up there
  // immediately — previously the only way to surface them was to remove and
  // re-add the provider.
  const refresh = useCallback(async () => {
    useLiveModels.getState().resetAll();
    await refreshAiConfig();
  }, [refreshAiConfig]);

  const byType = useMemo(() => {
    const map = {};
    (config?.providers || []).forEach((p) => {
      const key = p.provider_type;
      if (key === 'Compatible') {
        map.CompatibleList = map.CompatibleList || [];
        map.CompatibleList.push(p);
      } else {
        map[key] = p;
      }
    });
    return map;
  }, [config]);

  return (
    <Section
      title="AI Providers"
      defaultOpen
      actions={
        <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setAddOpen(true)} title="Add OpenAI-compatible">
          <Plus className="size-3.5" />
        </Button>
      }
    >
      <div className="space-y-2.5">
        {NATIVE_PROVIDERS.map((p) => (
          <ConnectCard
            key={p.type}
            provider={p}
            configured={byType[p.type]}
            onSaved={refresh}
          />
        ))}
        <FreeBuffCard configured={byType.FreeBuff} onSaved={refresh} />
        {(byType.CompatibleList || []).map((entry, i) => (
          <CompatibleEntryCard key={`${entry.name}-${i}`} entry={entry} onChanged={refresh} />
        ))}
      </div>
      <CompatibleAddDialog open={addOpen} onClose={() => setAddOpen(false)} onSaved={refresh} />
    </Section>
  );
}

