import React, { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FileText, Plus, Sparkles, Workflow } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { MOD_LABEL } from '@/lib/platform';

const MAX_SHOWN = 60;
const ICONS = { file: FileText, skill: Sparkles, workflow: Workflow };
const GROUP_LABEL = { file: 'Files', skill: 'Skills', workflow: 'Workflows' };

/** The text to paste for a picked item; skills and workflows paste their full instructions. */
async function textFor(item) {
  if (item.kind === 'file') return `@${item.value} `;
  const command = item.kind === 'skill' ? 'get_skill_body' : 'get_workflow_body';
  return invoke(command, { name: item.value });
}

/**
 * Picker for putting a project file reference, a skill or a workflow into a CLI
 * agent's input. It only pastes text at the CLI's cursor through `onInsert` and
 * never submits: the user reviews it and presses Enter in the CLI themselves.
 */
export function CliInsertPicker({ projectRoot, open, onOpenChange, onInsert }) {
  const [query, setQuery] = useState('');
  const [files, setFiles] = useState(null);
  const [skills, setSkills] = useState([]);
  const [workflows, setWorkflows] = useState([]);
  const [cursor, setCursor] = useState(0);
  const listRef = useRef(null);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setCursor(0);
    let stale = false;
    Promise.all([
      projectRoot
        ? invoke('list_project_files', { rootPath: projectRoot, maxFiles: 5000 }).catch(() => [])
        : Promise.resolve([]),
      invoke('list_skills').catch(() => []),
      invoke('list_workflows').catch(() => []),
    ]).then(([f, sk, wf]) => {
      if (stale) return;
      setFiles(Array.isArray(f) ? f : []);
      setSkills(Array.isArray(sk) ? sk : []);
      setWorkflows(Array.isArray(wf) ? wf : []);
    });
    return () => {
      stale = true;
    };
  }, [open, projectRoot]);

  const items = useMemo(() => {
    const q = query.trim().toLowerCase();
    const hit = (s) => !q || String(s).toLowerCase().includes(q);
    const named = (kind, list) =>
      list
        .filter((x) => hit(x.name) || hit(x.description))
        .map((x) => ({ kind, value: x.name, label: x.name, hint: x.description }));
    return [
      ...(files ?? []).filter(hit).slice(0, MAX_SHOWN).map((p) => ({ kind: 'file', value: p, label: p })),
      ...named('skill', skills),
      ...named('workflow', workflows),
    ];
  }, [query, files, skills, workflows]);

  useEffect(() => {
    setCursor((c) => Math.min(c, Math.max(items.length - 1, 0)));
  }, [items.length]);

  useEffect(() => {
    listRef.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [cursor]);

  const pick = async (item) => {
    if (!item) return;
    onOpenChange(false);
    try {
      const text = await textFor(item);
      if (text) onInsert(text);
    } catch (err) {
      toast.error(`Couldn't load ${item.label}: ${String(err)}`);
    }
  };

  const onKeyDown = (e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, items.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
    } else if (e.key === 'Enter') {
      // Picks the row in this menu; it is never forwarded to the CLI.
      e.preventDefault();
      pick(items[cursor]);
    }
  };

  return (
    <Popover open={open} onOpenChange={onOpenChange}>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <Button variant="ghost" size="icon-xs">
              <Plus className="size-3.5" />
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
          Insert a file, skill or workflow ({MOD_LABEL}+Shift+A)
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        align="end"
        sideOffset={6}
        className="w-80 gap-0 overflow-hidden p-0"
        onCloseAutoFocus={(e) => e.preventDefault()}
      >
        <input
          autoFocus
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setCursor(0);
          }}
          onKeyDown={onKeyDown}
          placeholder="Search files, skills, workflows"
          className="h-8 w-full border-b border-border bg-transparent px-2.5 text-xs text-foreground outline-none placeholder:text-muted-foreground"
        />
        <div ref={listRef} className="max-h-72 overflow-y-auto py-1">
          {items.length === 0 && (
            <div className="px-2.5 py-3 text-center text-xs text-muted-foreground">
              {files === null ? 'Loading…' : 'Nothing matches'}
            </div>
          )}
          {items.map((item, i) => {
            const Icon = ICONS[item.kind];
            const header = i === 0 || items[i - 1].kind !== item.kind;
            return (
              <React.Fragment key={`${item.kind}:${item.value}`}>
                {header && (
                  <div className="px-2.5 pb-0.5 pt-1.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                    {GROUP_LABEL[item.kind]}
                  </div>
                )}
                <button
                  type="button"
                  data-active={i === cursor}
                  onMouseEnter={() => setCursor(i)}
                  onClick={() => pick(item)}
                  title={item.hint || item.label}
                  className={cn(
                    'flex w-full items-center gap-2 px-2.5 py-1 text-left text-xs',
                    i === cursor ? 'bg-muted text-foreground' : 'text-foreground/90',
                  )}
                >
                  <Icon className="size-3 shrink-0 text-info" />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                </button>
              </React.Fragment>
            );
          })}
        </div>
        <div className="border-t border-border px-2.5 py-1 text-[10px] text-muted-foreground">
          Pasted at the CLI's cursor. Nothing is sent until you press Enter there.
        </div>
      </PopoverContent>
    </Popover>
  );
}
