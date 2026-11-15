import React, { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Play, Table2, Eye, KeyRound } from 'lucide-react';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { basename } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

const PAGE = 200;

function cellText(v) {
  if (v === null || v === undefined) return 'NULL';
  return typeof v === 'string' ? v : String(v);
}

/** Paged result grid. */
function Grid({ data, onPage }) {
  if (!data) return null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-max min-w-full border-collapse text-[12px]">
          <thead className="sticky top-0 bg-muted">
            <tr>
              <th className="border-b border-r border-border px-2 py-1 text-right font-normal text-muted-foreground">#</th>
              {data.columns.map((c) => (
                <th key={c} className="border-b border-r border-border px-2 py-1 text-left font-medium">{c}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {data.rows.map((row, i) => (
              <tr key={i} className="hover:bg-muted/40">
                <td className="border-b border-r border-border px-2 py-0.5 text-right text-muted-foreground">
                  {data.offset + i + 1}
                </td>
                {row.map((v, j) => (
                  <td
                    key={j}
                    title={cellText(v)}
                    className={cn(
                      'max-w-[28rem] truncate border-b border-r border-border px-2 py-0.5 font-mono',
                      v === null && 'italic text-muted-foreground',
                    )}
                  >
                    {cellText(v)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {data.rows.length === 0 && <div className="p-3 text-xs text-muted-foreground">No rows.</div>}
      </div>
      <div className="flex h-8 shrink-0 items-center gap-2 border-t border-border px-2 text-[11px] text-muted-foreground">
        <span>
          Rows {data.rows.length ? data.offset + 1 : 0}–{data.offset + data.rows.length}
        </span>
        <Button variant="ghost" size="sm" className="ml-auto h-6 text-[11px]" disabled={data.offset === 0}
          onClick={() => onPage(Math.max(0, data.offset - PAGE))}>Previous</Button>
        <Button variant="ghost" size="sm" className="h-6 text-[11px]" disabled={!data.has_more}
          onClick={() => onPage(data.offset + PAGE)}>Next</Button>
      </div>
    </div>
  );
}

/** Read-only SQLite browser: tables/views, paged rows, and ad-hoc SELECT queries. */
export default function SqlitePreview({ tab }) {
  const [schema, setSchema] = useState(null);
  const [error, setError] = useState(null);
  const [table, setTable] = useState(null);
  const [query, setQuery] = useState('');
  const [activeQuery, setActiveQuery] = useState(null);
  const [data, setData] = useState(null);
  const [dataError, setDataError] = useState(null);
  const reloadVersion = useFileReloadVersion(tab.path);

  useEffect(() => {
    let cancelled = false;
    setSchema(null);
    setError(null);
    invoke('sqlite_schema', { path: tab.path })
      .then((s) => {
        if (cancelled) return;
        setSchema(s);
        setTable((cur) => cur ?? s.find((t) => t.kind === 'table')?.name ?? s[0]?.name ?? null);
      })
      .catch((e) => { if (!cancelled) setError(String(e?.message || e)); });
    return () => { cancelled = true; };
  }, [tab.path, reloadVersion]);

  const load = useCallback((offset) => {
    setDataError(null);
    const args = activeQuery
      ? { path: tab.path, query: activeQuery, offset, pageSize: PAGE }
      : { path: tab.path, table, offset, pageSize: PAGE };
    if (!activeQuery && !table) return;
    invoke('sqlite_rows', args)
      .then(setData)
      .catch((e) => { setData(null); setDataError(String(e?.message || e)); });
  }, [tab.path, table, activeQuery]);

  useEffect(() => { load(0); }, [load, reloadVersion]);

  if (error) {
    return <div className="flex h-full items-center justify-center p-6 text-center text-sm text-destructive">{error}</div>;
  }
  if (!schema) return <div className="p-6"><Skeleton className="h-48 w-full" /></div>;

  const current = schema.find((t) => t.name === table);

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-2">
        <div className="truncate text-xs text-muted-foreground">
          {basename(tab.path)} · {schema.length} tables/views · read-only
        </div>
        <form
          className="ml-auto flex min-w-0 flex-1 items-center justify-end gap-1"
          onSubmit={(e) => { e.preventDefault(); setActiveQuery(query.trim() || null); }}
        >
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="SELECT … (read-only)"
            className="h-7 max-w-md font-mono text-xs"
          />
          <Button type="submit" variant="ghost" size="icon" className="size-7" title="Run query">
            <Play className="size-3.5" />
          </Button>
        </form>
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="w-56 shrink-0 overflow-auto border-r border-border py-1">
          {schema.map((t) => (
            <button
              key={t.name}
              type="button"
              onClick={() => { setActiveQuery(null); setTable(t.name); }}
              className={cn(
                'flex w-full items-center gap-2 px-2 py-1 text-left text-[12px] hover:bg-muted/60',
                !activeQuery && table === t.name && 'bg-muted',
              )}
            >
              {t.kind === 'view'
                ? <Eye className="size-3.5 shrink-0 text-muted-foreground" />
                : <Table2 className="size-3.5 shrink-0 text-muted-foreground" />}
              <span className="min-w-0 flex-1 truncate">{t.name}</span>
              {t.row_count != null && (
                <span className="shrink-0 text-[11px] text-muted-foreground">{t.row_count.toLocaleString()}</span>
              )}
            </button>
          ))}
        </div>
        <div className="flex min-w-0 flex-1 flex-col">
          {!activeQuery && current && (
            <div className="flex flex-wrap gap-x-3 gap-y-1 border-b border-border px-2 py-1.5 text-[11px] text-muted-foreground">
              {current.columns.map((c) => (
                <span key={c.name} className="inline-flex items-center gap-1">
                  {c.pk && <KeyRound className="size-3 text-primary" />}
                  <span className="text-foreground">{c.name}</span>
                  {c.decl_type && <span>{c.decl_type.toLowerCase()}</span>}
                </span>
              ))}
            </div>
          )}
          {dataError
            ? <div className="p-3 text-sm text-destructive">{dataError}</div>
            : <Grid data={data} onPage={load} />}
        </div>
      </div>
    </div>
  );
}
