// Shortcuts settings panel — VS Code-style command list with searchable
// rows, click-to-assign, conflict detection, and a one-shot Reset.
//
// Persists user overrides in the same place as before (UserSettings.keybindings)
// so the backend keybindings.json importer keeps working unchanged.

import React, { useMemo, useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { IS_WEB } from '@/lib/platform';
import { open } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Search, RotateCcw, X } from 'lucide-react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { cn } from '@/lib/utils';
import { useSettings } from '@/state/settings';
import {
  COMMANDS,
  displayKey,
  normalizeKey,
  effectiveKey,
  eventToKey,
} from '@/lib/commands';

function isTauri() {
  return IS_WEB || (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window);
}

function groupBy(items, fn) {
  const out = new Map();
  for (const it of items) {
    const k = fn(it);
    if (!out.has(k)) out.set(k, []);
    out.get(k).push(it);
  }
  return out;
}

// Modal-ish capture for the next key combo the user presses. Renders inline
// in the shortcut cell when active.
function KeyCaptureCell({ initial, onCommit, onCancel }) {
  const [combo, setCombo] = useState(initial || '');
  const ref = useRef(null);

  useEffect(() => { ref.current?.focus(); }, []);

  const handleKeyDown = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') { onCancel(); return; }
    if (e.key === 'Enter' && combo) { onCommit(combo); return; }
    if (e.key === 'Backspace' && !combo) { onCancel(); return; }
    const k = eventToKey(e);
    if (k) setCombo(k);
  };

  return (
    <div
      ref={ref}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onBlur={onCancel}
      className="flex h-6 min-w-[120px] items-center justify-end gap-1 rounded border border-primary/60 bg-primary/5 px-2 text-[11px] outline-none ring-1 ring-primary/40"
    >
      <span className="font-mono">
        {combo ? displayKey(combo) : 'Press any key…'}
      </span>
      <span className="ml-auto text-[10px] text-muted-foreground">
        Esc to cancel · Enter to save
      </span>
    </div>
  );
}

function ShortcutCell({ commandId, currentKey, conflicting, onStart }) {
  if (!currentKey) {
    return (
      <button
        onClick={() => onStart(commandId)}
        className="text-[11px] italic text-muted-foreground hover:text-foreground"
      >
        Click to assign
      </button>
    );
  }
  return (
    <button
      onClick={() => onStart(commandId)}
      title={conflicting ? `Conflicts with: ${conflicting}` : 'Click to change'}
      className={cn(
        'rounded border px-2 py-0.5 font-mono text-[11px] transition-colors',
        conflicting
          ? 'border-destructive/60 bg-destructive/10 text-destructive'
          : 'border-border bg-muted/60 text-foreground hover:bg-muted'
      )}
    >
      {displayKey(currentKey)}
    </button>
  );
}

export function ShortcutsSettings() {
  const settings = useSettings((s) => s.settings);
  const updateSettings = useSettings((s) => s.update);
  const detectVscode = useSettings((s) => s.detectVscodeKeybindings);

  const [query, setQuery] = useState('');
  const [capturingId, setCapturingId] = useState(null);
  const [busy, setBusy] = useState(false);

  const userBindings = settings?.keybindings ?? [];

  // Build {commandId → effective key} once per render. Cheap.
  const effective = useMemo(() => {
    const m = new Map();
    for (const c of COMMANDS) {
      m.set(c.id, effectiveKey(c.id, userBindings));
    }
    return m;
  }, [userBindings]);

  // Reverse map for conflict highlighting — group commands by normalised key.
  const collisions = useMemo(() => {
    const byKey = new Map();
    for (const [id, k] of effective) {
      if (!k) continue;
      const norm = normalizeKey(k);
      if (!byKey.has(norm)) byKey.set(norm, []);
      byKey.get(norm).push(id);
    }
    const conflictsFor = new Map();
    for (const [, ids] of byKey) {
      if (ids.length > 1) {
        for (const id of ids) {
          const others = ids.filter((i) => i !== id);
          conflictsFor.set(id, others.join(', '));
        }
      }
    }
    return conflictsFor;
  }, [effective]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return COMMANDS;
    return COMMANDS.filter((c) =>
      c.label.toLowerCase().includes(q) ||
      c.id.toLowerCase().includes(q) ||
      c.group.toLowerCase().includes(q) ||
      (effective.get(c.id) || '').toLowerCase().includes(q)
    );
  }, [query, effective]);

  const grouped = useMemo(() => groupBy(filtered, (c) => c.group), [filtered]);

  // Persist a new binding. We keep exactly one binding per command — the
  // shortcuts panel doesn't expose multi-binding-per-command yet, and the
  // VS Code importer overrides the whole list anyway.
  const commit = async (commandId, comboOrNull) => {
    const without = userBindings.filter((b) => b.command !== commandId);
    const next = comboOrNull
      ? [...without, { key: normalizeKey(comboOrNull), command: commandId, when: null }]
      : without;
    await updateSettings({ keybindings: next });
  };

  const handleStartCapture = (id) => {
    setCapturingId(id);
  };

  const handleCommit = async (combo) => {
    if (!capturingId) return;
    const id = capturingId;
    setCapturingId(null);
    await commit(id, combo);
    toast.success(`${id} → ${displayKey(combo)}`);
  };

  const handleClearBinding = async (id, e) => {
    e.stopPropagation();
    await commit(id, null);
  };

  const handleResetAll = async () => {
    if (!confirm('Reset all shortcuts to their defaults?')) return;
    await updateSettings({ keybindings: [] });
    toast.success('All shortcuts reset');
  };

  const handleImportVscode = async () => {
    if (!isTauri()) return;
    setBusy(true);
    try {
      const detection = await detectVscode().catch(() => null);
      let path = detection?.importable?.[0]?.path ?? null;
      if (!path) {
        const picked = await open({
          multiple: false,
          directory: false,
          filters: [{ name: 'JSON', extensions: ['json'] }],
        });
        path = Array.isArray(picked) ? picked[0] : picked;
      }
      if (!path || typeof path !== 'string') { setBusy(false); return; }
      await invoke('import_keybindings', { path });
      await useSettings.getState().load();
      toast.success('VS Code keybindings imported');
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-full flex-col">
      {/* Toolbar: search + import + reset */}
      <div className="flex items-center gap-2 pb-3">
        <div className="relative flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search commands..."
            className="h-8 pl-7 text-[12px]"
          />
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={handleImportVscode}
          disabled={busy}
          className="shrink-0"
        >
          Import VS Code…
        </Button>
        <Button
          variant="destructive"
          size="sm"
          onClick={handleResetAll}
          disabled={busy || userBindings.length === 0}
          className="shrink-0"
        >
          <RotateCcw className="mr-1 size-3.5" />
          Reset All
        </Button>
      </div>

      {/* Column header */}
      <div className="flex items-center justify-between border-b border-border/60 px-3 pb-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/70">
        <span>Command</span>
        <span>Shortcut</span>
      </div>

      {/* List */}
      <ScrollArea className="min-h-0 flex-1 -mr-2 pr-2">
        {[...grouped.entries()].map(([group, items]) => (
          <section key={group} className="mb-2">
            <div className="px-3 pt-3 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/70">
              {group}
            </div>
            <ul className="divide-y divide-border/40">
              {items.map((c) => {
                const k = effective.get(c.id);
                const isOverridden = userBindings.some((b) => b.command === c.id);
                const conflicting = collisions.get(c.id);
                return (
                  <li
                    key={c.id}
                    className="flex items-center justify-between gap-3 px-3 py-2 hover:bg-accent/30"
                  >
                    <div className="flex min-w-0 flex-col">
                      <span className="text-[13px] leading-tight text-foreground">{c.label}</span>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      {capturingId === c.id ? (
                        <KeyCaptureCell
                          initial={k}
                          onCommit={handleCommit}
                          onCancel={() => setCapturingId(null)}
                        />
                      ) : (
                        <>
                          <ShortcutCell
                            commandId={c.id}
                            currentKey={k}
                            conflicting={conflicting}
                            onStart={handleStartCapture}
                          />
                          {isOverridden && (
                            <button
                              onClick={(e) => handleClearBinding(c.id, e)}
                              title="Reset to default"
                              className="text-muted-foreground hover:text-foreground"
                            >
                              <X className="size-3" />
                            </button>
                          )}
                        </>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>
        ))}
        {filtered.length === 0 && (
          <div className="px-3 py-8 text-center text-[12px] text-muted-foreground">
            No commands match “{query}”.
          </div>
        )}
      </ScrollArea>
    </div>
  );
}
