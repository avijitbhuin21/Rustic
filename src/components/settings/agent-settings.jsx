import React, { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
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

// "Is a backend reachable for invoke()?" — true for the Tauri desktop app AND
// for the web build (HTTP-backed by rustic-server). It is only false in a pure
// browser preview with no server. Despite the legacy name, these gates mean
// "do we have a backend to call", not "are we specifically desktop Tauri".
function isTauri() {
  return IS_WEB || (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window);
}

// Provider errors come back as `HTTP 401: {"error":{"message":"…"}}` (or a
// bare string). Pull out the human part so the user reads "Incorrect API key
// provided" instead of a wall of raw JSON.
function prettyProviderError(raw) {
  const s = String(raw || '').trim();
  const brace = s.indexOf('{');
  if (brace !== -1) {
    try {
      const obj = JSON.parse(s.slice(brace));
      const msg = obj?.error?.message || obj?.message || obj?.error;
      if (typeof msg === 'string' && msg.trim()) {
        const prefix = s.slice(0, brace).trim().replace(/:$/, '');
        return prefix ? `${prefix} — ${msg.trim()}` : msg.trim();
      }
    } catch { /* fall through to raw */ }
  }
  return s;
}

// Test an API key by hitting the provider's live model-list endpoint before we
// store it, so an invalid key surfaces the real server error at connect time
// instead of silently failing later in "View models". Returns null on success
// or a readable error string on failure. Pass the raw key (not the `__STORED__`
// sentinel) to validate a key the user just typed.
async function validateProviderKey({ providerType, apiKey, baseUrl }) {
  if (!isTauri()) return null; // nothing to reach in the browser preview
  try {
    await invoke('fetch_ai_models', {
      providerType,
      apiKey,
      baseUrl: baseUrl || null,
      forceRefresh: true,
      includeAll: false,
    });
    return null;
  } catch (e) {
    return prettyProviderError(e);
  }
}

// ─── Shared AI config ─────────────────────────────────────────────────────────
//
// Provider list lived in each section's local state, so adding a provider in
// ProvidersSection didn't propagate to ToolsSection / SubAgentSection until the
// settings panel was closed and reopened. Lifting it here lets every section
// share one snapshot and trigger refreshes across siblings.

const AiConfigContext = React.createContext({ aiConfig: null, refreshAiConfig: () => {} });

function AiConfigProvider({ children }) {
  const [aiConfig, setAiConfig] = useState(null);
  const refreshAiConfig = useCallback(async () => {
    if (!isTauri()) { setAiConfig({ providers: [] }); return; }
    try { setAiConfig(await invoke('get_ai_config')); }
    catch { setAiConfig({ providers: [] }); }
  }, []);
  useEffect(() => { refreshAiConfig(); }, [refreshAiConfig]);
  return <AiConfigContext.Provider value={{ aiConfig, refreshAiConfig }}>{children}</AiConfigContext.Provider>;
}

function useAiConfig() {
  return useContext(AiConfigContext);
}

// ─── Collapsible Section ──────────────────────────────────────────────────────

function Section({ title, defaultOpen = false, actions, badge, children }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className="mb-3 rounded-xl border border-border/60 bg-muted/10 overflow-hidden">
      <header
        className="flex h-11 cursor-pointer select-none items-center gap-2 px-3"
        onClick={() => setOpen((v) => !v)}
      >
        <ChevronRight
          className={cn(
            'size-3.5 text-muted-foreground transition-transform',
            open && 'rotate-90'
          )}
        />
        <span className="text-[13px] font-semibold tracking-tight">{title}</span>
        {badge && (
          <Badge variant="outline" className="h-5 text-[10px] uppercase border-border/70 text-muted-foreground">
            {badge}
          </Badge>
        )}
        <span className="flex-1" />
        {actions && (
          <div className="flex items-center gap-1.5" onClick={(e) => e.stopPropagation()}>
            {actions}
          </div>
        )}
      </header>
      {open && <div className="border-t border-border/40 px-4 py-3">{children}</div>}
    </section>
  );
}

// ─── AI Providers ─────────────────────────────────────────────────────────────

const NATIVE_PROVIDERS = [
  { type: 'Claude',   label: 'Anthropic',     defaultModel: 'claude-sonnet-4-5',  keyPlaceholder: 'sk-ant-…' },
  { type: 'OpenAi',   label: 'OpenAI',        defaultModel: 'gpt-5-mini',         keyPlaceholder: 'sk-…' },
  { type: 'Gemini',   label: 'Google Gemini', defaultModel: 'gemini-2.5-flash',   keyPlaceholder: 'AIza…' },
  { type: 'OpenRouter', label: 'OpenRouter',  defaultModel: 'openrouter/auto',    keyPlaceholder: 'sk-or-…' },
];
function ModelsDialog({ open, onClose, title, providerType, baseUrl }) {
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

function EditProviderDialog({ open, onClose, onSaved, providerType, providerLabel, entry, allowBaseUrl = false, allowName = false }) {
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

function ConnectCard({ provider, configured, onSaved }) {
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

function CompatibleAddDialog({ open, onClose, onSaved }) {
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

function CompatibleEntryCard({ entry, onChanged }) {
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

function slugify(name) {
  return (name || '').trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

// FreeBuff is a keyless native provider: the token comes from the local
// `freebuff` CLI login (`~/.config/manicode/credentials.json`), not a typed
// key. The card auto-detects that login and toggles the provider on/off rather
// than asking for credentials.
function FreeBuffCard({ configured, onSaved }) {
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
function FreeBuffKeysDialog({ open, onClose }) {
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
          <p className="text-[12px] leading-snug text-muted-foreground">
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

function ProvidersSection() {
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

// ─── Sub Agent ───────────────────────────────────────────────────────────────

function SubAgentSection() {
  const { aiConfig: config } = useAiConfig();
  const [providerKey, setProviderKey] = useState('');
  const [model, setModel] = useState('');
  const [capEnabled, setCapEnabled] = useState(true);
  const [cap, setCap] = useState(10);
  // Models fetched for the currently selected provider. null = not loaded yet,
  // [] = loaded but empty (or fetch failed), array = ready.
  const [providerModels, setProviderModels] = useState(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  // Track subagent + budget from the shared aiConfig snapshot. Providers come
  // from the shared context (kept fresh across sections); subagent/budget are
  // also part of that config so we just sync them locally on each refresh.
  useEffect(() => {
    if (!config) return;
    if (config.subagent) {
      setProviderKey(config.subagent.provider_key || '');
      setModel(config.subagent.model || '');
    } else {
      setProviderKey(''); setModel('');
    }
    const c = config.budget?.max_concurrent_subagents;
    if (c === null || c === undefined) { setCapEnabled(true); setCap(10); }
    else { setCapEnabled(true); setCap(c); }
  }, [config]);

  const { refreshAiConfig } = useAiConfig();
  const refresh = refreshAiConfig;

  const providers = (config?.providers || []).map((p) => {
    const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
    const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
    return {
      key,
      label,
      defaultModel: p.default_model,
      providerType: p.provider_type,
      baseUrl: p.base_url || null,
      binaryPath: p.binary_path || null,
    };
  });

  // Load the model list for whichever provider is currently selected.
  useEffect(() => {
    let cancelled = false;
    const found = providers.find((p) => p.key === providerKey);
    if (!isTauri() || !found) {
      setProviderModels(null);
      return () => { cancelled = true; };
    }
    setModelsLoading(true);
    (async () => {
      try {
        const list = await invoke('fetch_ai_models', {
          providerType: found.providerType,
          apiKey: '__STORED__',
          baseUrl: found.baseUrl || null,
          forceRefresh: false,
          includeAll: false,
        });
        if (!cancelled) setProviderModels(Array.isArray(list) ? list : []);
      } catch {
        if (!cancelled) setProviderModels([]);
      } finally {
        if (!cancelled) setModelsLoading(false);
      }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerKey, config]);

  const onPick = (key) => {
    setProviderKey(key);
    const found = providers.find((p) => p.key === key);
    // Pre-fill with the provider's default model so users who don't want to
    // dig through the dropdown still get a sane pick.
    setModel(found?.defaultModel || '');
  };

  const save = async () => {
    try {
      if (providerKey && model.trim()) {
        await invoke('set_subagent_config', { providerKey, model: model.trim() });
      } else {
        await invoke('clear_subagent_config');
      }
      await invoke('set_subagent_concurrency_cap', { cap: capEnabled ? Number(cap) : null });
      toast.success('Sub-agent saved');
      refresh();
    } catch (e) { toast.error(String(e)); }
  };

  const clearChoice = async () => {
    try {
      await invoke('clear_subagent_config');
      refresh();
    } catch (e) { toast.error(String(e)); }
  };

  return (
    <Section title="Sub Agent">
      <p className="mb-3 text-[12px] leading-snug text-muted-foreground">
        Pick a cheaper, faster model the agent can route mechanical sub-agent work to. When set, the main agent picks
        per-spawn whether the sub-agent runs on the main chat model (best for reasoning) or this one (best for bulk reads,
        simple edits, summarising). Leave unset to always reuse the main model.
      </p>
      <div className="flex items-center gap-2">
        <Select value={providerKey} onValueChange={onPick}>
          <SelectTrigger className="h-8 w-40 text-xs">
            <SelectValue placeholder="Pick a provider…" />
          </SelectTrigger>
          <SelectContent>
            {providers.length === 0 && (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">No providers configured.</div>
            )}
            {providers.map((p) => (
              <SelectItem key={p.key} value={p.key}>{p.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={model} onValueChange={setModel} disabled={!providerKey}>
          <SelectTrigger className="h-8 flex-1 text-xs">
            <SelectValue placeholder={
              !providerKey
                ? 'Pick a provider first'
                : modelsLoading
                  ? 'Loading models…'
                  : 'Pick a model…'
            } />
          </SelectTrigger>
          <SelectContent>
            {/* When the saved model isn't in the fetched list (e.g. fetch failed
                or the id was custom), surface it as the first option so the user
                doesn't see a blank trigger. */}
            {model && !(providerModels || []).includes(model) && (
              <SelectItem key={model} value={model}>{model}</SelectItem>
            )}
            {(providerModels || []).map((m) => (
              <SelectItem key={m} value={m}>{m}</SelectItem>
            ))}
            {providerKey && !modelsLoading && (providerModels || []).length === 0 && !model && (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">No models returned.</div>
            )}
          </SelectContent>
        </Select>
        {(providerKey || model) && (
          <Button size="icon-sm" variant="ghost" className="size-8 text-muted-foreground hover:text-destructive" onClick={clearChoice}>
            <Trash2 className="size-3.5" />
          </Button>
        )}
      </div>

      <div className="mt-5">
        <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-2">Concurrency</div>
        <div className="rounded-lg border border-border/40 bg-muted/20 px-3 py-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-medium">Cap parallel sub-agents per task</div>
              <div className="text-[12px] text-muted-foreground mt-0.5">
                How many <code className="text-[11px]">spawn_subagent</code> calls can run simultaneously under one parent
                task. Default 10. Uncheck to lift the cap entirely (rate-limit safety still comes from the global stream
                cap in the Budget panel).
              </div>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <Switch checked={capEnabled} onCheckedChange={setCapEnabled} />
              <Input
                type="number"
                min={1}
                max={64}
                value={cap}
                onChange={(e) => setCap(parseInt(e.target.value, 10) || 1)}
                disabled={!capEnabled}
                className="h-7 w-16 text-xs"
              />
              <span className="text-[11px] text-muted-foreground">sub-agents</span>
            </div>
          </div>
        </div>
      </div>

      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save}>Save</Button>
      </div>
    </Section>
  );
}

// ─── Audio Input ───────────────────────────────────────────────────────────────

function AudioInputSection() {
  const { aiConfig: config, refreshAiConfig } = useAiConfig();
  const [providerKey, setProviderKey] = useState('');
  const [model, setModel] = useState('');
  const [providerModels, setProviderModels] = useState(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  useEffect(() => {
    if (!config) return;
    if (config.audio_input) {
      setProviderKey(config.audio_input.provider_key || '');
      setModel(config.audio_input.model || '');
    } else {
      setProviderKey(''); setModel('');
    }
  }, [config]);

  // OpenAI / Compatible transcribe via /audio/transcriptions (Whisper),
  // Gemini via generateContent, OpenRouter via chat/completions input_audio —
  // the backend routes per provider type, so all four are selectable here.
  // (Anthropic has no transcription path and is intentionally excluded.)
  const providers = (config?.providers || [])
    .filter((p) => ['OpenAi', 'Compatible', 'Gemini', 'OpenRouter'].includes(p.provider_type))
    .map((p) => {
      const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
      const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
      return { key, label, providerType: p.provider_type, baseUrl: p.base_url || null };
    });

  // Load models with includeAll so transcription models (whisper-1,
  // gpt-4o-transcribe) — which the chat-only filter hides — are selectable.
  useEffect(() => {
    let cancelled = false;
    const found = providers.find((p) => p.key === providerKey);
    if (!isTauri() || !found) {
      setProviderModels(null);
      return () => { cancelled = true; };
    }
    setModelsLoading(true);
    (async () => {
      try {
        const list = await invoke('fetch_ai_models', {
          providerType: found.providerType,
          apiKey: '__STORED__',
          baseUrl: found.baseUrl || null,
          forceRefresh: false,
          includeAll: true,
        });
        if (!cancelled) setProviderModels(Array.isArray(list) ? list : []);
      } catch {
        if (!cancelled) setProviderModels([]);
      } finally {
        if (!cancelled) setModelsLoading(false);
      }
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerKey, config]);

  const save = async () => {
    try {
      if (providerKey && model.trim()) {
        await invoke('set_audio_input_config', { providerKey, model: model.trim() });
        toast.success('Audio input saved');
      } else {
        await invoke('clear_audio_input_config');
        toast.success('Audio input disabled');
      }
      refreshAiConfig();
      // Tell the prompt-box(es) to re-read the audio flag so the mic toggles
      // immediately — Settings is an in-app panel, so no window focus fires.
      window.dispatchEvent(new Event('audio-input-changed'));
    } catch (e) { toast.error(String(e)); }
  };

  const clearChoice = async () => {
    try {
      await invoke('clear_audio_input_config');
      setProviderKey(''); setModel('');
      refreshAiConfig();
      window.dispatchEvent(new Event('audio-input-changed'));
    } catch (e) { toast.error(String(e)); }
  };

  return (
    <Section title="Audio Input">
      <p className="mb-3 text-[12px] leading-snug text-muted-foreground">
        Pick a speech-to-text model and a mic button appears in the chat composer: click it (when the box is empty) to
        record, and your speech is transcribed straight into the prompt. Works with OpenAI &amp; OpenAI-compatible
        Whisper models (<code className="text-[11px]">gpt-4o-transcribe</code>, <code className="text-[11px]">whisper-1</code>),
        Gemini, and any audio-capable OpenRouter model — pick any model from those providers. Transcript quality and
        whether it streams word-by-word depend on the model. Leave unset to hide the mic.
      </p>
      <div className="flex items-center gap-2">
        <Select value={providerKey} onValueChange={(v) => { setProviderKey(v); setModel(''); }}>
          <SelectTrigger className="h-8 w-40 text-xs">
            <SelectValue placeholder="Pick a provider…" />
          </SelectTrigger>
          <SelectContent>
            {providers.length === 0 && (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">Connect an OpenAI, Gemini, OpenRouter or Compatible provider first.</div>
            )}
            {providers.map((p) => (
              <SelectItem key={p.key} value={p.key}>{p.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={model} onValueChange={setModel} disabled={!providerKey}>
          <SelectTrigger className="h-8 flex-1 text-xs">
            <SelectValue placeholder={
              !providerKey ? 'Pick a provider first' : modelsLoading ? 'Loading models…' : 'Pick a model…'
            } />
          </SelectTrigger>
          <SelectContent>
            {model && !(providerModels || []).includes(model) && (
              <SelectItem key={model} value={model}>{model}</SelectItem>
            )}
            {(providerModels || []).map((m) => (
              <SelectItem key={m} value={m}>{m}</SelectItem>
            ))}
            {providerKey && !modelsLoading && (providerModels || []).length === 0 && !model && (
              <div className="px-2 py-1.5 text-xs text-muted-foreground">No models returned.</div>
            )}
          </SelectContent>
        </Select>
        {(providerKey || model) && (
          <Button size="icon-sm" variant="ghost" className="size-8 text-muted-foreground hover:text-destructive" onClick={clearChoice}>
            <Trash2 className="size-3.5" />
          </Button>
        )}
      </div>

      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save}>Save</Button>
      </div>
    </Section>
  );
}

// ─── Budget ──────────────────────────────────────────────────────────────────

function BudgetSection() {
  const [streamsEnabled, setStreamsEnabled] = useState(true);
  const [streams, setStreams] = useState(6);
  const [ceilingEnabled, setCeilingEnabled] = useState(false);
  const [ceilingUsd, setCeilingUsd] = useState(20);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const b = await invoke('get_budget_settings');
      if (b.max_concurrent_streams === null || b.max_concurrent_streams === undefined) {
        setStreamsEnabled(false);
      } else {
        setStreamsEnabled(true);
        setStreams(b.max_concurrent_streams);
      }
      if (b.daily_cost_ceiling_cents === null || b.daily_cost_ceiling_cents === undefined) {
        setCeilingEnabled(false);
      } else {
        setCeilingEnabled(true);
        setCeilingUsd(Math.round(b.daily_cost_ceiling_cents / 100));
      }
    } catch {}
  };
  useEffect(() => { refresh(); }, []);

  const save = async () => {
    try {
      await invoke('set_budget_settings', {
        maxConcurrentStreams: streamsEnabled ? Number(streams) : null,
        dailyCostCeilingCents: ceilingEnabled ? Math.max(0, Math.round(Number(ceilingUsd) * 100)) : null,
      });
      toast.success('Budget saved');
    } catch (e) { toast.error(String(e)); }
  };

  return (
    <Section title="Budget">
      <p className="mb-3 text-[12px] leading-snug text-muted-foreground">
        Cross-task limits. Stop runaway parallelism or spend before it bites.
      </p>

      <div className="rounded-lg border border-border/40 bg-muted/20 divide-y divide-border/40">
        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="text-[13px] font-medium">Cap concurrent provider streams</div>
            <div className="text-[12px] text-muted-foreground mt-0.5">
              Parallel API calls across every task and their sub-agents. Default 6. Raise only if your provider's rate
              limit can handle it.
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <Switch checked={streamsEnabled} onCheckedChange={setStreamsEnabled} />
            <Input
              type="number" min={1} max={64} value={streams}
              onChange={(e) => setStreams(parseInt(e.target.value, 10) || 1)}
              disabled={!streamsEnabled}
              className="h-7 w-16 text-xs"
            />
            <span className="text-[11px] text-muted-foreground">streams</span>
          </div>
        </div>

        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="text-[13px] font-medium">Daily cost ceiling (native API)</div>
            <div className="text-[12px] text-muted-foreground mt-0.5">
              Stops new turns when today's native-API spend hits the cap. Resets at midnight UTC.
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <Switch checked={ceilingEnabled} onCheckedChange={setCeilingEnabled} />
            <Input
              type="number" min={0} value={ceilingUsd}
              onChange={(e) => setCeilingUsd(parseFloat(e.target.value) || 0)}
              disabled={!ceilingEnabled}
              className="h-7 w-20 text-xs"
            />
            <span className="text-[11px] text-muted-foreground">usd/day</span>
          </div>
        </div>
      </div>

      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save}>Save budget settings</Button>
      </div>
    </Section>
  );
}

// ─── Tools ───────────────────────────────────────────────────────────────────

const WEB_SEARCH_BACKENDS = ['Tavily', 'Brave', 'Mcp'];

function WebSearchDialog({ open, onClose, value, providers, onSave }) {
  const [backend, setBackend] = useState(value?.backend || 'Tavily');
  const [apiKey, setApiKey] = useState(value?.api_key || '');

  useEffect(() => {
    if (open) {
      setBackend(value?.backend || 'Tavily');
      setApiKey(value?.api_key || '');
    }
  }, [open, value]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[460px] sm:max-w-[460px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Web Search backend</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          <p className="text-[11px] text-muted-foreground leading-snug">
            Used when the active provider can't run web_search server-side (OpenAI Chat Completions, OpenAI-compatible, OpenRouter).
            Anthropic, Gemini, and GPT-5 already run it server-side — no key needed there.
          </p>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Backend</div>
            <Select value={backend} onValueChange={setBackend}>
              <SelectTrigger className="h-8 text-xs"><SelectValue /></SelectTrigger>
              <SelectContent>
                {WEB_SEARCH_BACKENDS.map((b) => <SelectItem key={b} value={b}>{b}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>
          {backend !== 'Mcp' && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">{backend} API key</div>
              <Input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                className="h-8 text-xs"
              />
            </div>
          )}
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={() => { onSave({ backend, api_key: apiKey }); onClose(); }}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function MediaToolDialog({ open, onClose, title, badge, hint, providers, maxLimit, value, onSave }) {
  const [providerKey, setProviderKey] = useState('');
  const [model, setModel] = useState('');
  const [maxPerCall, setMaxPerCall] = useState(1);
  // Provider's model catalog. null = not loaded, [] = loaded empty / fetch
  // failed, array = ready. Re-fetched whenever the selected provider changes.
  const [providerModels, setProviderModels] = useState(null);
  const [modelsLoading, setModelsLoading] = useState(false);

  useEffect(() => {
    if (open) {
      setProviderKey(value?.provider_key || '');
      setModel(value?.model || '');
      setMaxPerCall(value?.max_per_call || 1);
    }
  }, [open, value]);

  // Mirror the SubAgentSection model-loading pattern so the user picks from a
  // real model list instead of pasting a raw model id.
  useEffect(() => {
    let cancelled = false;
    const found = providers.find((p) => p.key === providerKey);
    if (!isTauri() || !found || !found.providerType) {
      setProviderModels(null);
      return () => { cancelled = true; };
    }
    setModelsLoading(true);
    (async () => {
      try {
        const list = await invoke('fetch_ai_models', {
          providerType: found.providerType,
          apiKey: '__STORED__',
          baseUrl: found.baseUrl || null,
          forceRefresh: false,
          // Media tools need image / video / audio model ids that the
          // chat-only filter would drop (e.g. Gemini VEO, Imagen, Nano Banana
          // variants that don't expose generateContent).
          includeAll: true,
        });
        if (!cancelled) setProviderModels(Array.isArray(list) ? list : []);
      } catch {
        if (!cancelled) setProviderModels([]);
      } finally {
        if (!cancelled) setModelsLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [providerKey, providers]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px] flex items-center gap-2">
            {title}
            <Badge variant="outline" className="h-5 text-[10px] font-mono">{badge}</Badge>
          </DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          {hint && <p className="text-[11px] text-muted-foreground leading-snug">{hint}</p>}
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Provider</div>
            <Select value={providerKey} onValueChange={(v) => { setProviderKey(v); setModel(''); }}>
              <SelectTrigger className="h-8 text-xs"><SelectValue placeholder="Pick a provider…" /></SelectTrigger>
              <SelectContent>
                {providers.length === 0 ? (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">No providers configured.</div>
                ) : providers.map((p) => (
                  <SelectItem key={p.key} value={p.key}>{p.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Model</div>
            <Select value={model} onValueChange={setModel} disabled={!providerKey}>
              <SelectTrigger className="h-8 text-xs">
                <SelectValue placeholder={
                  !providerKey
                    ? 'Pick a provider first'
                    : modelsLoading
                      ? 'Loading models…'
                      : 'Pick a model…'
                } />
              </SelectTrigger>
              <SelectContent>
                {/* Surface the saved model even when it isn't in the fetched
                    list (custom id, fetch failed) so the trigger isn't blank. */}
                {model && !(providerModels || []).includes(model) && (
                  <SelectItem key={model} value={model}>{model}</SelectItem>
                )}
                {(providerModels || []).map((m) => (
                  <SelectItem key={m} value={m}>{m}</SelectItem>
                ))}
                {providerKey && !modelsLoading && (providerModels || []).length === 0 && !model && (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">No models returned.</div>
                )}
              </SelectContent>
            </Select>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Max per call (1–{maxLimit})</div>
            <Input
              type="number" min={1} max={maxLimit}
              value={maxPerCall}
              onChange={(e) => setMaxPerCall(parseInt(e.target.value, 10) || 1)}
              disabled={!providerKey}
              className="h-8 text-xs w-24"
            />
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60 flex-row justify-between sm:justify-between">
          <Button
            variant="ghost" size="sm" className="text-xs text-muted-foreground hover:text-destructive"
            onClick={() => { onSave({ provider_key: '', model: '', max_per_call: 1 }); onClose(); }}
          >
            Disable tool
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
            <Button
              size="sm" className="text-xs"
              onClick={() => { onSave({ provider_key: providerKey, model: model.trim(), max_per_call: maxPerCall }); onClose(); }}
            >
              Save
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ToolRow({ name, enabled, summary, statusLabel, onToggle, onConfigure, configurable = true }) {
  return (
    <div className="flex items-center gap-3 px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-[13px] font-medium">{name}</span>
          {statusLabel && (
            <span className="text-[11px] text-muted-foreground">{statusLabel}</span>
          )}
        </div>
        {summary && <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-1">{summary}</div>}
      </div>
      {configurable && (
        <Button
          size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-foreground"
          onClick={onConfigure} title="Configure"
        >
          <Pencil className="size-3.5" />
        </Button>
      )}
      {onToggle && <Switch checked={enabled} onCheckedChange={onToggle} />}
    </div>
  );
}

function ToolsSection() {
  // Honor the one-shot deep-link from the prompt-box "Tool settings…" entry —
  // when settings was opened with section='tools', we render this section
  // already expanded. SettingsPanel clears the hint right after first mount,
  // so the user's manual collapses stick on subsequent visits.
  const [defaultOpen] = useState(
    () => useLayout.getState().settingsInitialSection === 'tools',
  );
  const wrapperRef = useRef(null);
  const { aiConfig } = useAiConfig();
  const [tool, setTool] = useState(null);
  const [wsOpen, setWsOpen] = useState(false);
  const [openMedia, setOpenMedia] = useState(null); // 'image' | 'video' | 'animate' | null

  // When opened via the deep-link, slide the section into view. The tab-switch
  // slideVariants animation takes ~200ms, so we wait a tick for layout to
  // settle before scrolling — otherwise the target's position is still being
  // animated and the scroll lands short.
  useEffect(() => {
    if (!defaultOpen) return;
    const t = setTimeout(() => {
      wrapperRef.current?.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }, 260);
    return () => clearTimeout(t);
  }, [defaultOpen]);

  const refreshTool = async () => {
    if (!isTauri()) return;
    try { setTool(await invoke('get_tool_config')); }
    catch {}
  };
  useEffect(() => { refreshTool(); }, []);

  const providers = (aiConfig?.providers || []).map((p) => {
    const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
    const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
    return {
      key,
      label,
      providerType: p.provider_type,
      baseUrl: p.base_url || null,
    };
  });

  const update = async (patch) => {
    if (!tool) return;
    const next = { ...tool, ...patch };
    setTool(next);
    try { await invoke('set_tool_config', { config: next }); }
    catch (e) { toast.error(String(e)); refreshTool(); }
  };

  if (!tool) return (
    <div ref={wrapperRef}>
      <Section title="Tools" defaultOpen={defaultOpen}>
        <div className="text-xs text-muted-foreground">Loading…</div>
      </Section>
    </div>
  );

  const ws = tool.web_search || { enabled: false, backend: 'Tavily', api_key: '' };
  const wf = tool.web_fetch || { enabled: true };
  const media = tool.media || { image: {}, video: {}, animate: {}, link_animate_to_video: false };

  const mediaStatus = (m) => m?.provider_key && m?.model ? `${m.provider_key} · ${m.model}` : 'Not configured';
  const animateEffective = media.link_animate_to_video ? media.video : media.animate;

  return (
    <div ref={wrapperRef}>
    <Section title="Tools" defaultOpen={defaultOpen}>
      <div className="rounded-lg border border-border/40 bg-muted/10 divide-y divide-border/40">
        <ToolRow
          name="Web Search"
          enabled={ws.enabled}
          summary={`Backend: ${ws.backend || 'Tavily'}${ws.backend !== 'Mcp' && !ws.api_key ? ' · no key' : ''}`}
          onToggle={(v) => update({ web_search: { ...ws, enabled: v } })}
          onConfigure={() => setWsOpen(true)}
        />
        <ToolRow
          name="Web Fetch"
          enabled={wf.enabled}
          summary="Fetch and summarize a URL"
          onToggle={(v) => update({ web_fetch: { enabled: v } })}
          configurable={false}
        />
        <ToolRow
          name="Image creator"
          statusLabel="image_create"
          summary={mediaStatus(media.image)}
          onConfigure={() => setOpenMedia('image')}
        />
        <ToolRow
          name="Video creator"
          statusLabel="video_create"
          summary={mediaStatus(media.video)}
          onConfigure={() => setOpenMedia('video')}
        />
        <div className="px-3 py-2.5">
          <div className="flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="text-[13px] font-medium">Animator</span>
                <span className="text-[11px] text-muted-foreground">animate</span>
              </div>
              <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-1">
                {media.link_animate_to_video
                  ? `Linked to Video creator · ${mediaStatus(media.video)}`
                  : mediaStatus(media.animate)}
              </div>
            </div>
            <label className="flex items-center gap-1.5 text-[11px] text-muted-foreground select-none cursor-pointer">
              <Switch
                checked={!!media.link_animate_to_video}
                onCheckedChange={(v) => update({ media: { ...media, link_animate_to_video: v } })}
              />
              link to video
            </label>
            {!media.link_animate_to_video && (
              <Button
                size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-foreground"
                onClick={() => setOpenMedia('animate')} title="Configure"
              >
                <Pencil className="size-3.5" />
              </Button>
            )}
          </div>
        </div>
      </div>

      <p className="mt-2 px-1 text-[11px] text-muted-foreground/80">
        Media outputs save under <code className="text-[11px]">.rustic/generated/</code>.
      </p>

      <WebSearchDialog
        open={wsOpen}
        onClose={() => setWsOpen(false)}
        value={ws}
        providers={providers}
        onSave={(v) => update({ web_search: { ...ws, ...v } })}
      />
      <MediaToolDialog
        open={openMedia === 'image'}
        onClose={() => setOpenMedia(null)}
        title="Image creator"
        badge="image_create"
        hint="Suggested: OpenAI gpt-image-1 · Gemini gemini-2.5-flash-image"
        providers={providers}
        maxLimit={10}
        value={media.image}
        onSave={(v) => update({ media: { ...media, image: v } })}
      />
      <MediaToolDialog
        open={openMedia === 'video'}
        onClose={() => setOpenMedia(null)}
        title="Video creator"
        badge="video_create"
        hint="Suggested: OpenAI sora-2 · Gemini veo-3.1-generate-preview. Veo 3.1 also enables first+last frame interpolation."
        providers={providers}
        maxLimit={4}
        value={media.video}
        onSave={(v) => update({ media: { ...media, video: v } })}
      />
      <MediaToolDialog
        open={openMedia === 'animate'}
        onClose={() => setOpenMedia(null)}
        title="Animator"
        badge="animate"
        hint="Animates an existing project image into a short clip. With Veo 3.1 you can also pass an end frame to interpolate between two images."
        providers={providers}
        maxLimit={4}
        value={media.animate}
        onSave={(v) => update({ media: { ...media, animate: v } })}
      />
    </Section>
    </div>
  );
}

// ─── MCP Servers ─────────────────────────────────────────────────────────────

// MCP servers are global by design — the user-scope `.mcp.json` is the single
// source of truth, applied across every project. The dialog used to support a
// project-scoped variant but that was removed to keep the UX simple: one set
// of servers, configured once.
function McpJsonDialog({ open, onClose }) {
  const [json, setJson] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open || !isTauri()) return;
    invoke('read_mcp_json', { scope: 'user', projectId: null })
      .then((t) => setJson(typeof t === 'string' ? t : ''))
      .catch(() => setJson('{\n  "mcpServers": {}\n}'));
  }, [open]);

  const save = async () => {
    try { JSON.parse(json); }
    catch { toast.error('Invalid JSON'); return; }
    setSaving(true);
    try {
      const results = await invoke('save_mcp_json', { scope: 'user', projectId: null, content: json });
      const failed = (results || []).filter((r) => !r.connected);
      if (failed.length === 0) toast.success('MCP saved');
      else toast.error(`Saved, but ${failed.length} server(s) failed to connect`);
      onClose();
    } catch (e) { toast.error(String(e)); }
    finally { setSaving(false); }
  };

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Edit mcp.json</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4">
          <Textarea
            value={json}
            onChange={(e) => setJson(e.target.value)}
            className="min-h-[320px] font-mono text-[11px] resize-none"
            spellCheck={false}
          />
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={saving}>
            {saving ? 'Saving…' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function McpServerRow({ server, onRemove }) {
  const [open, setOpen] = useState(false);
  const [tools, setTools] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const id = server.id || server.name;
  const st = server.status || { state: 'unknown' };
  const connected = st.state === 'connected';
  const label =
    connected ? `Connected · ${st.tool_count ?? 0} tool${st.tool_count === 1 ? '' : 's'}` :
    st.state === 'failed' ? 'Failed' :
    'Idle';
  const tone =
    connected ? 'border-emerald-500/40 text-emerald-500' :
    st.state === 'failed' ? 'border-rose-500/40 text-rose-500' :
    'border-border/60 text-muted-foreground';

  const toggle = async () => {
    const next = !open;
    setOpen(next);
    if (next && tools === null && connected) {
      setLoading(true); setError('');
      try {
        const list = await invoke('list_mcp_server_tools', { id });
        setTools(Array.isArray(list) ? list : []);
      } catch (e) {
        setError(String(e));
        setTools([]);
      } finally { setLoading(false); }
    }
  };

  return (
    <li className="rounded-md border border-border/50 bg-muted/30 overflow-hidden">
      <div
        className={cn(
          'flex items-center gap-2 px-3 py-2',
          connected && 'cursor-pointer hover:bg-muted/50'
        )}
        onClick={() => connected && toggle()}
      >
        {connected ? (
          <ChevronRight className={cn('size-3.5 text-muted-foreground transition-transform shrink-0', open && 'rotate-90')} />
        ) : (
          <span className="w-3.5 shrink-0" />
        )}
        <span className="text-[12px] font-mono flex-1 truncate">{server.name || id}</span>
        <Badge variant="outline" className={cn('h-5 text-[10px]', tone)}>{label}</Badge>
        <Button
          size="icon-sm" variant="ghost" className="size-7 text-muted-foreground hover:text-destructive"
          onClick={(e) => { e.stopPropagation(); onRemove(id); }}
        >
          <Trash2 className="size-3.5" />
        </Button>
      </div>
      {st.state === 'failed' && st.error && (
        <p className="px-3 pb-2 text-[11px] text-rose-500/90 break-all">{st.error}</p>
      )}
      {open && connected && (
        <div className="border-t border-border/40 bg-muted/10">
          {loading ? (
            <div className="px-3 py-2 text-[11px] text-muted-foreground">Loading tools…</div>
          ) : error ? (
            <div className="px-3 py-2 text-[11px] text-destructive break-all">{error}</div>
          ) : (tools || []).length === 0 ? (
            <div className="px-3 py-2 text-[11px] text-muted-foreground">No tools advertised.</div>
          ) : (
            <ul className="divide-y divide-border/30">
              {tools.map((t) => (
                <li key={t.name} className="px-3 py-2">
                  <div className="text-[12px] font-mono text-foreground/90">{t.name}</div>
                  {t.description && (
                    <div className="mt-0.5 text-[11px] text-muted-foreground leading-snug whitespace-pre-wrap">
                      {t.description}
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </li>
  );
}

function McpSection() {
  const [servers, setServers] = useState([]);
  const [jsonOpen, setJsonOpen] = useState(false);

  // MCP servers are configured once at the user level and apply across all
  // projects — no per-project scoping. We pass projectId: null so the backend
  // always returns / writes the user-level server list.
  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_mcp_servers', { projectId: null });
      setServers(Array.isArray(list) ? list : []);
    } catch { setServers([]); }
  };
  useEffect(() => { refresh(); }, []);

  const remove = async (id) => {
    try { await invoke('remove_mcp_server', { id }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="MCP Servers"
      badge="Global"
      actions={
        <>
          <Button size="sm" variant="outline" className="h-7 text-xs gap-1.5" onClick={() => setJsonOpen(true)}>
            <ClipboardEdit className="size-3" /> Edit JSON
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setJsonOpen(true)}>
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {servers.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">
          No MCP servers configured.<br />
          Click "Edit JSON" to add one. Standard <code className="text-[11px]">.mcp.json</code> format.
        </div>
      ) : (
        <ul className="space-y-1.5">
          {servers.map((s) => (
            <McpServerRow key={s.id || s.name} server={s} onRemove={remove} />
          ))}
        </ul>
      )}
      <McpJsonDialog open={jsonOpen} onClose={() => { setJsonOpen(false); refresh(); }} />
    </Section>
  );
}

// ─── Skills ──────────────────────────────────────────────────────────────────

function MarkdownEditDialog({ open, title, name: initialName, body: initialBody, onClose, onSave, allowRename = true }) {
  const [name, setName] = useState(initialName || '');
  const [body, setBody] = useState(initialBody || '');
  const [saving, setSaving] = useState(false);

  useEffect(() => { if (open) { setName(initialName || ''); setBody(initialBody || ''); } }, [open, initialName, initialBody]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">{title}</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-3">
          {allowRename && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Name</div>
              <Input value={name} onChange={(e) => setName(e.target.value)} className="h-8 text-xs" />
            </div>
          )}
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground/70 mb-1">Body</div>
            <Textarea
              value={body}
              onChange={(e) => setBody(e.target.value)}
              className="min-h-[280px] font-mono text-[11px] resize-none"
              spellCheck={false}
            />
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" disabled={saving || !name.trim()} onClick={async () => {
            setSaving(true);
            try { await onSave({ name: name.trim(), body }); onClose(); }
            catch (e) { toast.error(String(e)); }
            finally { setSaving(false); }
          }}>
            {saving ? 'Saving…' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function MarkdownEntryRow({ entry, onPreview, onEdit, onCopy, onDelete, badge }) {
  return (
    <div className="px-3 py-2.5 hover:bg-muted/30 transition-colors group">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="text-[13px] font-medium">{entry.name}</span>
            {badge}
          </div>
          {entry.description && (
            <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-2">{entry.description}</div>
          )}
        </div>
        <div className="flex items-center gap-1 opacity-60 group-hover:opacity-100">
          <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={onPreview} title="Preview">
            <Eye className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={onEdit} title="Edit">
            <Pencil className="size-3.5" />
          </Button>
          {onCopy && (
            <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={onCopy} title="Copy name">
              <Copy className="size-3.5" />
            </Button>
          )}
          <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive" onClick={onDelete} title="Delete">
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      </div>
    </div>
  );
}

function PreviewDialog({ open, title, body, onClose }) {
  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[640px] sm:max-w-[640px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">{title}</DialogTitle>
        </DialogHeader>
        <ScrollArea className="max-h-[60vh]">
          <pre className="whitespace-pre-wrap px-5 py-4 text-[12px] font-mono">{body || '(empty)'}</pre>
        </ScrollArea>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button size="sm" className="text-xs" onClick={onClose}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function SkillsSection() {
  const [items, setItems] = useState([]);
  const [edit, setEdit] = useState(null); // {name, body} or {name: '', body: ''} for new
  const [preview, setPreview] = useState(null);
  const [info, setInfo] = useState(false);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_skills');
      setItems(Array.isArray(list) ? list : []);
    } catch { setItems([]); }
  };
  useEffect(() => { refresh(); }, []);

  const openPreview = async (name) => {
    try {
      const body = await invoke('get_skill_body', { name });
      setPreview({ title: name, body });
    } catch (e) { toast.error(String(e)); }
  };
  const openEdit = async (name) => {
    try {
      const body = await invoke('get_skill_body', { name });
      setEdit({ originalName: name, name, body });
    } catch (e) { toast.error(String(e)); }
  };

  const save = async ({ name, body }) => {
    if (edit?.originalName) {
      await invoke('update_skill', { originalName: edit.originalName, name, body });
    } else {
      await invoke('create_skill', { name, body });
    }
    refresh();
  };

  const remove = async (name) => {
    try { await invoke('delete_skill', { name }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="Skills"
      badge="Global"
      actions={
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" onClick={() => setInfo(true)} title="About skills">
            <Info className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setEdit({ originalName: null, name: '', body: '' })} title="Add skill">
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {items.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">No skills installed.</div>
      ) : (
        <div className="divide-y divide-border/40 rounded-md border border-border/40 bg-muted/10">
          {items.map((s) => (
            <MarkdownEntryRow
              key={s.name}
              entry={s}
              onPreview={() => openPreview(s.name)}
              onEdit={() => openEdit(s.name)}
              onCopy={() => { navigator.clipboard.writeText(s.name); toast.success('Copied'); }}
              onDelete={() => remove(s.name)}
            />
          ))}
        </div>
      )}
      <MarkdownEditDialog
        open={!!edit}
        title={edit?.originalName ? `Edit "${edit.originalName}"` : 'New skill'}
        name={edit?.name || ''}
        body={edit?.body || ''}
        onClose={() => setEdit(null)}
        onSave={save}
      />
      <PreviewDialog open={!!preview} title={preview?.title || ''} body={preview?.body || ''} onClose={() => setPreview(null)} />
      <Dialog open={info} onOpenChange={(v) => !v && setInfo(false)}>
        <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px]">
          <DialogHeader><DialogTitle className="text-[14px]">About skills</DialogTitle></DialogHeader>
          <div className="text-[12px] text-muted-foreground space-y-2">
            <p>Skills are reusable instructions the agent can opt into per-task — e.g. "follow brand guidelines", "use the canvas-design conventions".</p>
            <p>Stored as Markdown files under your global Rustic skills directory. The agent sees the title + description in its system prompt and can decide when to use them.</p>
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

// ─── Workflows ───────────────────────────────────────────────────────────────

function WorkflowsSection() {
  const [items, setItems] = useState([]);
  const [edit, setEdit] = useState(null);
  const [preview, setPreview] = useState(null);
  const [info, setInfo] = useState(false);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_workflows');
      setItems(Array.isArray(list) ? list : []);
    } catch { setItems([]); }
  };
  useEffect(() => { refresh(); }, []);

  const openPreview = async (name) => {
    try { const body = await invoke('get_workflow_body', { name }); setPreview({ title: name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const openEdit = async (name) => {
    try { const body = await invoke('get_workflow_body', { name }); setEdit({ originalName: name, name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const save = async ({ name, body }) => {
    if (edit?.originalName) await invoke('update_workflow', { originalName: edit.originalName, name, body });
    else await invoke('create_workflow', { name, body });
    refresh();
  };
  const remove = async (name) => {
    try { await invoke('delete_workflow', { name }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };

  return (
    <Section
      title="Workflows"
      badge="Global"
      actions={
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" onClick={() => setInfo(true)} title="About workflows">
            <Info className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setEdit({ originalName: null, name: '', body: '' })} title="Add workflow">
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {items.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">No workflows installed.</div>
      ) : (
        <div className="divide-y divide-border/40 rounded-md border border-border/40 bg-muted/10">
          {items.map((w) => (
            <MarkdownEntryRow
              key={w.name}
              entry={w}
              onPreview={() => openPreview(w.name)}
              onEdit={() => openEdit(w.name)}
              onCopy={() => { navigator.clipboard.writeText(w.name); toast.success('Copied'); }}
              onDelete={() => remove(w.name)}
            />
          ))}
        </div>
      )}
      <MarkdownEditDialog
        open={!!edit}
        title={edit?.originalName ? `Edit "${edit.originalName}"` : 'New workflow'}
        name={edit?.name || ''}
        body={edit?.body || ''}
        onClose={() => setEdit(null)}
        onSave={save}
      />
      <PreviewDialog open={!!preview} title={preview?.title || ''} body={preview?.body || ''} onClose={() => setPreview(null)} />
      <Dialog open={info} onOpenChange={(v) => !v && setInfo(false)}>
        <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px]">
          <DialogHeader><DialogTitle className="text-[14px]">About workflows</DialogTitle></DialogHeader>
          <div className="text-[12px] text-muted-foreground space-y-2">
            <p>Workflows are multi-step procedures the agent invokes by name (e.g. "landing-page-cloning-workflow"). They expand into a recipe the agent then executes.</p>
            <p>Stored as Markdown files under your global Rustic workflows directory.</p>
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

// ─── Rules ───────────────────────────────────────────────────────────────────

// Forward-slash + lowercase-drive normalisation so we can match a
// `Project.root_path` (which may use backslashes on Windows) against the
// `active_projects` keys returned by the backend (`project_key` uses
// forward slashes).
function normaliseProjectKey(p) {
  if (!p) return '';
  let s = String(p).replace(/\\/g, '/');
  // Lowercase the drive letter on Windows-style paths: `D:/foo` → `d:/foo`,
  // matching the canonical form the rule store uses.
  if (/^[A-Za-z]:\//.test(s)) {
    s = s[0].toLowerCase() + s.slice(1);
  }
  return s;
}

function RuleStatePicker({ value, projectCount, onPick }) {
  const opts = [
    { v: 'inactive', label: 'Off' },
    { v: 'global',   label: 'G'   },
    {
      v: 'project',
      // Show count when the rule is project-scoped in 2+ places so the
      // user knows the picker isn't just "current project only".
      label: projectCount > 0 ? `P · ${projectCount}` : 'P',
    },
  ];
  return (
    <div className="inline-flex rounded-md border border-border/60 bg-muted/30 p-0.5">
      {opts.map((o) => (
        <button
          key={o.v}
          type="button"
          onClick={() => onPick(o.v)}
          className={cn(
            'h-6 px-2 text-[11px] font-medium rounded-sm transition-colors',
            value === o.v
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:text-foreground'
          )}
          title={o.v === 'project' ? 'Pick project(s) where this rule applies' : o.v}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function RuleProjectPickerDialog({ open, onClose, ruleName, initialSelected, onSaved }) {
  const [projects, setProjects] = useState([]);
  const [selected, setSelected] = useState(new Set());
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open || !isTauri()) return;
    invoke('list_projects')
      .then((list) => setProjects(Array.isArray(list) ? list : []))
      .catch(() => setProjects([]));
    // Pre-fill from the backend's stored active_projects (already normalised
    // to forward-slash). Match by normalised root path so the same project
    // matches regardless of slash direction.
    const init = new Set();
    (initialSelected || []).forEach((p) => init.add(normaliseProjectKey(p)));
    setSelected(init);
  }, [open, initialSelected]);

  const toggle = (rootKey) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(rootKey)) next.delete(rootKey);
      else next.add(rootKey);
      return next;
    });
  };

  const save = async () => {
    setSaving(true);
    try {
      // Send the original (un-normalised) root paths for the selected
      // projects — the backend re-normalises with its own `project_key`.
      const picked = projects
        .filter((p) => selected.has(normaliseProjectKey(p.root_path)))
        .map((p) => p.root_path);
      await invoke('set_rule_projects', { name: ruleName, projectRoots: picked });
      onSaved?.();
      onClose();
    } catch (e) { toast.error(String(e)); }
    finally { setSaving(false); }
  };

  // Show project keys we have on file but no longer correspond to a known
  // project (project was deleted but rule still references it). Surface
  // them as read-only rows with a small note so the user can clear them.
  const knownKeys = new Set(projects.map((p) => normaliseProjectKey(p.root_path)));
  const orphans = Array.from(selected).filter((k) => !knownKeys.has(k));

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent aria-describedby={undefined} className="w-[520px] sm:max-w-[520px] p-0 gap-0">
        <DialogHeader className="px-5 pt-5 pb-3 border-b border-border/60">
          <DialogTitle className="text-[14px]">Projects for "{ruleName}"</DialogTitle>
        </DialogHeader>
        <div className="px-5 py-4 space-y-2">
          <p className="text-[11px] text-muted-foreground leading-snug">
            Tick the projects where this rule should apply. Selecting more than one is fine. Saving with nothing
            ticked deactivates the rule everywhere.
          </p>
          <div className="rounded-md border border-border/40 divide-y divide-border/40 max-h-72 overflow-y-auto">
            {projects.length === 0 ? (
              <div className="px-3 py-3 text-[11px] text-muted-foreground">No projects in your workspace yet.</div>
            ) : projects.map((p) => {
              const key = normaliseProjectKey(p.root_path);
              const checked = selected.has(key);
              return (
                <label
                  key={p.id}
                  className="flex cursor-pointer items-center gap-2.5 px-3 py-2 hover:bg-muted/40"
                  onClick={(e) => { e.preventDefault(); toggle(key); }}
                >
                  <div className={cn(
                    'flex size-4 shrink-0 items-center justify-center rounded-sm border transition-colors',
                    checked
                      ? 'border-primary bg-primary text-primary-foreground'
                      : 'border-border bg-transparent'
                  )}>
                    {checked && <Check className="size-3" strokeWidth={3} />}
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="text-[12.5px] font-medium truncate">{p.name}</div>
                    <div className="text-[11px] text-muted-foreground truncate font-mono">{p.root_path}</div>
                  </div>
                </label>
              );
            })}
            {orphans.map((k) => (
              <div key={k} className="flex items-center gap-2.5 px-3 py-2 bg-muted/10">
                <div className="size-4 shrink-0 rounded-sm border border-rose-500/40 bg-rose-500/10" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] text-rose-500/90 font-mono truncate">{k}</div>
                  <div className="text-[10.5px] text-muted-foreground">Project no longer exists — will be cleared on save.</div>
                </div>
                <Button
                  size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive"
                  onClick={() => setSelected((prev) => {
                    const next = new Set(prev);
                    next.delete(k);
                    return next;
                  })}
                >
                  <X className="size-3.5" />
                </Button>
              </div>
            ))}
          </div>
        </div>
        <DialogFooter className="mx-0 mb-0 px-5 py-3 border-t border-border/60">
          <Button variant="outline" size="sm" className="text-xs" onClick={onClose}>Cancel</Button>
          <Button size="sm" className="text-xs" onClick={save} disabled={saving}>
            {saving ? 'Saving…' : `Apply to ${selected.size} project${selected.size === 1 ? '' : 's'}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RulesSection() {
  const projectRoot = useAgent((s) => s.activeProject.root || null);
  const [items, setItems] = useState([]);
  const [edit, setEdit] = useState(null);
  const [preview, setPreview] = useState(null);
  const [info, setInfo] = useState(false);
  // Rule currently being edited in the project-picker dialog.
  const [projectPicker, setProjectPicker] = useState(null);

  const refresh = async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_rules', { projectRoot: projectRoot || null });
      setItems(Array.isArray(list) ? list : []);
    } catch { setItems([]); }
  };
  useEffect(() => { refresh(); }, [projectRoot]);

  const openPreview = async (name) => {
    try { const body = await invoke('get_rule_body', { name }); setPreview({ title: name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const openEdit = async (name) => {
    try { const body = await invoke('get_rule_body', { name }); setEdit({ originalName: name, name, body }); }
    catch (e) { toast.error(String(e)); }
  };
  const save = async ({ name, body }) => {
    if (edit?.originalName) await invoke('update_rule', { originalName: edit.originalName, name, body });
    else await invoke('create_rule', { name, body });
    refresh();
  };
  const remove = async (name) => {
    try { await invoke('delete_rule', { name }); refresh(); }
    catch (e) { toast.error(String(e)); }
  };
  // Off / Global flip directly; Project pops the multi-select dialog so the
  // user can choose which projects this rule applies to.
  const onPickState = (rule, next) => {
    if (next === 'project') {
      setProjectPicker({ name: rule.name, initial: rule.active_projects || [] });
      return;
    }
    invoke('set_rule_activation', { name: rule.name, state: next, projectRoot: projectRoot || null })
      .then(refresh)
      .catch((e) => toast.error(String(e)));
  };

  return (
    <Section
      title="Rules"
      actions={
        <>
          <Button size="icon-sm" variant="ghost" className="size-7 text-muted-foreground" onClick={() => setInfo(true)} title="About rules">
            <Info className="size-3.5" />
          </Button>
          <Button size="icon-sm" variant="ghost" className="size-7" onClick={() => setEdit({ originalName: null, name: '', body: '' })} title="Add rule">
            <Plus className="size-3.5" />
          </Button>
        </>
      }
    >
      {items.length === 0 ? (
        <div className="text-[12px] text-muted-foreground">No rules.</div>
      ) : (
        <div className="divide-y divide-border/40 rounded-md border border-border/40 bg-muted/10">
          {items.map((r) => (
            <div key={r.name} className="px-3 py-2.5 hover:bg-muted/30 group">
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1.5">
                    <span className="text-[13px] font-medium">{r.name}</span>
                    {r.state === 'global'  && <Badge variant="outline" className="h-5 text-[10px] uppercase border-primary/50 text-primary">Global</Badge>}
                    {r.state === 'project' && <Badge variant="outline" className="h-5 text-[10px] uppercase border-amber-500/50 text-amber-500">Project</Badge>}
                  </div>
                  {r.description && (
                    <div className="mt-0.5 text-[11px] text-muted-foreground line-clamp-2">{r.description}</div>
                  )}
                </div>
                <div className="flex items-center gap-2 opacity-80 group-hover:opacity-100">
                  <RuleStatePicker
                    value={r.state}
                    projectCount={(r.active_projects || []).length}
                    onPick={(v) => onPickState(r, v)}
                  />
                  <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={() => openPreview(r.name)} title="Preview">
                    <Eye className="size-3.5" />
                  </Button>
                  <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-foreground" onClick={() => openEdit(r.name)} title="Edit">
                    <Pencil className="size-3.5" />
                  </Button>
                  <Button size="icon-sm" variant="ghost" className="size-6 text-muted-foreground hover:text-destructive" onClick={() => remove(r.name)} title="Delete">
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
      <MarkdownEditDialog
        open={!!edit}
        title={edit?.originalName ? `Edit "${edit.originalName}"` : 'New rule'}
        name={edit?.name || ''}
        body={edit?.body || ''}
        onClose={() => setEdit(null)}
        onSave={save}
      />
      <PreviewDialog open={!!preview} title={preview?.title || ''} body={preview?.body || ''} onClose={() => setPreview(null)} />
      <RuleProjectPickerDialog
        open={!!projectPicker}
        ruleName={projectPicker?.name || ''}
        initialSelected={projectPicker?.initial || []}
        onClose={() => setProjectPicker(null)}
        onSaved={refresh}
      />
      <Dialog open={info} onOpenChange={(v) => !v && setInfo(false)}>
        <DialogContent aria-describedby={undefined} className="w-[480px] sm:max-w-[480px]">
          <DialogHeader><DialogTitle className="text-[14px]">About rules</DialogTitle></DialogHeader>
          <div className="text-[12px] text-muted-foreground space-y-2">
            <p>Rules are always-on instructions the agent honors during a chat — e.g. "no unnecessary comments", "always run tests after edits".</p>
            <p><span className="font-medium">Off</span> = not active. <span className="font-medium">G</span> = global, active in every project. <span className="font-medium">P</span> = active only in the current project.</p>
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

// ─── GitHub auto issue resolve (web/server build only) ──────────────────────

function GithubAutoResolveSection() {
  const { aiConfig } = useAiConfig();
  const projects = useExplorer((s) => s.projects);
  const [cfg, setCfg] = useState(null); // { enabled, publicBaseUrl, label }
  const [signedIn, setSignedIn] = useState(false);
  const [projectId, setProjectId] = useState(null);
  const [projCfg, setProjCfg] = useState(null);
  const [detectedRepo, setDetectedRepo] = useState(null);
  const [savingProject, setSavingProject] = useState(false);

  const refreshGlobal = useCallback(async () => {
    try {
      const r = await invoke('github_auto_get_config');
      setCfg(r.config);
      setSignedIn(!!r.signedIn);
    } catch { /* server route missing — leave section in loading state */ }
  }, []);
  useEffect(() => { refreshGlobal(); }, [refreshGlobal]);

  useEffect(() => {
    if (!projectId) { setProjCfg(null); setDetectedRepo(null); return; }
    let active = true;
    invoke('github_auto_get_project_config', { projectId })
      .then((r) => { if (active) { setProjCfg(r.config); setDetectedRepo(r.detectedRepo); } })
      .catch((e) => { if (active) { setProjCfg(null); toast.error(String(e)); } });
    return () => { active = false; };
  }, [projectId]);

  const saveGlobal = async (next) => {
    try {
      const saved = await invoke('github_auto_set_config', {
        enabled: next.enabled,
        publicBaseUrl: next.publicBaseUrl ?? '',
        label: next.label || 'rustic',
      });
      setCfg(saved);
      toast.success('GitHub auto-resolve settings saved');
    } catch (e) { toast.error(String(e)); refreshGlobal(); }
  };

  const saveProject = async (next) => {
    if (!projectId) return;
    setSavingProject(true);
    try {
      const saved = await invoke('github_auto_set_project_config', {
        projectId,
        enabled: next.enabled,
        costCapUsd: next.costCapUsd ?? null,
        model: next.model ?? null,
        providerType: next.providerType ?? null,
      });
      setProjCfg(saved);
      if (next.enabled && !projCfg?.enabled) {
        toast.success(`Auto-resolve enabled — webhook created on ${saved.repoFullName || 'the repo'}`);
      } else {
        toast.success('Project settings saved');
      }
    } catch (e) { toast.error(String(e)); }
    finally { setSavingProject(false); }
  };

  const providers = (aiConfig?.providers || []).map((p) => {
    const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
    const label = p.name ? `${p.provider_type} — ${p.name}` : p.provider_type;
    return { key, label, providerType: p.provider_type, baseUrl: p.base_url || null };
  });

  // Live model lists for the issue-task model dropdown — same cache the chat
  // model picker uses (backend caches /v1/models for 5 min on top).
  const liveByKey = useLiveModels((s) => s.byKey);
  const loadLive = useLiveModels((s) => s.load);
  useEffect(() => {
    if (!projectId || !aiConfig) return;
    for (const p of aiConfig.providers || []) {
      const key = p.name ? `Compatible:${slugify(p.name)}` : p.provider_type;
      loadLive({ key, providerType: p.provider_type, baseUrl: p.base_url || null });
    }
  }, [projectId, aiConfig, loadLive]);

  if (!cfg) {
    return (
      <Section title="GitHub Auto-Resolve" badge="server">
        <div className="text-xs text-muted-foreground">Loading…</div>
      </Section>
    );
  }

  return (
    <Section title="GitHub Auto-Resolve" badge="server">
      <p className="mb-3 text-[12px] leading-snug text-muted-foreground">
        Issues labeled <span className="font-mono">{cfg.label || 'rustic'}</span> on connected
        repos are pulled into <span className="font-mono">issues/</span>, fixed by a dedicated
        agent task (queued one at a time), and committed locally — never pushed. Clarifying
        questions go back and forth as issue comments.
      </p>

      {!signedIn && (
        <div className="mb-3 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[12px] text-amber-600 dark:text-amber-400">
          Sign in to GitHub (status bar, bottom left) first — the integration reuses that account
          to read issues, post comments and create webhooks.
        </div>
      )}

      <div className="rounded-lg border border-border/40 bg-muted/20 divide-y divide-border/40">
        <div className="flex items-start justify-between gap-3 px-3 py-3">
          <div className="min-w-0">
            <div className="text-[13px] font-medium">Auto issue resolve</div>
            <div className="text-[12px] text-muted-foreground mt-0.5">
              Master switch. Off = webhooks are ignored and the queue pauses.
            </div>
          </div>
          <Switch
            checked={!!cfg.enabled}
            onCheckedChange={(v) => saveGlobal({ ...cfg, enabled: v })}
          />
        </div>

        <div className="px-3 py-3">
          <div className="text-[13px] font-medium">Public server URL</div>
          <div className="text-[12px] text-muted-foreground mt-0.5 mb-2">
            Where GitHub delivers webhooks, e.g. <span className="font-mono">https://rustic.example.com</span>.
          </div>
          <div className="flex items-center gap-2">
            <Input
              value={cfg.publicBaseUrl || ''}
              onChange={(e) => setCfg({ ...cfg, publicBaseUrl: e.target.value })}
              placeholder="https://your-server.example.com"
              className="h-7 flex-1 text-xs font-mono"
            />
            <Input
              value={cfg.label || ''}
              onChange={(e) => setCfg({ ...cfg, label: e.target.value })}
              placeholder="rustic"
              title="Only issues with this label are processed"
              className="h-7 w-28 text-xs font-mono"
            />
            <Button size="sm" className="text-xs" onClick={() => saveGlobal(cfg)}>Save</Button>
          </div>
        </div>

        <div className="px-3 py-3">
          <div className="text-[13px] font-medium mb-2">Per-project</div>
          <Select value={projectId ?? ''} onValueChange={setProjectId}>
            <SelectTrigger className="h-7 w-full text-xs">
              <SelectValue placeholder="Pick a project…" />
            </SelectTrigger>
            <SelectContent>
              {projects.map((p) => (
                <SelectItem key={p.id} value={p.id} className="text-xs">{p.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>

          {projectId && projCfg && (
            <div className="mt-3 space-y-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-[12px] font-medium">
                    Enable for this project
                    {detectedRepo && (
                      <span className="ml-2 font-mono text-[11px] text-muted-foreground">{detectedRepo}</span>
                    )}
                  </div>
                  <div className="text-[11px] text-muted-foreground mt-0.5">
                    Enabling creates the repo webhook automatically (needs the public URL above).
                  </div>
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  {savingProject && <Loader2 className="size-3 animate-spin text-muted-foreground" />}
                  <Switch
                    checked={!!projCfg.enabled}
                    disabled={savingProject || (!detectedRepo && !projCfg.enabled)}
                    onCheckedChange={(v) => saveProject({ ...projCfg, enabled: v })}
                  />
                </div>
              </div>

              <div className="flex items-center gap-2">
                <span className="text-[12px] text-muted-foreground w-28 shrink-0">Cost cap per issue</span>
                <Input
                  type="number" min={0} step="0.5"
                  value={projCfg.costCapUsd ?? ''}
                  placeholder="uncapped"
                  onChange={(e) => setProjCfg({
                    ...projCfg,
                    costCapUsd: e.target.value === '' ? null : parseFloat(e.target.value),
                  })}
                  className="h-7 w-24 text-xs"
                />
                <span className="text-[11px] text-muted-foreground">
                  USD — each issue's fixer task may spend up to this, not the project as a whole
                </span>
              </div>

              <div className="flex items-center gap-2">
                <span className="text-[12px] text-muted-foreground w-28 shrink-0">Issue-task model</span>
                <Select
                  value={
                    projCfg.providerType && projCfg.model
                      ? `${projCfg.providerType}::${projCfg.model}`
                      : '__default__'
                  }
                  onValueChange={(v) => {
                    if (v === '__default__') {
                      setProjCfg({ ...projCfg, providerType: null, model: null });
                    } else {
                      const sep = v.indexOf('::');
                      setProjCfg({
                        ...projCfg,
                        providerType: v.slice(0, sep),
                        model: v.slice(sep + 2),
                      });
                    }
                  }}
                >
                  <SelectTrigger className="h-7 flex-1 text-xs">
                    <SelectValue placeholder="Project default" />
                  </SelectTrigger>
                  <SelectContent className="max-h-72">
                    <SelectItem value="__default__" className="text-xs">Project default</SelectItem>
                    {/* Keep a previously-saved model selectable even when the
                        provider's live list no longer (or doesn't yet) contain it. */}
                    {projCfg.providerType && projCfg.model &&
                      !(liveByKey[projCfg.providerType] || []).some(
                        (m) => (m.id || m.model_id) === projCfg.model,
                      ) && (
                      <SelectItem
                        value={`${projCfg.providerType}::${projCfg.model}`}
                        className="text-xs font-mono"
                      >
                        {projCfg.model} (saved)
                      </SelectItem>
                    )}
                    {providers.map((p) => {
                      const models = liveByKey[p.key] || [];
                      if (models.length === 0) return null;
                      return (
                        <SelectGroup key={p.key}>
                          <SelectLabel className="text-[11px] text-muted-foreground">{p.label}</SelectLabel>
                          {models.map((m) => {
                            const id = m.id || m.model_id;
                            if (!id) return null;
                            return (
                              <SelectItem
                                key={`${p.key}::${id}`}
                                value={`${p.key}::${id}`}
                                className="text-xs font-mono"
                              >
                                {id}
                              </SelectItem>
                            );
                          })}
                        </SelectGroup>
                      );
                    })}
                  </SelectContent>
                </Select>
              </div>

              <div className="flex justify-end">
                <Button
                  size="sm" variant="outline" className="text-xs"
                  disabled={savingProject}
                  onClick={() => saveProject(projCfg)}
                >
                  Save project settings
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
    </Section>
  );
}

// ─── Root ────────────────────────────────────────────────────────────────────

export function AgentSettings() {
  return (
    <AiConfigProvider>
      <div className="space-y-0">
        <ProvidersSection />
        <SubAgentSection />
        <AudioInputSection />
        <BudgetSection />
        {IS_WEB && <GithubAutoResolveSection />}
        <ToolsSection />
        <McpSection />
        <SkillsSection />
        <WorkflowsSection />
        <RulesSection />
      </div>
    </AiConfigProvider>
  );
}

export default AgentSettings;
