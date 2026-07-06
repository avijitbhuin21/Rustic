import React, { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauriAvailable as isTauri } from '@/lib/platform';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Checkbox } from '@/components/ui/checkbox';
import { Plus, Trash2, Save, RefreshCw, Download } from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';


export function SkillsPanel() {
  const [items, setItems] = useState([]);
  const [repoItems, setRepoItems] = useState([]);
  const [selectedRepo, setSelectedRepo] = useState([]);
  const [repoSource, setRepoSource] = useState('');
  const [activeName, setActiveName] = useState(null);
  const [originalName, setOriginalName] = useState(null);
  const [editName, setEditName] = useState('');
  const [body, setBody] = useState('');
  const [newName, setNewName] = useState('');

  const load = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke('list_skills');
      setItems(Array.isArray(list) ? list : []);
    } catch (e) {}
  }, []);

  const loadRepo = useCallback(async () => {
    if (!isTauri()) return;
    if (!repoSource.trim()) {
      setRepoItems([]);
      return;
    }
    try {
      const list = await invoke('list_repo_skills', { source: repoSource.trim() });
      setRepoItems(Array.isArray(list) ? list : []);
    } catch (e) {
      toast.error(String(e));
    }
  }, [repoSource]);

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
      load();
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
      toast.success('Saved');
      load();
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
      load();
    } catch (e) {}
  };

  const install = async () => {
    if (!isTauri() || selectedRepo.length === 0 || !repoSource.trim()) return;
    // The Rust `install_repo_skills` takes parallel `paths` and `names`
    // arrays — `paths` locates each skill inside the repo, `names` overrides
    // the on-disk name. The repo listing exposes the in-repo path on `.path`
    // and the suggested name on `.name`, so map the selected ids to both.
    const selectedSet = new Set(selectedRepo);
    const picked = repoItems.filter((r) => selectedSet.has(r.id || r.name));
    const paths = picked.map((r) => r.path || r.name);
    const names = picked.map((r) => r.name);
    try {
      await invoke('install_repo_skills', {
        source: repoSource.trim(),
        paths,
        names,
      });
      toast.success(`Installed ${selectedRepo.length} skill(s)`);
      setSelectedRepo([]);
      load();
    } catch (e) {
      toast.error('Install failed');
    }
  };

  return (
    <div className="flex h-full flex-col">
      <Tabs defaultValue="installed" className="flex h-full flex-col gap-0">
        <TabsList className="mx-2 mt-2 h-7 self-start" variant="line">
          <TabsTrigger value="installed" className="text-xs">
            Installed
          </TabsTrigger>
          <TabsTrigger value="browse" className="text-xs">
            Browse
          </TabsTrigger>
        </TabsList>

        <TabsContent value="installed" className="flex-1 min-h-0">
          <div className="flex h-full flex-col">
            <div className="flex items-center gap-1.5 border-b border-border p-2">
              <Input
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                placeholder="New skill name"
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
                        >
                          {name}
                        </button>
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
                        placeholder="Skill name"
                        className="h-7 text-xs"
                      />
                    </div>
                    <Textarea
                      value={body}
                      onChange={(e) => setBody(e.target.value)}
                      className="flex-1 resize-none rounded-none border-0 font-mono text-[11px]"
                      placeholder="Skill body..."
                    />
                    <div className="border-t border-border p-1.5">
                      <Button size="sm" className="h-7 w-full text-xs" onClick={save}>
                        <Save className="mr-1 size-3" /> Save
                      </Button>
                    </div>
                  </>
                ) : (
                  <div className="flex flex-1 items-center justify-center px-4 text-center text-xs text-muted-foreground">
                    Select a skill.
                  </div>
                )}
              </div>
            </div>
          </div>
        </TabsContent>

        <TabsContent value="browse" className="flex-1 min-h-0">
          <div className="flex h-full flex-col">
            <div className="flex items-center gap-1.5 border-b border-border p-2">
              <Input
                value={repoSource}
                onChange={(e) => setRepoSource(e.target.value)}
                placeholder="Source repo URL (e.g. https://github.com/owner/repo)"
                className="h-7 flex-1 text-xs"
              />
              <Button size="sm" variant="ghost" className="h-7 px-2" onClick={loadRepo}>
                <RefreshCw className="size-3.5" />
              </Button>
              <Button
                size="sm"
                variant="default"
                className="h-7 px-2 text-xs"
                onClick={install}
                disabled={selectedRepo.length === 0 || !repoSource.trim()}
              >
                <Download className="mr-1 size-3" /> Install ({selectedRepo.length})
              </Button>
            </div>
            <ScrollArea className="flex-1">
              <ul className="p-1">
                {repoItems.length === 0 && (
                  <li className="px-2 py-1.5 text-xs text-muted-foreground">
                    {repoSource.trim() ? 'No repo skills.' : 'Enter a source URL and refresh.'}
                  </li>
                )}
                {repoItems.map((s) => {
                  const id = s.id || s.name;
                  const checked = selectedRepo.includes(id);
                  return (
                    <li
                      key={id}
                      className="flex items-start gap-2 rounded px-2 py-1.5 text-xs hover:bg-muted/60"
                    >
                      <Checkbox
                        checked={checked}
                        onCheckedChange={(v) =>
                          setSelectedRepo((prev) =>
                            v ? [...prev, id] : prev.filter((x) => x !== id)
                          )
                        }
                      />
                      <div className="min-w-0 flex-1">
                        <div className="font-medium">{s.name || id}</div>
                        {s.description && (
                          <div className="text-[11px] italic text-muted-foreground">{s.description}</div>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            </ScrollArea>
          </div>
        </TabsContent>
      </Tabs>
    </div>
  );
}

export default SkillsPanel;
