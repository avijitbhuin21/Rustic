import React, { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { SplitPane } from '@/components/ui/split-pane';
import { Plus, Trash2, RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import { toastError } from '@/lib/toast-error';
import { cn } from '@/lib/utils';
import { notifyExtensionsChanged } from '@/lib/extensions-events';
import { MarkdownEntryEditor } from './markdown-entry-editor';


export function SkillsPanel() {
  const [items, setItems] = useState([]);
  const [activeName, setActiveName] = useState(null);
  const [originalName, setOriginalName] = useState(null);
  const [editName, setEditName] = useState('');
  const [body, setBody] = useState('');
  const [newName, setNewName] = useState('');
  const [mode, setMode] = useState('preview');

  const load = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_skills');
      setItems(Array.isArray(list) ? list : []);
    } catch (e) { console.warn('[skills-panel] load failed', e); }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Reload when the agent or Settings › Library changes a skill.
  useEffect(() => {
    window.addEventListener('rustic:extensions-changed', load);
    return () => window.removeEventListener('rustic:extensions-changed', load);
  }, [load]);

  const select = async (name) => {
    setActiveName(name);
    setOriginalName(name);
    setEditName(name);
    setMode('preview');
    if (!isTauri()) {
      setBody('');
      return;
    }
    try {
      const b = await invoke('get_skill_body', { name });
      setBody(typeof b === 'string' ? b : '');
    } catch (e) {
      setBody('');
    }
  };

  const create = async () => {
    if (!newName.trim() || !isTauri()) return;
    try {
      await invoke('create_skill', { name: newName.trim(), body: '' });
      setNewName('');
      notifyExtensionsChanged({ kind: 'skill' });
    } catch (e) {
      toast.error('Create failed');
    }
  };

  const save = async () => {
    if (!activeName || !isTauri()) return;
    const nextName = (editName || activeName).trim();
    try {
      await invoke('update_skill', { originalName: originalName || activeName, name: nextName, body });
      setActiveName(nextName);
      setOriginalName(nextName);
      setMode('preview');
      toast.success('Saved');
      notifyExtensionsChanged({ kind: 'skill' });
    } catch (e) {
      toast.error('Save failed');
    }
  };

  const remove = async (name) => {
    if (!isTauri()) return;
    try {
      await invoke('delete_skill', { name });
      if (activeName === name) {
        setActiveName(null);
        setOriginalName(null);
        setEditName('');
        setBody('');
      }
      notifyExtensionsChanged({ kind: 'skill' });
    } catch (e) { toastError('Delete failed', e); }
  };

  const list = (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <ul className="p-1">
        {items.length === 0 && (
          <li className="px-2 py-1.5 text-xs text-muted-foreground">No skills.</li>
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
                title={r.description || name}
              >
                {name}
              </button>
              {r.scope === 'builtin' ? (
                <span className="shrink-0 text-[10px] text-muted-foreground">built-in</span>
              ) : (
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-6 opacity-0 group-hover:opacity-100 touch-reveal"
                  onClick={() => remove(name)}
                  title="Delete"
                >
                  <Trash2 className="size-3" />
                </Button>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );

  const detail = activeName ? (
    <MarkdownEntryEditor
      kind="skill"
      title={activeName}
      editName={editName}
      onEditName={setEditName}
      body={body}
      onBody={setBody}
      mode={mode}
      onMode={setMode}
      onSave={save}
    />
  ) : (
    <div className="flex flex-1 items-center justify-center px-4 text-center text-xs text-muted-foreground">
      Select a skill.
    </div>
  );

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-1.5 border-b border-border p-2">
        <Input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && create()}
          placeholder="New skill name"
          className="h-7 text-xs"
        />
        <Button size="sm" variant="default" className="h-7 px-2" onClick={create} title="Create skill">
          <Plus className="size-3.5" />
        </Button>
        <Button size="sm" variant="ghost" className="h-7 px-2" onClick={load} title="Reload">
          <RefreshCw className="size-3.5" />
        </Button>
      </div>
      <SplitPane storageKey="agent-skills" defaultWidth={200} left={list} right={detail} />
    </div>
  );
}

export default SkillsPanel;
