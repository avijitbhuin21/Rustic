import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { parseDiff } from 'react-diff-view';
import { Diff, Hunk } from 'react-diff-view';
import { Loader2, FileText, AlertCircle, Image, File } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { ScrollArea } from '@/components/ui/scroll-area';
import { useGit } from '@/state/git';
import { useExplorer } from '@/state/explorer';
import { cn } from '@/lib/utils';
import 'react-diff-view/style/index.css';

// ── File type detection ────────────────────────────────────────────────

const IMAGE_EXTS = new Set([
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'avif', 'svg', 'tiff', 'tif',
]);

const BINARY_EXTS = new Set([
  'zip', 'tar', 'gz', 'bz2', 'xz', '7z', 'rar',
  'exe', 'dll', 'so', 'dylib', 'bin', 'obj',
  'mp3', 'mp4', 'avi', 'mov', 'mkv', 'wmv', 'flac', 'ogg', 'wav', 'aac',
  'woff', 'woff2', 'ttf', 'otf', 'eot',
  'class', 'jar', 'pyc', 'pyd',
  'sqlite', 'db', 'pdf',
]);

const MIME = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg',
  gif: 'image/gif', webp: 'image/webp', bmp: 'image/bmp',
  ico: 'image/x-icon', avif: 'image/avif', svg: 'image/svg+xml',
  tiff: 'image/tiff', tif: 'image/tiff',
};

function getExt(path) {
  const dot = path.lastIndexOf('.');
  return dot < 0 ? '' : path.slice(dot + 1).toLowerCase();
}
function isImageFile(path) { return IMAGE_EXTS.has(getExt(path)); }
function isBinaryFile(path) { return BINARY_EXTS.has(getExt(path)); }
function mimeFor(path) { return MIME[getExt(path)] ?? 'application/octet-stream'; }

// Build an absolute path from a project root + relative file path,
// preserving the OS separator used by the root.
function joinPath(root, rel) {
  if (!root) return rel;
  const sep = root.includes('\\') ? '\\' : '/';
  return root.replace(/[/\\]+$/, '') + sep + rel.replace(/^[/\\]+/, '');
}

// ── Text diff helpers (unchanged) ─────────────────────────────────────

function extractDiffText(payload) {
  if (!payload) return '';
  if (typeof payload === 'string') return payload;

  if (Array.isArray(payload.hunks)) {
    if (!payload.hunks.length) return '';
    const p = (payload.file_path ?? '').replace(/\\/g, '/');
    let text = `diff --git a/${p} b/${p}\n--- a/${p}\n+++ b/${p}\n`;
    for (const hunk of payload.hunks) {
      text += hunk.header + '\n';
      for (const line of hunk.lines ?? []) {
        // The backend parses the diff with `str::lines()`, which strips the
        // trailing newline from each line's `content`. We MUST re-add it here —
        // without the '\n' every diff line concatenates onto one physical line
        // and react-diff-view renders a single garbled row. This was the
        // "diff is weird / not how it's supposed to be" bug.
        text += `${line.origin}${line.content}\n`;
      }
    }
    return text;
  }

  return (
    payload.unified ??
    payload.diff ??
    payload.patch ??
    payload.text ??
    payload.content ??
    ''
  );
}

function ensureHeaders(diffText, path) {
  if (!diffText) return '';
  if (diffText.startsWith('diff --git') || diffText.startsWith('---')) {
    return diffText;
  }
  const p = path ?? 'file';
  return `diff --git a/${p} b/${p}\n--- a/${p}\n+++ b/${p}\n${diffText}`;
}

// ── Image diff sub-view ────────────────────────────────────────────────

function ImageDiffView({ path, projectId }) {
  const rootPath = useExplorer(
    (s) => s.projects.find((p) => p.id === projectId)?.root_path ?? ''
  );
  const [src, setSrc] = useState(null);
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);

  useEffect(() => {
    let cancelled = false;
    const absPath = joinPath(rootPath, path);
    invoke('read_file_base64', { path: absPath })
      .then((res) => {
        if (cancelled) return;
        setSrc(`data:${mimeFor(path)};base64,${res.data}`);
        setSize(res.size);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => { cancelled = true; };
  }, [rootPath, path]);

  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <Image className="size-6 opacity-40" />
        <span className="text-xs">Image unavailable</span>
        <span className="text-[10px] text-muted-foreground/60 max-w-xs text-center">{error}</span>
      </div>
    );
  }

  if (!src) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="size-4 animate-spin text-muted-foreground" />
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div
        className="flex-1 overflow-auto p-6"
        style={{
          backgroundImage: [
            'linear-gradient(45deg, hsl(var(--muted)) 25%, transparent 25%)',
            'linear-gradient(-45deg, hsl(var(--muted)) 25%, transparent 25%)',
            'linear-gradient(45deg, transparent 75%, hsl(var(--muted)) 75%)',
            'linear-gradient(-45deg, transparent 75%, hsl(var(--muted)) 75%)',
          ].join(', '),
          backgroundSize: '16px 16px',
          backgroundPosition: '0 0, 0 8px, 8px -8px, -8px 0px',
        }}
      >
        <div className="flex h-full w-full items-center justify-center">
          <img
            src={src}
            alt={path.split(/[/\\]/).pop()}
            className="max-h-full max-w-full object-contain drop-shadow-md"
          />
        </div>
      </div>
      {size != null && (
        <div className="flex h-6 shrink-0 items-center border-t border-border bg-muted/30 px-3 text-[11px] text-muted-foreground">
          <span>{(size / 1024).toFixed(1)} KB</span>
        </div>
      )}
    </div>
  );
}

// ── Binary diff sub-view ───────────────────────────────────────────────

function BinaryDiffView({ path }) {
  const ext = getExt(path);
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
      <File className="size-6 opacity-40" />
      <span className="text-xs font-medium">Binary file</span>
      {ext && (
        <span className="text-[11px] text-muted-foreground/60">
          .{ext} files cannot be displayed as a diff
        </span>
      )}
    </div>
  );
}

// ── Shared diff header ─────────────────────────────────────────────────

function DiffHeader({ path, viewType, onViewTypeChange, showToggle = true }) {
  return (
    <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-3">
      <FileText className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate text-xs">{path}</span>
      {showToggle && (
        <ToggleGroup
          type="single"
          size="sm"
          value={viewType}
          onValueChange={(v) => v && onViewTypeChange(v)}
          className="h-6"
        >
          <ToggleGroupItem value="unified" className="h-6 px-2 text-[10px]">
            Unified
          </ToggleGroupItem>
          <ToggleGroupItem value="split" className="h-6 px-2 text-[10px]">
            Split
          </ToggleGroupItem>
        </ToggleGroup>
      )}
    </div>
  );
}

// ── Main DiffView ──────────────────────────────────────────────────────

export default function DiffView({ file, projectId }) {
  const activeProjectId = useGit((s) => s.activeProjectId);
  const id = projectId ?? file?.projectId ?? activeProjectId;
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const [diffText, setDiffText] = useState('');
  const [viewType, setViewType] = useState('unified');

  const path = file?.path ?? file?.file ?? '';
  const commitOid = file?.commitOid ?? file?.oid;

  // Skip text-diff loading for image/binary files — they have their own sub-views.
  const isImage = path && isImageFile(path);
  const isBinary = path && !isImage && isBinaryFile(path);

  useEffect(() => {
    if (isImage || isBinary) return;
    let cancelled = false;
    async function load() {
      if (!id || !path) {
        setDiffText('');
        return;
      }
      setLoading(true);
      setError(null);
      try {
        let result;
        if (commitOid) {
          result = await invoke('git_commit_file_diff', {
            projectId: id,
            oid: commitOid,
            path,
          });
        } else {
          result = await invoke('git_diff', { projectId: id, path });
        }
        if (cancelled) return;
        setDiffText(extractDiffText(result));
      } catch (err) {
        if (!cancelled) setError(String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    load();
    return () => { cancelled = true; };
  }, [id, path, commitOid, isImage, isBinary]);

  const files = useMemo(() => {
    const text = ensureHeaders(diffText, path);
    if (!text) return [];
    try {
      return parseDiff(text);
    } catch (err) {
      console.error('parseDiff failed', err);
      return [];
    }
  }, [diffText, path]);

  if (!path) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <FileText className="size-6" />
        <span className="text-sm">Select a file to view its diff.</span>
      </div>
    );
  }

  // ── Image ──────────────────────────────────────────────────────────
  if (isImage) {
    return (
      <div className="flex h-full flex-col bg-background">
        <DiffHeader path={path} showToggle={false} />
        <div className="flex-1 overflow-hidden">
          <ImageDiffView path={path} projectId={id} />
        </div>
      </div>
    );
  }

  // ── Binary ─────────────────────────────────────────────────────────
  if (isBinary) {
    return (
      <div className="flex h-full flex-col bg-background">
        <DiffHeader path={path} showToggle={false} />
        <div className="flex-1 overflow-hidden">
          <BinaryDiffView path={path} />
        </div>
      </div>
    );
  }

  // ── Text diff ──────────────────────────────────────────────────────
  return (
    <div className="flex h-full flex-col bg-background">
      <DiffHeader
        path={path}
        viewType={viewType}
        onViewTypeChange={setViewType}
        showToggle
      />
      <div className="flex-1 overflow-hidden">
        {loading && (
          <div className="flex h-full items-center justify-center gap-2 text-muted-foreground">
            <Loader2 className="size-4 animate-spin" />
            <span className="text-xs">Loading diff…</span>
          </div>
        )}
        {!loading && error && (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center text-destructive">
            <AlertCircle className="size-5" />
            <span className="text-xs">{error}</span>
            <Button
              size="xs"
              variant="outline"
              onClick={() => {
                setError(null);
                setDiffText('');
              }}
            >
              Retry
            </Button>
          </div>
        )}
        {!loading && !error && files.length === 0 && (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            No changes to display.
          </div>
        )}
        {!loading && !error && files.length > 0 && (
          <ScrollArea className="h-full">
            <div className={cn('font-mono text-[12px]', 'diff-view-host')}>
              {files.map((f, i) => (
                <Diff
                  key={i}
                  viewType={viewType}
                  diffType={f.type}
                  hunks={f.hunks}
                >
                  {(hunks) =>
                    hunks.map((h) => <Hunk key={h.content} hunk={h} />)
                  }
                </Diff>
              ))}
            </div>
          </ScrollArea>
        )}
      </div>
    </div>
  );
}
