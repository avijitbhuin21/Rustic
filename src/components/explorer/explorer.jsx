import React, { useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { FolderGit2, FolderPlus, RefreshCw, ListCollapse } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Skeleton } from '@/components/ui/skeleton';
import { useExplorer, copyEntry, moveEntry, readClipboardFiles, writeClipboardFiles } from '@/state/explorer';
import { useClipboard } from '@/state/clipboard';
import { pasteOsClipboardImageInto, uploadsAbsoluteDir } from '@/lib/clipboard-image';
import { IS_WEB } from '@/lib/platform';
import { cn } from '@/lib/utils';
import { isTypingTarget } from '@/lib/commands';
import { ProjectSection } from './project-section';
import CloneRepoDialog from './clone-repo-dialog';

export function Explorer({ onOpenFile }) {
  const projects = useExplorer((s) => s.projects);
  const activeProjectId = useExplorer((s) => s.activeProjectId);
  const loading = useExplorer((s) => s.loading);
  const error = useExplorer((s) => s.error);
  const loadProjects = useExplorer((s) => s.loadProjects);
  const addProject = useExplorer((s) => s.addProject);
  const collapseAllProjects = useExplorer((s) => s.collapseAllProjects);
  // Guard against the same Ctrl+V firing the paste pipeline more than once
  // when the keydown bubbles through React (very fast double-trigger when
  // dev-tools / extensions also listen).
  const pastingRef = useRef(false);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Resolve where a paste should land. Selection wins (folder → into that
  // folder, file → its parent dir); otherwise drop into the active project's
  // `.rustic/uploaded/` (image fallback only — file pastes without a selected
  // folder default to the active project root). Returns { dstDir, project,
  // hasSelection } or null when there's nowhere sane to put it.
  const resolvePasteDestination = () => {
    const selected = useExplorer.getState().lastSelectedNode;
    if (selected?.path) {
      const dstDir = selected.isDir
        ? selected.path
        : selected.path.replace(/[\\/][^\\/]+$/, '');
      const owner = projects.find(
        (p) => p.root_path && (dstDir === p.root_path || dstDir.startsWith(p.root_path)),
      );
      return { dstDir, project: owner || null, hasSelection: true };
    }
    const project = projects.find((p) => p.id === activeProjectId) || projects[0];
    if (!project) return null;
    return { dstDir: uploadsAbsoluteDir(project.root_path), project, hasSelection: false };
  };

  // Real browser `paste` event: this is the ONLY place with access to the
  // actual clipboard payload (`clipboardData.files`). In the web build, when
  // the user copies a file in the OS file manager and hits Ctrl+V over the
  // explorer, the file rides in here — we upload it into the target folder.
  // (A keydown handler can never see this; the OS-clipboard Tauri bridge that
  // the desktop used is a 501 no-op on the server.)
  const handleNativeFilePaste = async (e) => {
    if (!IS_WEB) return;
    const target = e.target;
    const isEditable =
      target &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable);
    if (isEditable) return;

    const files = Array.from(e.clipboardData?.files || []);
    // Some browsers expose pasted files only via `items` (kind: 'file').
    if (files.length === 0 && e.clipboardData?.items) {
      for (const it of e.clipboardData.items) {
        if (it.kind === 'file') {
          const f = it.getAsFile();
          if (f) files.push(f);
        }
      }
    }
    if (files.length === 0) {
      // No file payload. The OS likely put only a file *path* on the clipboard
      // (common for file-manager copies), which the browser sandbox forbids JS
      // from reading. Tell the user how to get the bytes across.
      const hasText = Array.from(e.clipboardData?.types || []).includes('text/plain');
      if (hasText) {
        toast.info('Paste delivered a path, not file data. Use right-click → Upload, or drag the file in.');
        e.preventDefault();
      }
      return; // let the keydown handler attempt the in-app clipboard otherwise
    }

    e.preventDefault();
    e.stopPropagation();

    const dest = resolvePasteDestination();
    const dstDir = dest.hasSelection
      ? dest.dstDir
      : (projects.find((p) => p.id === activeProjectId) || projects[0])?.root_path;
    if (!dstDir) {
      toast.error('Open a project or select a folder before pasting.');
      return;
    }

    const displayPath = (p) =>
      dest.project?.root_path
        ? p.replace(dest.project.root_path, '').replace(/^[\\/]+/, '') || p
        : p;

    try {
      const { uploadFileList } = await import('@/lib/file-transfer');
      const count = await uploadFileList(dstDir, files);
      toast.success(`Pasted ${count} file${count > 1 ? 's' : ''} to ${displayPath(dstDir)}`);
    } catch (err) {
      toast.error(`Paste failed: ${err?.message || err}`);
    }
  };

  const handlePasteShortcut = async (e) => {
    if (e.defaultPrevented) return;

    // F2 (rename) and Delete (delete file) must never fire while the user is
    // typing — in the inline rename input, the code editor, or any text field.
    // Otherwise Delete wipes the selected file instead of deleting a character,
    // and F2 fights the rename box. The global keybinding-bridge owns these for
    // the non-typing case (a focused tree row), so we simply bow out here.
    const typing = isTypingTarget(e.target);

    // Handle F2 for rename
    if (e.key === 'F2' && !typing) {
      e.preventDefault();
      window.dispatchEvent(new CustomEvent('rustic:explorer-rename'));
      return;
    }

    // Handle Delete key
    if (e.key === 'Delete' && !typing) {
      e.preventDefault();
      window.dispatchEvent(new CustomEvent('rustic:explorer-delete'));
      return;
    }

    const target = e.target;
    const isEditable =
      target &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable);

    // Ctrl/Cmd+C and Ctrl/Cmd+X: copy/cut the selected node into the in-app
    // clipboard. Without this, keyboard copy did nothing in the browser (there
    // is no OS-clipboard bridge on web), so a later paste found an empty
    // clipboard and reported "Nothing to paste".
    const isCopyKey =
      (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && (e.key === 'c' || e.key === 'C');
    const isCutKey =
      (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && (e.key === 'x' || e.key === 'X');
    if ((isCopyKey || isCutKey) && !isEditable) {
      const st = useExplorer.getState();
      // Multi-selection (Ctrl/Shift-click) wins over the single last-clicked
      // node, so Ctrl+C after selecting 5 files copies all 5.
      const selItems = st.selection?.items ?? [];
      const paths = selItems.length > 1
        ? selItems.map((it) => it.path)
        : st.lastSelectedNode?.path
          ? [st.lastSelectedNode.path]
          : [];
      if (paths.length === 0) return; // nothing selected → let the browser handle it
      e.preventDefault();
      if (isCutKey) useClipboard.getState().cut(paths);
      else useClipboard.getState().copy(paths);
      // Also put a real file list on the OS clipboard (desktop) so the copy
      // pastes into Windows Explorer / Finder too — same as the context menu.
      if (!IS_WEB) {
        writeClipboardFiles(paths, isCutKey).catch(() => {});
      }
      toast.success(`${isCutKey ? 'Cut' : 'Copied'} ${paths.length} item${paths.length > 1 ? 's' : ''}`);
      return;
    }

    const isPaste =
      (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && (e.key === 'v' || e.key === 'V');
    if (!isPaste) return;
    // Don't hijack paste when the user is typing in an editable element that
    // the keydown might bubble through (renaming a file, the search box, etc).
    if (isEditable) {
      return;
    }
    const dest = resolvePasteDestination();
    if (!dest) {
      toast.error('Add a project before pasting.');
      return;
    }
    if (pastingRef.current) return;
    pastingRef.current = true;
    e.preventDefault();

    // For file pastes, the destination must be a real folder. If nothing's
    // selected we fall back to the active project root rather than the image
    // uploads dir, so Ctrl+V on the explorer pane behaves like a regular file
    // manager paste.
    const fileDstDir = dest.hasSelection
      ? dest.dstDir
      : (projects.find((p) => p.id === activeProjectId) || projects[0])?.root_path || dest.dstDir;

    const displayPath = (p) =>
      dest.project?.root_path
        ? p.replace(dest.project.root_path, '').replace(/^[\\/]+/, '') || p
        : p;

    try {
      // 1. In-app clipboard (Copy/Cut from a Rustic context menu)
      const { paths: clipPaths, isCut, clear } = useClipboard.getState();
      if (clipPaths.length > 0) {
        for (const src of clipPaths) {
          if (isCut) await moveEntry(src, fileDstDir);
          else await copyEntry(src, fileDstDir);
        }
        if (isCut) clear();
        const label = isCut ? 'Moved' : 'Pasted';
        toast.success(`${label} ${clipPaths.length} item${clipPaths.length > 1 ? 's' : ''} to ${displayPath(fileDstDir)}`);
        return;
      }

      // 2. OS clipboard file list (files copied from VS Code, Windows
      // Explorer, Finder, etc.) — desktop only; the web build has no
      // OS-clipboard bridge (those commands 501).
      let osClipboardError = null;
      if (!IS_WEB) {
        // Read and copy are SEPARATE failure domains: a read failure falls
        // through to the image attempt; a copy/move failure must surface as
        // "Paste failed" (the outer catch), not get mislabeled as a
        // clipboard problem.
        let osDrop = null;
        try {
          osDrop = await readClipboardFiles();
          console.log('[explorer] OS clipboard file list:', osDrop?.paths, 'cut:', osDrop?.cut);
        } catch (err) {
          // Don't swallow this — it's exactly the error that made real paste
          // failures look like an empty clipboard ("nothing to paste").
          console.error('[explorer] OS clipboard file read failed:', err);
          osClipboardError = err;
        }
        if (osDrop?.paths?.length > 0) {
          for (const src of osDrop.paths) {
            if (osDrop.cut) await moveEntry(src, fileDstDir);
            else await copyEntry(src, fileDstDir);
          }
          const label = osDrop.cut ? 'Moved' : 'Pasted';
          toast.success(`${label} ${osDrop.paths.length} file${osDrop.paths.length > 1 ? 's' : ''} to ${displayPath(fileDstDir)}`);
          return;
        }

        // 3. OS clipboard image (screenshot, snipping tool, browser image copy)
        const saved = await pasteOsClipboardImageInto(dest.dstDir);
        if (saved) {
          toast.success(`Saved to ${displayPath(saved)}`);
          return;
        }
      }
      if (osClipboardError) {
        const msg = osClipboardError?.message || String(osClipboardError);
        toast.error(`Clipboard read failed: ${msg}`);
      } else {
        toast.info('Nothing to paste.');
      }
    } catch (err) {
      const msg = typeof err === 'string' ? err : err?.message || String(err);
      toast.error(`Paste failed: ${msg}`);
    } finally {
      pastingRef.current = false;
    }
    // The FS watcher emits `rustic:fs-change` which refreshes only the
    // affected parent directory. We deliberately do NOT dispatch the nuclear
    // `rustic:tree-refresh` — that one clears the children cache and
    // collapses every expanded folder.
  };

  // External-file drop catch-all for the explorer body. Folder rows handle
  // their own drops (into that folder); anything that bubbles here — empty
  // space, the skeleton, the project header gaps — lands in the active
  // project's root. Internal tree drags (no 'Files' type) are ignored so the
  // row-level move logic keeps working.
  const handleExternalDragOver = (e) => {
    const types = Array.from(e.dataTransfer?.types || []);
    if (!types.includes('Files')) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  };
  const handleExternalDrop = async (e) => {
    const files = Array.from(e.dataTransfer?.files || []);
    if (files.length === 0) return;
    e.preventDefault();
    e.stopPropagation();
    const project = projects.find((p) => p.id === activeProjectId) || projects[0];
    if (!project?.root_path) {
      toast.error('Add a project before dropping files.');
      return;
    }
    try {
      const count = IS_WEB
        ? await (await import('@/lib/file-transfer')).uploadFileList(project.root_path, files)
        : await (await import('@/lib/desktop-upload')).uploadDroppedFiles(project.root_path, files);
      toast.success(`Added ${count} file${count > 1 ? 's' : ''} to the project root`);
    } catch (err) {
      console.error('[explorer] drop upload failed:', err);
      toast.error(`Drop failed: ${err?.message || err}`);
    }
  };

  const handleAddProject = async () => {
    try {
      const path = await open({ directory: true, multiple: false });
      if (typeof path === 'string') await addProject(path);
    } catch (err) {
      console.error('add project failed:', err);
    }
  };

  const handleCollapseAll = () => collapseAllProjects();

  const [cloneDialogOpen, setCloneDialogOpen] = useState(false);
  const [spinning, setSpinning] = useState(false);
  const handleRefresh = async () => {
    setSpinning(true);
    const minDelay = new Promise((r) => setTimeout(r, 700));
    try {
      // Refresh both the project list AND the file tree contents. Previously
      // this only reloaded the project list — so after the agent reverted a
      // file, the tree showed stale entries and clicking Refresh did nothing
      // visible. The window event is picked up by every mounted <FileTree>
      // which drops its cache and re-fetches.
      window.dispatchEvent(new CustomEvent('rustic:tree-refresh'));
      await Promise.all([loadProjects(), minDelay]);
    } finally {
      setSpinning(false);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          Explorer
        </span>
        <div className="flex items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={handleAddProject}>
                <FolderPlus className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" align="start" sideOffset={4} className="px-2 py-1">Add Project Folder</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={() => setCloneDialogOpen(true)}>
                <FolderGit2 className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" align="start" sideOffset={4} className="px-2 py-1">Clone Repository</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={handleCollapseAll}>
                <ListCollapse className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">Collapse All</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={handleRefresh}
                disabled={spinning}
              >
                <RefreshCw className={cn('size-3', spinning && 'animate-spin')} />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" align="end" sideOffset={4} className="px-2 py-1">Refresh</TooltipContent>
          </Tooltip>
        </div>
      </div>
      <div
        className="explorer-scroll min-h-0 flex-1 overflow-y-auto overflow-x-hidden outline-none"
        // tabIndex makes the pane focusable so Ctrl+V keystrokes land here when
        // the user clicks empty space inside the explorer. The handler routes
        // pasted screenshots into the active project's .rustic/uploaded folder.
        tabIndex={0}
        onKeyDown={handlePasteShortcut}
        onPaste={handleNativeFilePaste}
        onDragOver={handleExternalDragOver}
        onDrop={handleExternalDrop}
      >
        {loading && projects.length === 0 && (
          <div className="flex flex-col gap-1 px-2 py-2">
            <Skeleton className="h-5 w-3/4" />
            <Skeleton className="ml-3 h-4 w-2/3" />
            <Skeleton className="ml-3 h-4 w-1/2" />
            <Skeleton className="ml-3 h-4 w-4/5" />
            <Skeleton className="h-5 w-2/3" />
            <Skeleton className="ml-3 h-4 w-1/2" />
          </div>
        )}
        {error && (
          <div className="flex flex-col items-start gap-2 px-3 py-4 text-xs">
            <span className="text-destructive">Error: {error}</span>
            <Button variant="ghost" size="sm" onClick={handleRefresh} disabled={spinning}>
              <RefreshCw className={cn('size-3', spinning && 'animate-spin')} />
              Retry
            </Button>
          </div>
        )}
        {!loading && projects.length === 0 && !error && (
          <div className="flex flex-col items-start gap-2 px-3 py-4 text-xs text-muted-foreground">
            <span>No projects added yet.</span>
            <Button variant="outline" size="sm" onClick={handleAddProject}>
              <FolderPlus className="size-3" />
              Add Folder
            </Button>
          </div>
        )}
        {projects.map((p) => (
          <ProjectSection key={p.id} project={p} onOpenFile={onOpenFile} />
        ))}
      </div>
      <CloneRepoDialog open={cloneDialogOpen} onOpenChange={setCloneDialogOpen} />
    </div>
  );
}
