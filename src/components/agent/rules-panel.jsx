import React, { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { IS_WEB } from '@/lib/platform';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Switch } from '@/components/ui/switch';
import { Plus, Trash2, Save, RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { useAgent } from '@/state/agent';

function isTauri() {
  return IS_WEB || (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window);
}

export function RulesPanel() {
  const projectRoot = useAgent((s) => s.activeProject.root || null);
  const [items, setItems] = useState([]);
  // `activeName` is the rule name currently being edited. We also keep
  // `originalName` so an in-place rename round-trips correctly through
  // `update_rule`'s (original_name, name, body) shape.
  const [activeName, setActiveName] = useState(null);
  const [originalName, setOriginalName] = useState(null);
  const [editName, setEditName] = useState('');
  const [body, setBody] = useState('');
  const [newName, setNewName] = useState('');

  const load = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_rules', { projectRoot });
      setItems(Array.isArray(list) ? list : []);
    } catch (e) {}
  }, [projectRoot]);

  useEffect(() => {
    load();
  }, [load]);

  const select = async (name) => {
    setActiveName(name);
    setOriginalName(name);
    setEditName(name);
    if (!isTauri()) {
      setBody('');
      return;
    }
    try {
      const b = await invoke('get_rule_body', { name });
      setBody(typeof b === 'string' ? b : '');
    } catch (e) {
      setBody('');
    }
  };

  const create = async () => {
    if (!newName.trim() || !isTauri()) return;
    try {
      await invoke('create_rule', { name: newName.trim(), body: '' });
      setNewName('');
      load();
    } catch (e) {
      toast.error('Create failed');
    }
  };

  const save = async () => {
    if (!activeName || !isTauri()) return;
    const nextName = (editName || activeName).trim();
    try {
      await invoke('update_rule', { originalName: originalName || activeName, name: nextName, body });
      setActiveName(nextName);
      setOriginalName(nextName);
      toast.success('Saved');
      load();
    } catch (e) {
      toast.error('Save failed');
    }
  };

  const remove = async (name) => {
    if (!isTauri()) return;
    try {
      await invoke('delete_rule', { name });
      if (activeName === name) {
        setActiveName(null);
        setOriginalName(null);
        setEditName('');
        setBody('');
      }
      load();
    } catch (e) {}
  };

  const toggle = async (name, active) => {
    if (!isTauri()) return;
    try {
      await invoke('set_rule_activation', {
        name,
        state: active ? 'global' : 'inactive',
        projectRoot,
      });
      load();
    } catch (e) {
      toast.error(`Toggle failed: ${typeof e === 'string' ? e : e?.message ?? e}`);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-1.5 border-b border-border p-2">
        <Input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="New rule name"
          className="h-7 text-xs"
        />
        <Button size="sm" variant="default" className="h-7 px-2" onClick={create}>
          <Plus className="size-3.5" />
        </Button>
        <Button size="sm" variant="ghost" className="h-7 px-2" onClick={load}>
          <RefreshCw className="size-3.5" />
        </Button>
      </div>
      <div className="flex flex-1 min-h-0">
        <ScrollArea className="w-1/2 border-r border-border">
          <ul className="p-1">
            {items.length === 0 && (
              <li className="px-2 py-1.5 text-xs text-muted-foreground">No rules.</li>
            )}
            {items.map((r) => {
              const name = r.name || r.id;
              return (
                <li
                  key={name}
                  className={cn(
                    'group flex items-center gap-1.5 rounded px-2 py-1.5 text-xs hover:bg-muted/60',
                    activeName === name && 'bg-muted'
                  )}
                >
                  <button
                    type="button"
                    onClick={() => select(name)}
                    className="min-w-0 flex-1 truncate text-left"
                  >
                    {name}
                  </button>
                  <Switch
                    checked={r.state === 'global' || r.state === 'project'}
                    onCheckedChange={(v) => toggle(name, v)}
                    className="scale-75"
                  />
                  <Button
                    size="icon"
                    variant="ghost"
                    className="size-6 opacity-0 group-hover:opacity-100"
                    onClick={() => remove(name)}
                  >
                    <Trash2 className="size-3" />
                  </Button>
                </li>
              );
            })}
          </ul>
        </ScrollArea>
        <div className="flex w-1/2 flex-col">
          {activeName ? (
            <>
              <div className="border-b border-border p-1.5">
                <Input
                  value={editName}
                  onChange={(e) => setEditName(e.target.value)}
                  placeholder="Rule name"
                  className="h-7 text-xs"
                />
              </div>
              <Textarea
                value={body}
                onChange={(e) => setBody(e.target.value)}
                className="flex-1 resize-none rounded-none border-0 font-mono text-[11px]"
                placeholder="Rule body..."
              />
              <div className="border-t border-border p-1.5">
                <Button size="sm" className="h-7 w-full text-xs" onClick={save}>
                  <Save className="mr-1 size-3" /> Save
                </Button>
              </div>
            </>
          ) : (
            <div className="flex flex-1 items-center justify-center px-4 text-center text-xs text-muted-foreground">
              Select a rule to edit.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default RulesPanel;
