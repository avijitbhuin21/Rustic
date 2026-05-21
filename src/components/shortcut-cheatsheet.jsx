import React, { useEffect, useState } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { ScrollArea } from '@/components/ui/scroll-area';

const GROUPS = [
  {
    title: 'General',
    items: [
      ['Ctrl+P', 'Go to file'],
      ['Ctrl+Shift+P', 'Command palette'],
      ['Ctrl+/', 'Toggle this cheatsheet'],
      ['Ctrl+B', 'Toggle sidebar'],
      ['Ctrl+J', 'Toggle bottom panel'],
    ],
  },
  {
    title: 'Editor',
    items: [
      ['Ctrl+S', 'Save'],
      ['Ctrl+W', 'Close tab'],
      ['Ctrl+F', 'Find'],
      ['Ctrl+H', 'Replace'],
      ['F2', 'Rename symbol (Monaco)'],
    ],
  },
  {
    title: 'Explorer',
    items: [
      ['F2', 'Rename'],
      ['Del', 'Delete'],
      ['Right-click', 'Context menu'],
    ],
  },
  {
    title: 'Agent',
    items: [
      ['Ctrl+Enter', 'Send message'],
      ['Esc', 'Cancel streaming'],
    ],
  },
];

export function ShortcutCheatsheet() {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const onKey = (e) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key === '/') {
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
            {GROUPS.map((g) => (
              <section key={g.title}>
                <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                  {g.title}
                </h3>
                <dl className="flex flex-col gap-1">
                  {g.items.map(([key, label]) => (
                    <div key={key} className="flex items-center justify-between text-xs">
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
