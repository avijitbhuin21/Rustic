import React, { useEffect, useMemo, useState } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useSettings } from '@/state/settings';
import { COMMANDS, effectiveKey, displayKey } from '@/lib/commands';

// Hints that aren't registry commands (Monaco built-ins, chat composer,
// pointer gestures) — merged into the derived groups below.
const STATIC_EXTRAS = {
  Editor: [
    ['Ctrl+F', 'Find'],
    ['Ctrl+H', 'Replace'],
    ['Ctrl+W', 'Close tab'],
    ['F2', 'Rename symbol'],
  ],
  Explorer: [
    ['Right-click', 'Context menu'],
  ],
  Agent: [
    ['Ctrl+Enter', 'Send message'],
    ['Esc', 'Cancel streaming'],
  ],
};

const GROUP_ORDER = ['View', 'File', 'Editor', 'Terminal', 'Explorer', 'Agent', 'Workspace', 'Preferences', 'Help'];

export function ShortcutCheatsheet() {
  const [open, setOpen] = useState(false);
  const keybindings = useSettings((s) => s.settings?.keybindings);

  // Derived from the command registry + the user's overrides, so the sheet
  // never drifts from what the keys actually do.
  const groups = useMemo(() => {
    const by = new Map();
    const push = (title, item) => {
      if (!by.has(title)) by.set(title, []);
      by.get(title).push(item);
    };
    for (const c of COMMANDS) {
      const key = effectiveKey(c.id, keybindings);
      if (!key) continue;
      push(c.group, [displayKey(key), c.label]);
    }
    for (const [title, items] of Object.entries(STATIC_EXTRAS)) {
      for (const item of items) push(title, item);
    }
    return [...by.entries()]
      .sort((a, b) => GROUP_ORDER.indexOf(a[0]) - GROUP_ORDER.indexOf(b[0]))
      .map(([title, items]) => ({ title, items }));
  }, [keybindings]);

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === '\\' && !e.ctrlKey && !e.metaKey && !e.altKey && !isInputFocused()) {
        e.preventDefault();
        setOpen((o) => !o);
      } else if (e.key === '?' && !e.ctrlKey && !e.metaKey && !isInputFocused()) {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Keyboard Shortcuts</DialogTitle>
        </DialogHeader>
        <ScrollArea className="max-h-[60vh] pr-2">
          <div className="grid grid-cols-2 gap-x-6 gap-y-4">
            {groups.map((g) => (
              <section key={g.title}>
                <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                  {g.title}
                </h3>
                <dl className="flex flex-col gap-1">
                  {g.items.map(([key, label]) => (
                    <div key={`${key}-${label}`} className="flex items-center justify-between text-xs">
                      <dt className="text-foreground">{label}</dt>
                      <dd>
                        <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10px]">
                          {key}
                        </kbd>
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            ))}
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}

function isInputFocused() {
  const a = document.activeElement;
  if (!a) return false;
  const tag = (a.tagName || '').toLowerCase();
  return tag === 'input' || tag === 'textarea' || a.isContentEditable;
}
