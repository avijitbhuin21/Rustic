import React, { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Skeleton } from '@/components/ui/skeleton';

export default function MarkdownPreview({ tab }) {
  const [text, setText] = useState(null);
  const [error, setError] = useState(null);

  useEffect(() => {
    let cancelled = false;
    invoke('read_file_content', { path: tab.path })
      .then((c) => {
        if (!cancelled) setText(c ?? '');
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [tab.path]);

  const html = useMemo(() => {
    if (text == null) return '';
    const raw = marked.parse(text, { breaks: true, gfm: true });
    return DOMPurify.sanitize(raw);
  }, [text]);

  if (error) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (text == null) {
    return (
      <div className="flex h-full w-full flex-col gap-2 p-6">
        <Skeleton className="h-6 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    );
  }

  return (
    <ScrollArea className="h-full w-full">
      <div
        className="prose prose-invert prose-sm mx-auto max-w-3xl p-6 text-foreground [&_a]:text-primary [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_pre]:rounded [&_pre]:bg-muted [&_pre]:p-3"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </ScrollArea>
  );
}
