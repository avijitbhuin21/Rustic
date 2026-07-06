// Worktree isolation settings section.
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
import { Section, isTauri } from './shared';

// ─── Worktrees ────────────────────────────────────────────────────────────────

export function WorktreeSection() {
  const [command, setCommand] = useState('');
  const [timeoutSecs, setTimeoutSecs] = useState(600);
  const [linkedDirs, setLinkedDirs] = useState('');
  const [createHook, setCreateHook] = useState('');
  const [removeHook, setRemoveHook] = useState('');
  const [overrides, setOverrides] = useState([]);
  const [loaded, setLoaded] = useState(false);
  const projects = useExplorer((s) => s.projects);

  useEffect(() => {
    if (!isTauri()) return;
    (async () => {
      try {
        const s = await invoke('worktree_get_settings');
        setCommand(s?.validation_command || '');
        setTimeoutSecs(Number(s?.validation_timeout_secs) || 600);
        setLinkedDirs((s?.symlink_directories || []).join(', '));
        setCreateHook(s?.create_hook || '');
        setRemoveHook(s?.remove_hook || '');
        setOverrides(
          Object.entries(s?.project_validation_commands || {}).map(([projectId, cmd]) => ({ projectId, cmd })),
        );
        setLoaded(true);
      } catch { /* command missing on old server — leave defaults */ }
    })();
  }, []);

  const save = async () => {
    try {
      const map = {};
      for (const o of overrides) {
        if (o.projectId && o.cmd.trim()) map[o.projectId] = o.cmd.trim();
      }
      await invoke('worktree_set_settings', {
        settings: {
          validation_command: command.trim(),
          validation_timeout_secs: Math.max(1, Number(timeoutSecs) || 600),
          symlink_directories: linkedDirs.split(',').map((s) => s.trim()).filter(Boolean),
          create_hook: createHook.trim(),
          remove_hook: removeHook.trim(),
          project_validation_commands: map,
        },
      });
      toast.success('Worktree settings saved');
    } catch (e) { toast.error(String(e)); }
  };

  const setOverride = (idx, patch) =>
    setOverrides((prev) => prev.map((o, i) => (i === idx ? { ...o, ...patch } : o)));

  return (
    <Section title="Worktrees">
      <p className="mb-3 text-[12px] italic leading-snug text-muted-foreground">
        Every task runs in an isolated git worktree and auto-merges back when its turn ends. The validation command
        runs inside the worktree before each merge lands — if it fails, the merge is parked instead of landing.
        Per-project overrides replace the global command for that project. Linked directories are
        junctioned/symlinked from your main checkout into each new worktree so heavy dependency folders
        (node_modules, target, .venv) don't have to be reinstalled per task. To copy extra gitignored files into
        worktrees (beyond the automatic .env* copy), list patterns in a .worktreeinclude file at the repo root.
        Hooks let non-git projects join isolation: the create hook receives the task id and must print the new
        worktree's absolute path as its last stdout line; the remove hook receives the worktree path.
      </p>
      <div className="flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <span className="w-40 shrink-0 text-xs text-muted-foreground">Validation command</span>
          <Input
            className="h-8 flex-1 text-xs"
            placeholder="e.g. bun run typecheck — empty disables validation"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
          />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-40 shrink-0 text-xs text-muted-foreground">Validation timeout (s)</span>
          <Input
            className="h-8 w-28 text-xs"
            type="number"
            min={1}
            value={timeoutSecs}
            onChange={(e) => setTimeoutSecs(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <span className="w-40 shrink-0 text-xs text-muted-foreground">Per-project validation</span>
            <Button
              size="sm"
              variant="outline"
              className="h-7 text-xs"
              onClick={() => setOverrides((prev) => [...prev, { projectId: '', cmd: '' }])}
            >
              Add override
            </Button>
          </div>
          {overrides.map((o, idx) => (
            <div key={idx} className="ml-[10.5rem] flex items-center gap-2">
              <select
                className="h-8 w-44 shrink-0 rounded-md border border-input bg-background px-2 text-xs"
                value={o.projectId}
                onChange={(e) => setOverride(idx, { projectId: e.target.value })}
              >
                <option value="">Select project…</option>
                {projects.map((p) => (
                  <option key={p.id} value={String(p.id)}>{p.name}</option>
                ))}
              </select>
              <Input
                className="h-8 flex-1 text-xs"
                placeholder="validation command for this project"
                value={o.cmd}
                onChange={(e) => setOverride(idx, { cmd: e.target.value })}
              />
              <Button
                size="sm"
                variant="ghost"
                className="h-7 px-2 text-xs"
                onClick={() => setOverrides((prev) => prev.filter((_, i) => i !== idx))}
              >
                ×
              </Button>
            </div>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <span className="w-40 shrink-0 text-xs text-muted-foreground">Linked directories</span>
          <Input
            className="h-8 flex-1 text-xs"
            placeholder="comma-separated, e.g. node_modules, target, .venv"
            value={linkedDirs}
            onChange={(e) => setLinkedDirs(e.target.value)}
          />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-40 shrink-0 text-xs text-muted-foreground">Create hook</span>
          <Input
            className="h-8 flex-1 text-xs"
            placeholder="non-git isolation: <cmd> <task_id> → prints worktree path"
            value={createHook}
            onChange={(e) => setCreateHook(e.target.value)}
          />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-40 shrink-0 text-xs text-muted-foreground">Remove hook</span>
          <Input
            className="h-8 flex-1 text-xs"
            placeholder="cleanup: <cmd> <worktree_path>"
            value={removeHook}
            onChange={(e) => setRemoveHook(e.target.value)}
          />
        </div>
      </div>
      <div className="mt-3 flex justify-end">
        <Button size="sm" className="text-xs" onClick={save} disabled={!loaded && isTauri()}>Save</Button>
      </div>
    </Section>
  );
}

