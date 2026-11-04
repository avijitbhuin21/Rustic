// Right-hand detail pane shared by the Skills / Workflows / Rules panels:
// name + Preview/Edit toggle, rendered markdown in Preview, textarea + Save in Edit.
import React, { useMemo } from 'react';
import { Eye, Pencil, Save } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { renderMarkdown } from '@/lib/markdown-render';

/** Preview/edit body for one markdown library entry. */
export function MarkdownEntryEditor({ title, editName, onEditName, body, onBody, mode, onMode, onSave, kind = 'entry' }) {
  const html = useMemo(() => renderMarkdown((body || '').trim()), [body]);
  return (
    <>
      <div className="flex shrink-0 items-center gap-1.5 border-b border-border p-1.5">
        {mode === 'edit' ? (
          <Input value={editName} onChange={(e) => onEditName(e.target.value)} placeholder={`${kind[0].toUpperCase()}${kind.slice(1)} name`} className="h-7 min-w-0 flex-1 text-xs" />
        ) : (
          <span className="min-w-0 flex-1 truncate px-1 text-xs font-medium" title={title}>{title}</span>
        )}
        <div className="flex shrink-0 items-center rounded-md border border-border p-0.5">
          <Button size="sm" variant={mode === 'preview' ? 'secondary' : 'ghost'} className="h-6 gap-1 px-2 text-[11px]" onClick={() => onMode('preview')} title="Rendered preview">
            <Eye className="size-3" /> Preview
          </Button>
          <Button size="sm" variant={mode === 'edit' ? 'secondary' : 'ghost'} className="h-6 gap-1 px-2 text-[11px]" onClick={() => onMode('edit')} title="Edit markdown">
            <Pencil className="size-3" /> Edit
          </Button>
        </div>
      </div>
      {mode === 'edit' ? (
        <>
          <Textarea
            value={body}
            onChange={(e) => onBody(e.target.value)}
            className="min-h-0 flex-1 resize-none overflow-y-auto rounded-none border-0 font-mono text-[11px] [field-sizing:fixed]"
            placeholder={`${kind[0].toUpperCase()}${kind.slice(1)} body…`}
            spellCheck={false}
          />
          <div className="shrink-0 border-t border-border p-1.5">
            <Button size="sm" className="h-7 w-full text-xs" onClick={onSave}>
              <Save className="mr-1 size-3" /> Save
            </Button>
          </div>
        </>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          {(body || '').trim() ? (
            <div
              className="rustic-markdown min-w-0 break-words px-4 py-3 text-[13px] [&_pre]:overflow-x-auto"
              // Sanitized by DOMPurify inside renderMarkdown.
              dangerouslySetInnerHTML={{ __html: html }}
            />
          ) : (
            <p className="px-4 py-3 text-xs text-muted-foreground">This {kind} has no body yet. Switch to Edit to write it.</p>
          )}
        </div>
      )}
    </>
  );
}

export default MarkdownEntryEditor;
