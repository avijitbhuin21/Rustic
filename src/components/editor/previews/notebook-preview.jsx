import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { toast } from 'sonner';
import CodeMirror from '@uiw/react-codemirror';
import { python } from '@codemirror/lang-python';
import { oneDark } from '@codemirror/theme-one-dark';
import { keymap } from '@codemirror/view';
import { Prec } from '@codemirror/state';
import {
  Play,
  Plus,
  Trash2,
  ArrowUp,
  ArrowDown,
  Save,
  RotateCcw,
  Loader2,
  Type,
  Code2,
  Eye,
  Pencil,
  GripVertical,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { useEditor, basename } from '@/state/editor';
import { writeTextFileScoped } from '@/lib/fs-io';
import { setActiveSaver, clearActiveSaver } from '@/lib/active-editor';

let cellSeq = 0;
const newCellId = () => `cell-${Date.now()}-${++cellSeq}`;

/** Joins nbformat's source (string | array of lines) into one string. */
function joinSource(src) {
  if (Array.isArray(src)) return src.join('');
  return typeof src === 'string' ? src : '';
}

/** Splits a source string back into nbformat's array-of-lines shape. */
function splitSource(text) {
  if (!text) return [];
  return text.split(/(?<=\n)/);
}

/** Strips ANSI escape sequences from kernel tracebacks. */
function stripAnsi(s) {
  // eslint-disable-next-line no-control-regex
  return String(s ?? '').replace(/\u001b\[[0-9;]*m/g, '');
}

function renderMarkdown(text) {
  try {
    return DOMPurify.sanitize(marked.parse(text || '', { gfm: true, breaks: true }));
  } catch {
    return DOMPurify.sanitize(text || '');
  }
}

/** Renders one nbformat output object (stream / result / display / error). */
function CellOutput({ output }) {
  if (!output) return null;
  if (output.output_type === 'stream') {
    const text = joinSource(output.text);
    return (
      <pre
        className={cn(
          'whitespace-pre-wrap break-words px-3 py-1 font-mono text-[11px] leading-relaxed',
          output.name === 'stderr' ? 'text-amber-600 dark:text-amber-400' : 'text-foreground/90',
        )}
      >
        {stripAnsi(text)}
      </pre>
    );
  }
  if (output.output_type === 'error') {
    const tb = Array.isArray(output.traceback)
      ? output.traceback.map(stripAnsi).join('\n')
      : `${output.ename}: ${output.evalue}`;
    return (
      <pre className="whitespace-pre-wrap break-words px-3 py-1 font-mono text-[11px] leading-relaxed text-destructive">
        {tb}
      </pre>
    );
  }
  if (output.output_type === 'execute_result' || output.output_type === 'display_data') {
    const data = output.data || {};
    if (data['image/png']) {
      const b64 = Array.isArray(data['image/png']) ? data['image/png'].join('') : data['image/png'];
      return (
        <div className="px-3 py-1">
          <img src={`data:image/png;base64,${b64}`} alt="output" className="max-w-full rounded" />
        </div>
      );
    }
    if (data['text/html']) {
      const html = DOMPurify.sanitize(joinSource(data['text/html']));
      return (
        <div
          className="notebook-html-output overflow-x-auto px-3 py-1 text-xs [&_table]:border-collapse [&_td]:border [&_td]:border-border [&_td]:px-2 [&_td]:py-0.5 [&_th]:border [&_th]:border-border [&_th]:bg-muted/50 [&_th]:px-2 [&_th]:py-0.5"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      );
    }
    if (data['text/plain']) {
      return (
        <pre className="whitespace-pre-wrap break-words px-3 py-1 font-mono text-[11px] leading-relaxed text-foreground/90">
          {stripAnsi(joinSource(data['text/plain']))}
        </pre>
      );
    }
  }
  return null;
}

function MarkdownCell({ cell, editing, onChange, onRequestEdit, dark }) {
  if (!editing) {
    return (
      <div
        className="prose-chat min-h-[1.5rem] cursor-text px-3 py-2 text-sm [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:text-xs [&_h1]:my-2 [&_h1]:text-xl [&_h1]:font-semibold [&_h2]:my-2 [&_h2]:text-lg [&_h2]:font-semibold [&_h3]:my-1.5 [&_h3]:text-base [&_h3]:font-semibold [&_ol]:list-decimal [&_ol]:pl-5 [&_p]:my-1 [&_pre]:my-2 [&_pre]:overflow-x-auto [&_pre]:rounded [&_pre]:bg-muted/70 [&_pre]:p-2 [&_pre]:text-xs [&_ul]:list-disc [&_ul]:pl-5"
        onDoubleClick={onRequestEdit}
        title="Double-click to edit"
        dangerouslySetInnerHTML={{
          __html: cell.source.trim()
            ? renderMarkdown(cell.source)
            : '<span style="opacity:.5;font-style:italic">Empty markdown cell — double-click or hit Edit</span>',
        }}
      />
    );
  }
  return (
    <CodeMirror
      value={cell.source}
      onChange={onChange}
      theme={dark ? oneDark : undefined}
      basicSetup={{ lineNumbers: false, foldGutter: false, highlightActiveLine: false }}
      autoFocus
      style={{ fontSize: 12 }}
    />
  );
}

export default function NotebookPreview({ tab }) {
  const [nb, setNb] = useState(null); // raw parsed notebook JSON (metadata preserved)
  const [cells, setCells] = useState(null);
  const [error, setError] = useState(null);
  const [dirty, setDirty] = useState(false);
  const [kernelState, setKernelState] = useState('stopped'); // stopped | starting | ready
  const [kernelLabel, setKernelLabel] = useState('');
  const [running, setRunning] = useState({}); // cellId -> true
  // Markdown cells with edit mode ON (cellId -> true); default is preview.
  const [mdEdit, setMdEdit] = useState({});
  // Drag-to-reorder: id of the cell being dragged + current drop target.
  const dragCellId = useRef(null);
  const [dragOverId, setDragOverId] = useState(null);
  const execCounter = useRef(0);
  const tabSetDirty = useEditor((s) => s.setDirty);
  const dark =
    typeof document !== 'undefined' && document.documentElement.classList.contains('dark');

  const notebookId = tab.path;
  const cwd = useMemo(() => {
    const idx = Math.max(tab.path.lastIndexOf('\\'), tab.path.lastIndexOf('/'));
    return idx > 0 ? tab.path.slice(0, idx) : tab.path;
  }, [tab.path]);

  // ── Load ──────────────────────────────────────────────────────────
  useEffect(() => {
    let cancelled = false;
    setError(null);
    setCells(null);
    invoke('read_file_content', { path: tab.path })
      .then((text) => {
        if (cancelled) return;
        let parsed;
        try {
          parsed = text?.trim() ? JSON.parse(text) : null;
        } catch (e) {
          setError(`Not a valid notebook: ${e.message}`);
          return;
        }
        const base = parsed || {
          nbformat: 4,
          nbformat_minor: 5,
          metadata: {},
          cells: [],
        };
        setNb(base);
        const list = (base.cells || []).map((c) => ({
          id: newCellId(),
          cell_type: c.cell_type === 'markdown' ? 'markdown' : c.cell_type === 'raw' ? 'raw' : 'code',
          source: joinSource(c.source),
          outputs: Array.isArray(c.outputs) ? c.outputs : [],
          execution_count: c.execution_count ?? null,
          metadata: c.metadata || {},
        }));
        if (list.length === 0) {
          list.push({ id: newCellId(), cell_type: 'code', source: '', outputs: [], execution_count: null, metadata: {} });
        }
        setCells(list);
        setDirty(false);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  // Mirror dirty into the tab strip's yellow dot.
  useEffect(() => {
    tabSetDirty(tab.id, dirty);
    return () => tabSetDirty(tab.id, false);
  }, [dirty, tab.id, tabSetDirty]);

  // ── Kernel events ─────────────────────────────────────────────────
  useEffect(() => {
    let unlisten;
    listen('notebook-kernel-output', (e) => {
      const ev = e.payload;
      if (!ev || ev.notebook_id !== notebookId) return;
      if (ev.kind === 'started') {
        setKernelState('ready');
        setKernelLabel(ev.message || 'python');
      } else if (ev.kind === 'exited') {
        setKernelState('stopped');
        setRunning({});
      } else if (ev.kind === 'reply' && ev.payload) {
        const r = ev.payload;
        const cellId = r.id;
        execCounter.current += 1;
        const count = execCounter.current;
        setRunning((prev) => {
          const next = { ...prev };
          delete next[cellId];
          return next;
        });
        setCells((prev) => {
          if (!prev) return prev;
          return prev.map((c) => {
            if (c.id !== cellId) return c;
            const outputs = [];
            if (r.stdout) outputs.push({ output_type: 'stream', name: 'stdout', text: splitSource(r.stdout) });
            if (r.stderr) {
              outputs.push(
                r.ok
                  ? { output_type: 'stream', name: 'stderr', text: splitSource(r.stderr) }
                  : { output_type: 'error', ename: 'Error', evalue: '', traceback: r.stderr.split('\n') },
              );
            }
            for (const img of r.images || []) {
              outputs.push({ output_type: 'display_data', data: { 'image/png': img }, metadata: {} });
            }
            if (r.result != null) {
              outputs.push({
                output_type: 'execute_result',
                execution_count: count,
                data: { 'text/plain': splitSource(r.result) },
                metadata: {},
              });
            }
            return { ...c, outputs, execution_count: count };
          });
        });
        setDirty(true);
      }
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      if (unlisten) unlisten();
    };
  }, [notebookId]);

  // Kill the kernel when the tab unmounts.
  useEffect(() => {
    return () => {
      invoke('notebook_kernel_stop', { notebookId }).catch(() => {});
    };
  }, [notebookId]);

  // ── Actions ───────────────────────────────────────────────────────
  const ensureKernel = useCallback(async () => {
    if (kernelState === 'ready') return true;
    setKernelState('starting');
    try {
      await invoke('notebook_kernel_start', { notebookId, cwd });
      setKernelState('ready');
      return true;
    } catch (e) {
      setKernelState('stopped');
      toast.error(`Could not start Python kernel: ${e}`);
      return false;
    }
  }, [kernelState, notebookId, cwd]);

  // Latest cells snapshot for event handlers/keymaps — avoids running a
  // stale closure's source when a keystroke and Run land in the same tick.
  const cellsRef = useRef(null);
  cellsRef.current = cells;

  const runCell = useCallback(
    async (cellId) => {
      const cell = (cellsRef.current || []).find((c) => c.id === cellId);
      if (!cell || cell.cell_type !== 'code' || !cell.source.trim()) return;
      if (!(await ensureKernel())) return;
      setRunning((prev) => ({ ...prev, [cell.id]: true }));
      try {
        await invoke('notebook_kernel_exec', {
          notebookId,
          cellId: cell.id,
          code: cell.source,
        });
      } catch (e) {
        setRunning((prev) => {
          const next = { ...prev };
          delete next[cell.id];
          return next;
        });
        toast.error(`Run failed: ${e}`);
      }
    },
    [ensureKernel, notebookId],
  );

  const runAll = useCallback(async () => {
    const ids = (cellsRef.current || []).map((c) => c.id);
    for (const id of ids) {
      // Sequential sends — the kernel executes stdin messages in order.
      // eslint-disable-next-line no-await-in-loop
      await runCell(id);
    }
  }, [runCell]);

  const restartKernel = useCallback(async () => {
    try {
      await invoke('notebook_kernel_stop', { notebookId });
    } catch {}
    setKernelState('stopped');
    setRunning({});
    await ensureKernel();
  }, [notebookId, ensureKernel]);

  const patchCell = (id, patch) => {
    setCells((prev) => prev.map((c) => (c.id === id ? { ...c, ...patch } : c)));
    setDirty(true);
  };

  const addCell = (afterId, cellType = 'code') => {
    const cell = { id: newCellId(), cell_type: cellType, source: '', outputs: [], execution_count: null, metadata: {} };
    setCells((prev) => {
      const idx = afterId ? prev.findIndex((c) => c.id === afterId) : prev.length - 1;
      const next = [...prev];
      next.splice(idx + 1, 0, cell);
      return next;
    });
    if (cellType === 'markdown') setMdEdit((prev) => ({ ...prev, [cell.id]: true }));
    setDirty(true);
  };

  // Drop `dragCellId` before/onto `targetId`'s position.
  const dropCellOn = (targetId) => {
    const fromId = dragCellId.current;
    dragCellId.current = null;
    setDragOverId(null);
    if (!fromId || fromId === targetId) return;
    setCells((prev) => {
      const from = prev.findIndex((c) => c.id === fromId);
      const to = prev.findIndex((c) => c.id === targetId);
      if (from < 0 || to < 0) return prev;
      const next = [...prev];
      const [c] = next.splice(from, 1);
      next.splice(to, 0, c);
      return next;
    });
    setDirty(true);
  };

  const deleteCell = (id) => {
    setCells((prev) => (prev.length > 1 ? prev.filter((c) => c.id !== id) : prev));
    setDirty(true);
  };

  const moveCell = (id, dir) => {
    setCells((prev) => {
      const idx = prev.findIndex((c) => c.id === id);
      const to = idx + dir;
      if (idx < 0 || to < 0 || to >= prev.length) return prev;
      const next = [...prev];
      const [c] = next.splice(idx, 1);
      next.splice(to, 0, c);
      return next;
    });
    setDirty(true);
  };

  const onSave = useCallback(async () => {
    if (!cells) return;
    const serialized = {
      ...(nb || { nbformat: 4, nbformat_minor: 5, metadata: {} }),
      cells: cells.map((c) => {
        const base = {
          cell_type: c.cell_type,
          metadata: c.metadata || {},
          source: splitSource(c.source),
        };
        if (c.cell_type === 'code') {
          base.outputs = c.outputs || [];
          base.execution_count = c.execution_count ?? null;
        }
        return base;
      }),
    };
    try {
      await writeTextFileScoped(tab.path, JSON.stringify(serialized, null, 1));
      setDirty(false);
      toast.success(`Saved ${basename(tab.path)}`);
    } catch (e) {
      toast.error(`Save failed: ${e?.message || e}`);
    }
  }, [cells, nb, tab.path]);

  // Ctrl+S routes here while this tab is active.
  useEffect(() => {
    setActiveSaver(onSave);
    return () => clearActiveSaver(onSave);
  }, [onSave]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }
  if (!cells) {
    return (
      <div className="flex h-full w-full flex-col gap-3 p-6">
        <Skeleton className="h-8 w-1/2" />
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-24 w-full" />
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col">
      {/* Toolbar */}
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-muted/20 px-2">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button size="icon-xs" variant="ghost" onClick={runAll} aria-label="Run all cells">
              <Play className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Run all cells</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button size="icon-xs" variant="ghost" onClick={restartKernel} aria-label="Restart kernel">
              <RotateCcw className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Restart kernel (clears state)</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button size="icon-xs" variant="ghost" onClick={onSave} aria-label="Save notebook">
              <Save className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">Save (Ctrl+S)</TooltipContent>
        </Tooltip>
        <div className="mx-1 h-4 w-px bg-border" />
        <Button
          size="sm"
          variant="ghost"
          className="h-6 gap-1 px-1.5 text-[11px] text-muted-foreground hover:text-foreground"
          onClick={() => addCell(null, 'code')}
        >
          <Plus className="size-3" /> Code
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-6 gap-1 px-1.5 text-[11px] text-muted-foreground hover:text-foreground"
          onClick={() => addCell(null, 'markdown')}
        >
          <Plus className="size-3" /> Markdown
        </Button>
        <span
          className={cn(
            'ml-2 inline-flex items-center gap-1.5 rounded px-1.5 py-0.5 text-[10px] font-medium',
            kernelState === 'ready'
              ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
              : kernelState === 'starting'
                ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                : 'bg-muted text-muted-foreground',
          )}
          title={kernelLabel || undefined}
        >
          {kernelState === 'starting' && <Loader2 className="size-3 animate-spin" />}
          {kernelState === 'ready' ? 'Kernel ready' : kernelState === 'starting' ? 'Starting…' : 'Kernel stopped'}
        </span>
        {dirty && <span className="ml-auto pr-1 text-[10px] text-yellow-500">● unsaved</span>}
      </div>

      {/* Cells */}
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-4xl flex-col gap-2 p-3 pb-24">
          {cells.map((cell, idx) => (
            <div
              key={cell.id}
              onDragOver={(e) => {
                if (dragCellId.current) {
                  e.preventDefault();
                  if (dragOverId !== cell.id) setDragOverId(cell.id);
                }
              }}
              onDragLeave={() => {
                if (dragOverId === cell.id) setDragOverId(null);
              }}
              onDrop={(e) => {
                e.preventDefault();
                dropCellOn(cell.id);
              }}
              className={cn(
                'group/cell relative rounded-md border border-border/60 bg-background',
                dragOverId === cell.id && dragCellId.current !== cell.id && 'ring-1 ring-primary',
              )}
            >
              {/* Cell header: drag handle + cell number + hover actions */}
              <div
                draggable
                onDragStart={(e) => {
                  dragCellId.current = cell.id;
                  e.dataTransfer.effectAllowed = 'move';
                }}
                onDragEnd={() => {
                  dragCellId.current = null;
                  setDragOverId(null);
                }}
                className="flex cursor-grab items-center gap-1 rounded-t-md border-b border-border/40 bg-muted/30 px-2 py-0.5 active:cursor-grabbing"
                title="Drag to reorder"
              >
                <GripVertical className="size-3 shrink-0 text-muted-foreground/40" />
                <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                  Cell {idx + 1}
                  {cell.cell_type === 'markdown' && ' · md'}
                  {cell.cell_type === 'code' && running[cell.id] && ' · running…'}
                  {cell.cell_type === 'code' && !running[cell.id] && cell.execution_count != null
                    ? ` · [${cell.execution_count}]`
                    : ''}
                </span>
                <div className="ml-auto flex items-center gap-0.5 opacity-0 transition-opacity group-hover/cell:opacity-100">
                  {cell.cell_type === 'code' ? (
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      onClick={() => runCell(cell.id)}
                      disabled={!!running[cell.id]}
                      aria-label="Run cell"
                      title="Run cell (Shift+Enter)"
                    >
                      {running[cell.id] ? (
                        <Loader2 className="size-3 animate-spin" />
                      ) : (
                        <Play className="size-3" />
                      )}
                    </Button>
                  ) : (
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      onClick={() =>
                        setMdEdit((prev) => ({ ...prev, [cell.id]: !prev[cell.id] }))
                      }
                      aria-label={mdEdit[cell.id] ? 'Preview markdown' : 'Edit markdown'}
                      title={mdEdit[cell.id] ? 'Preview' : 'Edit'}
                    >
                      {mdEdit[cell.id] ? <Eye className="size-3" /> : <Pencil className="size-3" />}
                    </Button>
                  )}
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    onClick={() =>
                      patchCell(cell.id, {
                        cell_type: cell.cell_type === 'code' ? 'markdown' : 'code',
                        outputs: [],
                        execution_count: null,
                      })
                    }
                    aria-label="Toggle cell type"
                    title={cell.cell_type === 'code' ? 'Convert to markdown' : 'Convert to code'}
                  >
                    {cell.cell_type === 'code' ? <Type className="size-3" /> : <Code2 className="size-3" />}
                  </Button>
                  <Button size="icon-xs" variant="ghost" onClick={() => moveCell(cell.id, -1)} disabled={idx === 0} aria-label="Move up" title="Move up">
                    <ArrowUp className="size-3" />
                  </Button>
                  <Button size="icon-xs" variant="ghost" onClick={() => moveCell(cell.id, 1)} disabled={idx === cells.length - 1} aria-label="Move down" title="Move down">
                    <ArrowDown className="size-3" />
                  </Button>
                  <Button size="icon-xs" variant="ghost" onClick={() => addCell(cell.id)} aria-label="Add cell below" title="Add cell below">
                    <Plus className="size-3" />
                  </Button>
                  <Button size="icon-xs" variant="ghost" onClick={() => deleteCell(cell.id)} disabled={cells.length <= 1} aria-label="Delete cell" title="Delete cell">
                    <Trash2 className="size-3" />
                  </Button>
                </div>
              </div>

              {/* Cell body */}
              {cell.cell_type === 'markdown' ? (
                <MarkdownCell
                  cell={cell}
                  dark={dark}
                  editing={!!mdEdit[cell.id]}
                  onRequestEdit={() => setMdEdit((prev) => ({ ...prev, [cell.id]: true }))}
                  onChange={(v) => patchCell(cell.id, { source: v })}
                />
              ) : (
                <CodeMirror
                  value={cell.source}
                  onChange={(v) => patchCell(cell.id, { source: v })}
                  extensions={[
                    python(),
                    // Highest precedence so Shift/Ctrl+Enter runs the cell
                    // instead of CodeMirror inserting a newline.
                    Prec.highest(
                      keymap.of([
                        {
                          key: 'Shift-Enter',
                          run: () => {
                            runCell(cell.id);
                            return true;
                          },
                        },
                        {
                          key: 'Mod-Enter',
                          run: () => {
                            runCell(cell.id);
                            return true;
                          },
                        },
                      ]),
                    ),
                  ]}
                  theme={dark ? oneDark : undefined}
                  basicSetup={{ foldGutter: false, highlightActiveLine: false }}
                  style={{ fontSize: 12 }}
                />
              )}

              {/* Outputs */}
              {cell.cell_type === 'code' && (cell.outputs?.length ?? 0) > 0 && (
                <div className="rounded-b-md border-t border-border/40 bg-muted/20">
                  {cell.outputs.map((o, i) => (
                    <CellOutput key={i} output={o} />
                  ))}
                </div>
              )}
            </div>
          ))}

          <div className="flex justify-center gap-2 pt-1">
            <Button size="sm" variant="outline" className="h-6 gap-1 text-[11px]" onClick={() => addCell(null, 'code')}>
              <Plus className="size-3" /> Code
            </Button>
            <Button size="sm" variant="outline" className="h-6 gap-1 text-[11px]" onClick={() => addCell(null, 'markdown')}>
              <Plus className="size-3" /> Markdown
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
