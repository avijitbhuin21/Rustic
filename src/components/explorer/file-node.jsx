import React from 'react';
import { getIcon } from 'material-file-icons';
import {
  Folder,
  FolderOpen,
  ChevronRight,
  FilePlus,
  FolderPlus,
  Pencil,
  Trash2,
  Copy,
  Scissors,
  Clipboard,
  TerminalSquare,
  ExternalLink,
  Download,
  Upload,
} from 'lucide-react';
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
} from '@/components/ui/context-menu';
import { cn } from '@/lib/utils';
import { toast } from 'sonner';
import {
  deleteEntry,
  copyEntry,
  moveEntry,
  writeClipboardFiles,
  readClipboardFiles,
  pasteClipboardImageInto,
  revealInFileManager,
} from '@/state/explorer';
import { useClipboard } from '@/state/clipboard';
import { useExplorer } from '@/state/explorer';
import { useTerminal } from '@/state/terminal';
import { useGit, gitDecorationsFor } from '@/state/git';
import { IS_WEB } from '@/lib/platform';
import {
  downloadPath,
  pickAndUploadFiles,
  pickAndUploadFolder,
} from '@/lib/file-transfer';
import { confirm } from '@/components/confirm-dialog';
import { contextMenuState } from './context-menu-state';

const GIT_TINT = {
  M: 'text-yellow-500',
  A: 'text-emerald-500',
  D: 'text-red-500',
  R: 'text-blue-500',
  U: 'text-orange-500',
  '?': 'text-emerald-400',
};

const GIT_LABEL = {
  M: 'Modified',
  A: 'Added',
  D: 'Deleted',
  R: 'Renamed',
  U: 'Conflict',
  '?': 'Untracked',
};

export function FileNode({ node, style, dragHandle, tree }) {
  const isFolder = node.data.is_dir;
  // Folders keep the monochrome lucide glyph (tints with the theme); files get
  // the colored Material file-type logo (Python/TS/Rust/etc.) via material-file-icons,
  // which inlines a self-sizing SVG string — no asset pipeline needed.
  const FolderIcon = node.isOpen ? FolderOpen : Folder;
  const [dragOver, setDragOver] = React.useState(false);
  // True for the brief window between an Enter/Escape keypress and the blur it
  // triggers, so the blur handler knows the edit was already resolved and skips
  // its commit-on-blur logic (otherwise Escape-to-cancel would commit instead).
  const renameResolvedRef = React.useRef(false);
  // Set when a context-menu action is about to spawn an inline edit (Rename /
  // New File / New Folder). Radix restores focus to the trigger row only after
  // its ~100ms exit animation — well after the setTimeout(0) below has mounted
  // the edit input — so that focus restore blurred the input and cancelled the
  // edit. While this flag is set, onCloseAutoFocus is prevented instead.
  const editPendingRef = React.useRef(false);

  const parentDir = isFolder ? node.data.path : node.data.path.replace(/[\\/][^\\/]+$/, '');

  // Visual indicator for cut items — mirror how most file managers dim them.
  const isCutItem = useClipboard(
    (s) => s.isCut && s.paths.includes(node.data.path)
  );

  // Multi-selection support: when this row is part of a Ctrl/Shift-click
  // selection of 2+ items, the context-menu copy/cut/delete act on the whole
  // set instead of just this row.
  const selectionItems = useExplorer((s) => s.selection.items);
  const inMultiSelection =
    selectionItems.length > 1 && selectionItems.some((it) => it.path === node.data.path);
  const selCount = inMultiSelection ? selectionItems.length : 1;

  const gitProjectId = tree?.props?.gitProjectId ?? null;
  const treeRootPath = tree?.props?.rootPath ?? '';
  const relPath = React.useMemo(() => {
    if (!treeRootPath) return null;
    const p = node.data.path.replace(/\\/g, '/');
    const root = treeRootPath.replace(/\\/g, '/').replace(/\/+$/, '');
    return p.startsWith(root + '/') ? p.slice(root.length + 1) : null;
  }, [treeRootPath, node.data.path]);
  const gitStatus = useGit((s) => {
    if (!gitProjectId || !relPath) return null;
    const deco = gitDecorationsFor(s.projects[gitProjectId]?.status);
    if (!deco) return null;
    if (isFolder) return deco.dirs.has(relPath) ? 'dir' : null;
    return deco.files.get(relPath) ?? null;
  });

  const handleNewFile = () => {
    contextMenuState.suppressActivate();
    editPendingRef.current = true;
    // Defer so Radix's context-menu close runs first before the edit input
    // mounts; editPendingRef then keeps the post-close focus restore from
    // stealing focus back to the row.
    setTimeout(() => {
      tree?.props?.onCreateAndEdit?.(parentDir, 'file');
    }, 0);
  };

  const handleNewFolder = () => {
    contextMenuState.suppressActivate();
    editPendingRef.current = true;
    setTimeout(() => {
      tree?.props?.onCreateAndEdit?.(parentDir, 'folder');
    }, 0);
  };

  const handleRename = () => {
    contextMenuState.suppressActivate();
    editPendingRef.current = true;
    setTimeout(() => node.edit(), 0);
  };

  const handleDelete = async () => {
    contextMenuState.suppressActivate();

    if (inMultiSelection) {
      const norm = (p) => p.replace(/\\/g, '/');
      // Skip items nested inside another selected folder — deleting the
      // folder already removes them.
      const folderPaths = selectionItems.filter((it) => it.isDir).map((it) => norm(it.path));
      const items = selectionItems.filter((it) => {
        const p = norm(it.path);
        return !folderPaths.some((f) => f !== p && p.startsWith(f + '/'));
      });
      const ok = await confirm({
        title: `Delete ${items.length} items?`,
        description: 'This cannot be undone.',
        confirmLabel: 'Delete',
        destructive: true,
      });
      if (!ok) return;
      const parents = new Set();
      let failed = 0;
      for (const it of items) {
        try {
          await deleteEntry(it.path);
          parents.add(it.path.replace(/[\\/][^\\/]+$/, ''));
        } catch (e) {
          failed += 1;
          console.error('explorer: delete failed', it.path, e);
        }
      }
      for (const p of parents) tree?.props?.onRefresh?.(p);
      if (failed) toast.error(`Deleted ${items.length - failed} item${items.length - failed === 1 ? '' : 's'}, ${failed} failed`);
      else toast.success(`Deleted ${items.length} items`);
      useExplorer.getState().clearSelection();
      try { tree?.deselectAll?.(); } catch {}
      return;
    }

    const ok = await confirm({
      title: `Delete ${node.data.name}?`,
      description: 'This cannot be undone.',
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok) return;
    const runDelete = async () => {
      try {
        await deleteEntry(node.data.path);
        toast.success(`Deleted ${node.data.name}`);
        tree?.props?.onRefresh?.(parentDir);
      } catch (e) {
        toast.error(String(e), { action: { label: 'Retry', onClick: runDelete } });
      }
    };
    await runDelete();
  };

  const handleCopy = async () => {
    contextMenuState.suppressActivate();
    const paths = inMultiSelection
      ? selectionItems.map((it) => it.path)
      : [node.data.path];
    useClipboard.getState().copy(paths);
    if (!IS_WEB) {
      try {
        await writeClipboardFiles(paths, false);
      } catch {
        // OS clipboard write failed; in-app clipboard still works
      }
    }
  };

  const handleCut = async () => {
    contextMenuState.suppressActivate();
    const paths = inMultiSelection
      ? selectionItems.map((it) => it.path)
      : [node.data.path];
    useClipboard.getState().cut(paths);
    if (!IS_WEB) {
      try {
        await writeClipboardFiles(paths, true);
      } catch {
        // OS clipboard write failed; in-app clipboard still works
      }
    }
  };

  const handlePaste = async () => {
    contextMenuState.suppressActivate();
    const { paths, isCut, clear } = useClipboard.getState();

    // 1. In-app clipboard (most reliable — covers copy & cut within the app)
    if (paths.length > 0) {
      try {
        for (const src of paths) {
          if (isCut) {
            await moveEntry(src, parentDir);
          } else {
            await copyEntry(src, parentDir);
          }
        }
        if (isCut) clear();
        const label = isCut ? 'Moved' : 'Pasted';
        toast.success(`${label} ${paths.length} item${paths.length > 1 ? 's' : ''}`);
        tree?.props?.onRefresh?.(parentDir);
        return;
      } catch (e) {
        toast.error(String(e));
        return;
      }
    }

    // 2. OS clipboard file list (files copied from Windows Explorer / Finder)
    // Desktop only — the web build has no OS-clipboard bridge (those commands
    // return 501), so skip straight to "nothing to paste".
    let osClipboardError = null;
    if (!IS_WEB) {
      // Read and copy are SEPARATE failure domains: a read failure falls
      // through to the image attempt; a copy/move failure surfaces as
      // "Paste failed", not as a clipboard problem.
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
        try {
          for (const src of osDrop.paths) {
            if (osDrop.cut) await moveEntry(src, parentDir);
            else await copyEntry(src, parentDir);
          }
          const label = osDrop.cut ? 'Moved' : 'Pasted';
          toast.success(`${label} ${osDrop.paths.length} file${osDrop.paths.length > 1 ? 's' : ''}`);
          tree?.props?.onRefresh?.(parentDir);
        } catch (e) {
          toast.error(`Paste failed: ${e?.message || e}`);
        }
        return;
      }

      // 3. OS clipboard image (screenshot / snipping tool / browser image copy)
      try {
        const imgPath = await pasteClipboardImageInto(parentDir);
        if (imgPath) {
          toast.success('Image pasted');
          tree?.props?.onRefresh?.(parentDir);
          return;
        }
      } catch (err) {
        console.error('[explorer] clipboard image paste failed:', err);
      }
    }

    if (osClipboardError) {
      toast.error(`Clipboard read failed: ${osClipboardError?.message || osClipboardError}`);
    } else {
      toast.info('Nothing to paste');
    }
  };

  const handleCopyPath = async () => {
    contextMenuState.suppressActivate();
    try {
      await navigator.clipboard.writeText(node.data.path);
    } catch {
      toast.error('Copy failed');
    }
  };

  const handleReveal = async () => {
    contextMenuState.suppressActivate();
    try {
      await revealInFileManager(node.data.path);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleOpenTerminal = async () => {
    contextMenuState.suppressActivate();
    if (!isFolder) return;
    try {
      const info = await useTerminal.getState().createTerminal({ cwd: node.data.path, label: node.data.name });
      const { useEditor } = await import('@/state/editor');
      useEditor.getState().openTerminal(info.id, info.label ?? node.data.name);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleDownload = async () => {
    contextMenuState.suppressActivate();
    try {
      const t = toast.loading(
        isFolder ? `Zipping "${node.data.name}"…` : `Downloading "${node.data.name}"…`
      );
      await downloadPath(node.data.path);
      toast.dismiss(t);
      toast.success(isFolder ? `Downloaded "${node.data.name}.zip"` : `Downloaded "${node.data.name}"`);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleUpload = async (folder) => {
    contextMenuState.suppressActivate();
    // Files upload into this folder; for a file node, into its parent dir.
    const dstDir = isFolder ? node.data.path : parentDir;
    try {
      const count = folder
        ? await pickAndUploadFolder(dstDir)
        : await pickAndUploadFiles(dstDir);
      if (count > 0) {
        toast.success(`Uploaded ${count} item${count > 1 ? 's' : ''}`);
        tree?.props?.onRefresh?.(dstDir);
      }
    } catch (e) {
      toast.error(String(e));
    }
  };

  // Both files and folders are draggable. The path is carried for two
  // consumers: the editor (drop a file into Monaco to open it) and a folder
  // row (drop onto it to MOVE the dragged entry inside). `copyMove` lets the
  // editor treat it as a copy and a folder treat it as a move.
  const handleDragStart = (e) => {
    e.dataTransfer.setData('application/x-rustic-file', node.data.path);
    e.dataTransfer.setData('text/plain', node.data.path);
    e.dataTransfer.effectAllowed = 'copyMove';
  };

  // Folders are drop targets for move; EVERY row is a drop target for
  // external OS files (a file row routes them into its parent folder).
  // Guard against dropping an item onto itself / its own parent (no-op) or a
  // folder into its own descendant (would orphan it) — the destination guard
  // also lives in the tree handler.
  const handleDragOver = (e) => {
    // OS files dragged in from outside report no internal-move type; show a
    // copy cursor for those and a move cursor for in-app drags.
    const hasExternalFiles = Array.from(e.dataTransfer.types || []).includes('Files');
    if (!isFolder && !hasExternalFiles) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = hasExternalFiles ? 'copy' : 'move';
    if (!dragOver) setDragOver(true);
  };
  const handleDragLeave = () => {
    if (dragOver) setDragOver(false);
  };
  const handleDrop = async (e) => {
    // External OS files: upload into this folder (or a file row's parent).
    // Desktop gets bytes-only File objects (Tauri native drag-drop is off for
    // the WebView2 HTML5-DnD fix), so it streams them via desktop-upload;
    // web uses the HTTP upload transport.
    const osFiles = Array.from(e.dataTransfer.files || []);
    if (osFiles.length > 0) {
      e.preventDefault();
      e.stopPropagation();
      setDragOver(false);
      const dstDir = isFolder ? node.data.path : parentDir;
      try {
        const count = IS_WEB
          ? await (await import('@/lib/file-transfer')).uploadFileList(dstDir, osFiles)
          : await (await import('@/lib/desktop-upload')).uploadDroppedFiles(dstDir, osFiles);
        toast.success(`Uploaded ${count} item${count > 1 ? 's' : ''}`);
        tree?.props?.onRefresh?.(dstDir);
      } catch (err) {
        console.error('[explorer] drop upload failed:', err);
        toast.error(String(err?.message || err));
      }
      return;
    }

    if (!isFolder) return;
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);

    // In-app move (drag an explorer node onto a folder).
    const src =
      e.dataTransfer.getData('application/x-rustic-file') ||
      e.dataTransfer.getData('text/plain');
    if (!src) return;
    tree?.props?.onMoveEntry?.(src, node.data.path);
  };

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={dragHandle}
          style={style}
          draggable
          onDragStart={handleDragStart}
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
          onDrop={handleDrop}
          data-explorer-node={isFolder ? 'folder' : 'file'}
          className={cn(
            'explorer-node-enter flex h-6 cursor-pointer items-center gap-1 px-1 text-xs hover:bg-muted/50',
            node.isSelected && 'bg-muted text-foreground',
            !node.isSelected && 'text-foreground/80',
            isCutItem && 'opacity-40',
            dragOver && 'bg-primary/15 ring-1 ring-inset ring-primary/40'
          )}
          onClick={(e) => {
            // Ctrl/Cmd-click toggles this row in/out of the multi-selection,
            // Shift-click extends the range from the last anchor — neither
            // opens files nor toggles folders. stopPropagation keeps
            // react-arborist's row-level handleClick (which only understands
            // metaKey, so Ctrl never worked on Windows) from double-handling.
            if (e.ctrlKey || e.metaKey || e.shiftKey) {
              e.stopPropagation();
              if (e.shiftKey) node.selectContiguous();
              else if (node.isSelected) node.deselect();
              else node.selectMulti();
              useExplorer.getState().setLastSelectedNode({
                path: node.data.path,
                isDir: !!isFolder,
              });
              tree?.props?.onNodeClick?.(node);
              return;
            }
            useExplorer.getState().setLastSelectedNode({
              path: node.data.path,
              isDir: !!isFolder,
            });
            tree?.props?.onNodeClick?.(node);
            if (isFolder) {
              // Select before toggling so a plain folder click collapses any
              // multi-selection (file-manager convention) and sets the anchor
              // for a subsequent Shift-click range.
              node.select();
              node.toggle();
              return;
            }
            node.select();
            tree?.props?.onActivate?.(node);
          }}
          onContextMenu={() => {
            // Right-click counts as selection for paste-destination purposes:
            // users frequently right-click a folder to open the context menu
            // and never left-click it before hitting Ctrl+V.
            useExplorer.getState().setLastSelectedNode({
              path: node.data.path,
              isDir: !!isFolder,
            });
            // Also notify the tree for F2/Delete shortcuts
            tree?.props?.onNodeClick?.(node);
          }}
        >
          <span className="flex w-4 items-center justify-center">
            {isFolder ? (
              <ChevronRight
                className="size-3 text-muted-foreground transition-transform duration-200 ease-in-out"
                style={{ transform: node.isOpen ? 'rotate(90deg)' : 'rotate(0deg)' }}
              />
            ) : null}
          </span>
          {isFolder ? (
            <FolderIcon className="size-3.5 shrink-0 text-primary/70" />
          ) : (
            <span
              className="inline-flex size-3.5 shrink-0 items-center justify-center"
              dangerouslySetInnerHTML={{ __html: getIcon(node.data.name).svg }}
            />
          )}
          {node.isEditing ? (
            <input
              autoFocus
              defaultValue={node.data.name}
              onFocus={(e) => {
                // Fresh edit: clear the Enter/Escape latch so a stale value
                // from a previous rename can't suppress this one's blur commit.
                renameResolvedRef.current = false;
                // Select the stem (or whole name for folders / dotfiles) so
                // typing replaces the placeholder name from createAndEdit
                // but leaves the extension alone when the user just wants
                // to rename a file without retyping `.txt` etc.
                const v = e.currentTarget.value;
                const dot = v.lastIndexOf('.');
                if (!isFolder && dot > 0) {
                  e.currentTarget.setSelectionRange(0, dot);
                } else {
                  e.currentTarget.select();
                }
              }}
              onBlur={(e) => {
                // Belt-and-suspenders for the fs-change refresh guard in
                // file-tree.jsx: if a stray re-render ever blurs this input
                // mid-rename, COMMIT what the user typed instead of silently
                // discarding it (VS Code-style commit-on-blur). Enter/Escape
                // already resolved the edit, so skip then. An empty or
                // unchanged value is "no rename" → reset.
                if (renameResolvedRef.current) {
                  renameResolvedRef.current = false;
                  return;
                }
                const value = e.target.value.trim();
                if (value && value !== node.data.name) {
                  node.submit(value);
                } else {
                  node.reset();
                }
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  renameResolvedRef.current = true;
                  node.submit(e.target.value);
                }
                if (e.key === 'Escape') {
                  renameResolvedRef.current = true;
                  node.reset();
                }
              }}
              className="h-5 min-w-0 flex-1 rounded border border-border bg-input/30 px-1 text-xs outline-none focus:border-primary"
            />
          ) : (
            <span className={cn('truncate', !isFolder && gitStatus && GIT_TINT[gitStatus])}>
              {node.data.name}
            </span>
          )}
          {!node.isEditing && !isFolder && gitStatus && (
            <span
              className={cn(
                'ml-auto w-4 shrink-0 pr-0.5 text-center font-mono text-[10px] font-semibold',
                GIT_TINT[gitStatus]
              )}
              title={GIT_LABEL[gitStatus]}
            >
              {gitStatus}
            </span>
          )}
          {!node.isEditing && isFolder && gitStatus && (
            <span
              className="ml-auto mr-1.5 size-1.5 shrink-0 rounded-full bg-yellow-500/60"
              title="Contains changes"
            />
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent
        className="w-56"
        onCloseAutoFocus={(e) => {
          if (editPendingRef.current) {
            editPendingRef.current = false;
            e.preventDefault();
          }
        }}
      >
        {isFolder && (
          <>
            <ContextMenuItem onSelect={handleNewFile}>
              <FilePlus className="size-3.5" />
              New File
            </ContextMenuItem>
            <ContextMenuItem onSelect={handleNewFolder}>
              <FolderPlus className="size-3.5" />
              New Folder
            </ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        <ContextMenuItem onSelect={handleRename}>
          <Pencil className="size-3.5" />
          Rename
          <ContextMenuShortcut>F2</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuItem onSelect={handleDelete} variant="destructive">
          <Trash2 className="size-3.5" />
          Delete{inMultiSelection ? ` (${selCount})` : ''}
          <ContextMenuShortcut>Del</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={handleCopy}>
          <Copy className="size-3.5" />
          Copy{inMultiSelection ? ` (${selCount})` : ''}
        </ContextMenuItem>
        <ContextMenuItem onSelect={handleCut}>
          <Scissors className="size-3.5" />
          Cut{inMultiSelection ? ` (${selCount})` : ''}
        </ContextMenuItem>
        <ContextMenuItem onSelect={handlePaste}>
          <Clipboard className="size-3.5" />
          Paste
        </ContextMenuItem>
        <ContextMenuSeparator />
        {IS_WEB && (
          <>
            <ContextMenuItem onSelect={handleDownload}>
              <Download className="size-3.5" />
              Download{isFolder ? ' (zip)' : ''}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => handleUpload(false)}>
              <Upload className="size-3.5" />
              Upload Files{isFolder ? '' : ' Here'}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => handleUpload(true)}>
              <Upload className="size-3.5" />
              Upload Folder
            </ContextMenuItem>
            <ContextMenuSeparator />
          </>
        )}
        <ContextMenuItem onSelect={handleCopyPath}>
          <Copy className="size-3.5" />
          Copy Path
        </ContextMenuItem>
        {!IS_WEB && (
          <ContextMenuItem onSelect={handleReveal}>
            <ExternalLink className="size-3.5" />
            Reveal in File Manager
          </ContextMenuItem>
        )}
        {isFolder && (
          <ContextMenuItem onSelect={handleOpenTerminal}>
            <TerminalSquare className="size-3.5" />
            Open in Terminal
          </ContextMenuItem>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}
