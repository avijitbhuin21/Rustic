// Shared plumbing for the agent settings sections: backend gate, provider-key validation, AiConfig context, Section shell.
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
import { isTauriAvailable } from '@/lib/platform';
import { ProvidersSection } from './providers-section';
import { SubAgentSection } from './subagent-section';
import { ToolsSection } from './tools-section';

// "Is a backend reachable for invoke()?" — true for the Tauri desktop app AND
// for the web build (HTTP-backed by rustic-server). It is only false in a pure
// browser preview with no server. Despite the legacy name, these gates mean
// "do we have a backend to call", not "are we specifically desktop Tauri".
// Canonical definition lives in @/lib/platform; re-exported here because the
// settings sections import it from './shared'.
export const isTauri = isTauriAvailable;

// Provider errors come back as `HTTP 401: {"error":{"message":"…"}}` (or a
// bare string). Pull out the human part so the user reads "Incorrect API key
// provided" instead of a wall of raw JSON.
export function prettyProviderError(raw) {
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
export async function validateProviderKey({ providerType, apiKey, baseUrl }) {
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

export const AiConfigContext = React.createContext({ aiConfig: null, refreshAiConfig: () => {} });

export function AiConfigProvider({ children }) {
  const [aiConfig, setAiConfig] = useState(null);
  const refreshAiConfig = useCallback(async () => {
    if (!isTauri()) { setAiConfig({ providers: [] }); return; }
    try { setAiConfig(await invoke('get_ai_config')); }
    catch { setAiConfig({ providers: [] }); }
  }, []);
  useEffect(() => { refreshAiConfig(); }, [refreshAiConfig]);
  return <AiConfigContext.Provider value={{ aiConfig, refreshAiConfig }}>{children}</AiConfigContext.Provider>;
}

export function useAiConfig() {
  return useContext(AiConfigContext);
}

// ─── Collapsible Section ──────────────────────────────────────────────────────

// When true (set by the per-tab wrappers below), Sections render as always-
// open static cards instead of collapsible accordions.
export const FlatSectionsContext = createContext(false);

export function anchorSlug(title) {
  return String(title).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

export function Section({ title, defaultOpen = false, actions, badge, children }) {
  const flat = useContext(FlatSectionsContext);
  const [open, setOpen] = useState(defaultOpen);
  const expanded = flat || open;
  return (
    <section
      data-settings-anchor={anchorSlug(title)}
      className="mb-3 rounded-xl border border-border/60 bg-muted/10 overflow-hidden"
    >
      <header
        className={cn('flex h-11 select-none items-center gap-2 px-3', !flat && 'cursor-pointer')}
        onClick={flat ? undefined : () => setOpen((v) => !v)}
      >
        {!flat && (
          <ChevronRight
            className={cn(
              'size-3.5 text-muted-foreground transition-transform',
              open && 'rotate-90'
            )}
          />
        )}
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
      {expanded && <div className="border-t border-border/40 px-4 py-3">{children}</div>}
    </section>
  );
}


export function slugify(name) {
  return (name || '').trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}
