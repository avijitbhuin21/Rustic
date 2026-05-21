import React from 'react';
import {
  File,
  FileCode,
  FileJson,
  FileText,
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
  createFile,
  createFolder,
  deleteEntry,
  copyEntry,
  moveEntry,
  writeClipboardFiles,
  readClipboardFiles,
  pasteClipboardImageInto,
  revealInFileManager,
} from '@/state/explorer';
import { useClipboard } from '@/state/clipboard';
import { useTerminal } from '@/state/terminal';
import { confirm } from '@/components/confirm-dialog';

const EXT_ICON = {
  js: FileCode, jsx: FileCode, ts: FileCode, tsx: FileCode,
  py: FileCode, rs: FileCode, go: FileCode, java: FileCode,
  c: FileCode, cpp: FileCode, h: FileCode, hpp: FileCode,
  json: FileJson, jsonc: FileJson,
  md: FileText, txt: FileText, log: FileText,
};

function fileIcon(name) {
  const ext = name.split('.').pop()?.toLowerCase();
  return EXT_ICON[ext] ?? File;
}

export function FileNode({ node, style, dragHandle, tree }) {
  const isFolder = node.data.is_dir;
  const Icon = isFolder ? (node.isOpen ? FolderOpen : Folder) : fileIcon(node.data.name);

  const parentDir = isFolder ? node.data.path : node.data.path.replace(/[\\/][^\\/]+$/, '');

  // Visual indicator for cut items — mirror how most file managers dim them.
  const isCutItem = useClipboard(
    (s) => s.isCut && s.paths.includes(node.data.path)
  );

  const handleNewFile = async () => {
    const name = window.prompt('New file name:', '');
    if (!name) return;
    try {
      await createFile(parentDir, name);
      toast.success(`Created ${name}`);
      tree?.props?.onRefresh?.(parentDir);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleNewFolder = async () => {
    const name = window.prompt('New folder name:', '');
    if (!name) return;
    try {
      await createFolder(parentDir, name);
      toast.success(`Created ${name}`);
      tree?.props?.onRefresh?.(parentDir);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleRename = () => {
    node.edit();
  };

  const handleDelete = async () => {
    const ok = await confirm({
      title: `Delete ${node.data.name}?`,
      description: 'This cannot be undone.',
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok) return;
    try {
      await deleteEntry(node.data.path);
      toast.success(`Deleted ${node.data.name}`);
      tree?.props?.onRefresh?.(parentDir);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleCopy = async () => {
    useClipboard.getState().copy([node.data.path]);
    try {
      await writeClipboardFiles([node.data.path], false);
    } catch {
      // OS clipboard write failed; in-app clipboard still works
    }
    toast.success(`Copied "${node.data.name}"`);
  };

  const handleCut = async () => {
    useClipboard.getState().cut([node.data.path]);
    try {
      await writeClipboardFiles([node.data.path], true);
    } catch {
      // OS clipboard write failed; in-app clipboard still works
    }
    toast.success(`Cut "${node.data.name}"`);
  };

  const handlePaste = async () => {
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
    try {
      const osPaths = await readClipboardFiles();
      if (osPaths.length > 0) {
        for (const src of osPaths) {
          await copyEntry(src, parentDir);
        }
        toast.success(`Pasted ${osPaths.length} file${osPaths.length > 1 ? 's' : ''}`);
        tree?.props?.onRefresh?.(parentDir);
        return;
      }
    } catch {
      // fall through
    }

    // 3. OS clipboard image (screenshot / snipping tool / browser image copy)
    try {
      const imgPath = await pasteClipboardImageInto(parentDir);
      if (imgPath) {
        toast.success('Image pasted');
        tree?.props?.onRefresh?.(parentDir);
        return;
      }
    } catch {
      // fall through
    }

    toast.info('Nothing to paste');
  };

  const handleCopyPath = async () => {
    try {
      await navigator.clipboard.writeText(node.data.path);
      toast.success('Path copied');
    } catch {
      toast.error('Copy failed');
    }
  };

  const handleReveal = async () => {
    try {
      await revealInFileManager(node.data.path);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleOpenTerminal = async () => {
    if (!isFolder) return;
    try {
      const info = await useTerminal.getState().createTerminal({ cwd: node.data.path, label: node.data.name });
      const { useEditor } = await import('@/state/editor');
      useEditor.getState().openTerminal(info.id, info.label ?? node.data.name);
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleFileDragStart = (e) => {
    if (isFolder) return;
    e.dataTransfer.setData('application/x-rustic-file', node.data.path);
    e.dataTransfer.setData('text/plain', node.data.path);
    e.dataTransfer.effectAllowed = 'copy';
  };

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={dragHandle}
          style={style}
          draggable={!isFolder}
          onDragStart={handleFileDragStart}
          className={cn(
            'explorer-node-enter flex h-6 cursor-pointer items-center gap-1 px-1 text-xs hover:bg-muted/50',
            node.isSelected && 'bg-muted text-foreground',
            !node.isSelected && 'text-foreground/80',
            isCutItem && 'opacity-40'
          )}
          onClick={() => {
            if (isFolder) {
              node.toggle();
              return;
            }
            node.select();
            tree?.props?.onActivate?.(node);
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
          <Icon className={cn('size-3.5 shrink-0', isFolder ? 'text-primary/70' : 'text-muted-foreground')} />
          {node.isEditing ? (
            <input
              autoFocus
              defaultValue={node.data.name}
              onBlur={() => node.reset()}
              onKeyDown={(e) => {
                if (e.key === 'Enter') node.submit(e.target.value);
                if (e.key === 'Escape') node.reset();
              }}
              className="h-5 min-w-0 flex-1 rounded border border-border bg-input/30 px-1 text-xs outline-none focus:border-primary"
            />
          ) : (
            <span className="truncate">{node.data.name}</span>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-56">
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
          Delete
          <ContextMenuShortcut>Del</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={handleCopy}>
          <Copy className="size-3.5" />
          Copy
        </ContextMenuItem>
        <ContextMenuItem onSelect={handleCut}>
          <Scissors className="size-3.5" />
          Cut
        </ContextMenuItem>
        <ContextMenuItem onSelect={handlePaste}>
          <Clipboard className="size-3.5" />
          Paste
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={handleCopyPath}>
          <Copy className="size-3.5" />
          Copy Path
        </ContextMenuItem>
        <ContextMenuItem onSelect={handleReveal}>
          <ExternalLink className="size-3.5" />
          Reveal in File Manager
        </ContextMenuItem>
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
