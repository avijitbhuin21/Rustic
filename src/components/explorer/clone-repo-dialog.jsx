import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { FolderGit2, FolderOpen, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { useExplorer } from '@/state/explorer';

// Accept both full URLs ("https://github.com/user/repo[.git]") and the bare
// "github.com/user/repo" people paste from the address bar — the backend only
// allows https:// and scp-style URLs, so prefix the scheme when it's missing.
function normalizeUrl(raw) {
  const url = raw.trim();
  if (!url) return '';
  const looksScp = !url.includes('://') && url.includes('@') && url.includes(':');
  if (looksScp || url.includes('://')) return url;
  return `https://${url}`;
}

export default function CloneRepoDialog({ open: isOpen, onOpenChange }) {
  const addProject = useExplorer((s) => s.addProject);
  const [url, setUrl] = useState('');
  const [destDir, setDestDir] = useState('');
  const [cloning, setCloning] = useState(false);
  const [progressText, setProgressText] = useState(null);

  // Prefill the destination with ~/projects when the dialog opens, so the
  // folder question is answered up front and the user only has to change it
  // when they want somewhere else.
  useEffect(() => {
    if (!isOpen) return;
    setProgressText(null);
    invoke('get_default_projects_dir')
      .then((dir) => setDestDir((cur) => cur || dir))
      .catch(() => {});
  }, [isOpen]);

  // Live clone progress — the backend streams git's sideband lines
  // ("Receiving objects: 42% (12000/90000)", "Updating files: …") under the
  // synthetic project id `__clone__`.
  useEffect(() => {
    if (!isOpen) return undefined;
    let alive = true;
    let unlisten;
    (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        const un = await listen('git-progress', (e) => {
          const p = e.payload ?? {};
          if (p.projectId !== '__clone__' || !alive) return;
          setProgressText(p.phase === 'done' ? null : (p.text ?? 'Cloning…'));
        });
        if (!alive) un();
        else unlisten = un;
      } catch {
        // Event transport unavailable — spinner alone will have to do.
      }
    })();
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [isOpen]);

  async function handleBrowse() {
    try {
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked === 'string' && picked) setDestDir(picked);
    } catch (err) {
      toast.error(`Folder picker failed: ${err?.message ?? err}`);
    }
  }

  const canClone = normalizeUrl(url).length > 0 && destDir.trim().length > 0 && !cloning;

  async function handleClone() {
    if (!canClone) return;
    setCloning(true);
    setProgressText('Starting clone…');
    try {
      const clonedPath = await invoke('git_clone', {
        url: normalizeUrl(url),
        targetDir: destDir.trim(),
      });
      await addProject(clonedPath);
      toast.success(`Cloned into ${clonedPath}`);
      setUrl('');
      onOpenChange(false);
    } catch (err) {
      toast.error(`Clone failed: ${err?.message ?? err}`);
    } finally {
      setCloning(false);
      setProgressText(null);
    }
  }

  return (
    <Dialog open={isOpen} onOpenChange={(v) => !cloning && onOpenChange(v)}>
      <DialogContent className="sm:max-w-[440px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <FolderGit2 className="size-4" />
            Clone Repository
          </DialogTitle>
        </DialogHeader>

        <div className="flex flex-col gap-4 py-2">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="clone-url">Repository URL</Label>
            <Input
              id="clone-url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleClone()}
              placeholder="https://github.com/user/repo.git"
              disabled={cloning}
              autoFocus
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="clone-dest">Clone into folder</Label>
            <div className="flex gap-1.5">
              <Input
                id="clone-dest"
                value={destDir}
                onChange={(e) => setDestDir(e.target.value)}
                placeholder="Pick a destination folder"
                disabled={cloning}
                className="min-w-0 flex-1"
              />
              <Button
                variant="outline"
                size="icon"
                className="shrink-0"
                onClick={handleBrowse}
                disabled={cloning}
                aria-label="Browse for folder"
              >
                <FolderOpen className="size-3.5" />
              </Button>
            </div>
            <p className="text-[11px] text-muted-foreground">
              The repository is cloned into a new sub-folder here and opened as a project.
            </p>
          </div>

          {(cloning || progressText) && (
            <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <Loader2 className="size-3 shrink-0 animate-spin" />
              <span className="truncate">{progressText ?? 'Cloning…'}</span>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={cloning}>
            Cancel
          </Button>
          <Button onClick={handleClone} disabled={!canClone}>
            {cloning ? (
              <>
                <Loader2 className="size-3.5 animate-spin" />
                Cloning…
              </>
            ) : (
              <>
                <FolderGit2 className="size-3.5" />
                Clone
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
