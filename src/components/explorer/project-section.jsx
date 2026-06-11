import React, { useEffect, useRef, useState } from 'react';
import { ChevronRight, FolderGit2, Terminal, X, FilePlus, FolderPlus } from 'lucide-react';
import { FileTree } from './file-tree';
import { useExplorer } from '@/state/explorer';
import { useTerminal } from '@/state/terminal';
import { useEditor } from '@/state/editor';
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from '@/components/ui/context-menu';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import { IS_WEB } from '@/lib/platform';

export function ProjectSection({ project, onOpenFile }) {
  const expanded = useExplorer((s) => !!s.expandedProjects[project.id]);
  const toggle = useExplorer((s) => s.toggleProjectExpanded);
  const removeProject = useExplorer((s) => s.removeProject);
  // Keep FileTree mounted once it's been opened so state (open folders, cache) survives collapse
  const [everExpanded, setEverExpanded] = useState(expanded);
  const fileTreeRef = useRef(null);

  useEffect(() => {
    if (expanded) setEverExpanded(true);
  }, [expanded]);

  const handleOpenTerminal = async (e) => {
    e.stopPropagation();
    try {
      // Route through useEditor.openTerminal to show the terminal in the
      // bottom panel. Calling createTerminal alone spawns the PTY but never
      // surfaces it in the UI.
      const info = await useTerminal.getState().createTerminal({ cwd: project.root_path, label: project.name });
      useEditor.getState().openTerminal(info.id, project.name);
      toast.success(`Terminal opened in ${project.name}`);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const handleRemove = async (e) => {
    e.stopPropagation();
    try {
      await removeProject(project.id);
    } catch (err) {
      toast.error(String(err));
    }
  };

  // Trigger FileTree.createAndEdit on the project root. If the project is
  // collapsed (FileTree not mounted yet) we expand it first, then poll for
  // the ref to attach across the next few frames — useImperativeHandle only
  // wires up once FileTree's first render commits.
  const requestCreate = async (kind, e) => {
    e?.stopPropagation?.();
    if (!expanded) toggle(project.id);
    setEverExpanded(true);
    for (let i = 0; i < 30; i++) {
      if (fileTreeRef.current) break;
      await new Promise((r) => requestAnimationFrame(r));
    }
    if (!fileTreeRef.current) {
      toast.error('File tree not ready yet — try again.');
      return;
    }
    fileTreeRef.current.createAndEdit(project.root_path, kind);
  };

  const handleNewFile = (e) => requestCreate('file', e);
  const handleNewFolder = (e) => requestCreate('folder', e);

  // Root drop target: dragging a node onto the project header or the empty zone
  // below the tree moves it to the project ROOT. The tree rows themselves can
  // only drop INTO nested folders — there is no folder row representing the
  // root — so without these zones a file dragged into a folder can never be
  // dragged back out to the top level.
  const [rootDragOver, setRootDragOver] = useState(false);
  const hasRusticDrag = (e) => {
    const t = e.dataTransfer?.types;
    return !!t && (Array.from(t).includes('application/x-rustic-file') || Array.from(t).includes('text/plain'));
  };
  const hasExternalFiles = (e) =>
    Array.from(e.dataTransfer?.types || []).includes('Files');
  const onRootDragOver = (e) => {
    if (!hasRusticDrag(e) && !hasExternalFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = hasExternalFiles(e) ? 'copy' : 'move';
    if (!rootDragOver) setRootDragOver(true);
  };
  const onRootDragLeave = () => {
    if (rootDragOver) setRootDragOver(false);
  };
  const onRootDrop = async (e) => {
    // External OS files dropped on the project header / empty zone: copy them
    // into the project root. Desktop streams bytes via desktop-upload (no OS
    // paths on HTML5 drops); web uses the HTTP upload transport.
    const osFiles = Array.from(e.dataTransfer?.files || []);
    if (osFiles.length > 0) {
      e.preventDefault();
      e.stopPropagation();
      setRootDragOver(false);
      try {
        const count = IS_WEB
          ? await (await import('@/lib/file-transfer')).uploadFileList(project.root_path, osFiles)
          : await (await import('@/lib/desktop-upload')).uploadDroppedFiles(project.root_path, osFiles);
        toast.success(`Added ${count} file${count > 1 ? 's' : ''} to ${project.name}`);
      } catch (err) {
        console.error('[explorer] root drop upload failed:', err);
        toast.error(String(err?.message || err));
      }
      return;
    }

    if (!hasRusticDrag(e)) return;
    e.preventDefault();
    e.stopPropagation();
    setRootDragOver(false);
    const src =
      e.dataTransfer.getData('application/x-rustic-file') ||
      e.dataTransfer.getData('text/plain');
    if (!src) return;
    // Make sure the tree is mounted so its `moveInto` is available.
    if (!fileTreeRef.current) {
      setEverExpanded(true);
      if (!expanded) toggle(project.id);
      for (let i = 0; i < 30; i++) {
        if (fileTreeRef.current) break;
        // eslint-disable-next-line no-await-in-loop
        await new Promise((r) => requestAnimationFrame(r));
      }
    }
    fileTreeRef.current?.moveInto(src);
  };

  return (
    <div className="flex flex-col border-b border-border/60 last:border-b-0">
      <div
        onClick={() => toggle(project.id)}
        onDragOver={onRootDragOver}
        onDragLeave={onRootDragLeave}
        onDrop={onRootDrop}
        data-explorer-node="folder"
        className={cn(
          'group/project sticky top-0 z-10 flex h-7 cursor-pointer items-center gap-1 border-b border-border/60 bg-muted/60 px-2 text-[11px] font-semibold uppercase tracking-wide text-foreground/90 backdrop-blur hover:bg-muted/80',
          rootDragOver && 'bg-primary/15 ring-1 ring-inset ring-primary/40'
        )}
        title={rootDragOver ? 'Drop to move to project root' : undefined}
      >
        <ChevronRight
          className="size-3 shrink-0 transition-transform duration-200 ease-in-out"
          style={{ transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)' }}
        />
        <FolderGit2 className="size-3 shrink-0" />
        <span className="min-w-0 flex-1 truncate">{project.name}</span>
        <div className="ml-auto flex items-center gap-0.5 opacity-0 transition-opacity group-hover/project:opacity-100">
          <button
            onClick={handleNewFile}
            title="New file in project root"
            className="flex size-5 items-center justify-center rounded hover:bg-foreground/10"
          >
            <FilePlus className="size-3" />
          </button>
          <button
            onClick={handleNewFolder}
            title="New folder in project root"
            className="flex size-5 items-center justify-center rounded hover:bg-foreground/10"
          >
            <FolderPlus className="size-3" />
          </button>
          <button
            onClick={handleOpenTerminal}
            title="Open terminal in project root"
            className="flex size-5 items-center justify-center rounded hover:bg-foreground/10"
          >
            <Terminal className="size-3" />
          </button>
          <button
            onClick={handleRemove}
            title="Remove project from workspace"
            className="flex size-5 items-center justify-center rounded hover:bg-destructive/20 hover:text-destructive"
          >
            <X className="size-3" />
          </button>
        </div>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateRows: expanded ? '1fr' : '0fr',
          transition: 'grid-template-rows 220ms ease',
        }}
      >
        <div style={{ overflow: 'hidden' }}>
          {everExpanded && (
            <>
              <FileTree ref={fileTreeRef} rootPath={project.root_path} onOpenFile={onOpenFile} />
              {/* Right-clickable empty zone at the bottom of every expanded
                  project. Targets the project root so users can create
                  top-level files/folders without having to right-click a
                  sibling node first. Keep this tall enough to be an obvious
                  click target but small enough not to push other projects
                  far out of view. */}
              <ContextMenu>
                <ContextMenuTrigger asChild>
                  <div
                    className={cn(
                      'h-16 w-full',
                      rootDragOver && 'bg-primary/10 ring-1 ring-inset ring-primary/30'
                    )}
                    onDragOver={onRootDragOver}
                    onDragLeave={onRootDragLeave}
                    onDrop={onRootDrop}
                    title={rootDragOver ? 'Drop to move to project root' : undefined}
                    onClick={() => {
                      useExplorer.getState().setLastSelectedNode({
                        path: project.root_path,
                        isDir: true,
                      });
                    }}
                    onContextMenu={() => {
                      useExplorer.getState().setLastSelectedNode({
                        path: project.root_path,
                        isDir: true,
                      });
                    }}
                  />
                </ContextMenuTrigger>
                <ContextMenuContent className="w-48">
                  <ContextMenuItem onSelect={handleNewFile}>
                    <FilePlus className="size-3.5" />
                    New File
                  </ContextMenuItem>
                  <ContextMenuItem onSelect={handleNewFolder}>
                    <FolderPlus className="size-3.5" />
                    New Folder
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
