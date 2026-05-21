import React, { useMemo } from 'react';
import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { User, Sparkles } from 'lucide-react';
import { cn } from '@/lib/utils';
import { ToolCallCard } from './tool-call-card';

function renderMarkdown(text) {
  if (!text) return '';
  try {
    const html = marked.parse(text, { breaks: true, gfm: true });
    return DOMPurify.sanitize(html);
  } catch {
    return DOMPurify.sanitize(text);
  }
}

function MarkdownBlock({ text }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  return (
    <div
      className="prose-chat text-sm leading-relaxed [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-[12px] [&_p]:my-1 [&_pre]:my-2 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted/70 [&_pre]:p-2 [&_pre]:text-[12px] [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_ul]:my-1 [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:my-1 [&_ol]:list-decimal [&_ol]:pl-5 [&_h1]:my-2 [&_h1]:text-base [&_h1]:font-semibold [&_h2]:my-2 [&_h2]:text-sm [&_h2]:font-semibold [&_h3]:my-2 [&_h3]:text-sm [&_h3]:font-semibold"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

function ImageAttachment({ src, alt }) {
  return (
    <img
      src={src}
      alt={alt || 'attachment'}
      className="my-1 max-h-48 rounded-md border border-border object-contain"
    />
  );
}

export function ChatMessage({ message, toolResults }) {
  const isUser = message.role === 'user';
  const isTool = message.role === 'tool';

  if (isTool) return null;

  return (
    <div className={cn('flex gap-2 px-3 py-2', isUser && 'bg-muted/20')}>
      <div className="mt-0.5 shrink-0">
        {isUser ? (
          <div className="flex size-6 items-center justify-center rounded-full bg-primary/10 text-primary">
            <User className="size-3.5" />
          </div>
        ) : (
          <div className="flex size-6 items-center justify-center rounded-full bg-muted text-foreground">
            <Sparkles className="size-3.5" />
          </div>
        )}
      </div>
      <div className="min-w-0 flex-1 space-y-1.5">
        {(message.content || []).map((block, idx) => {
          if (block.type === 'text') {
            return <MarkdownBlock key={idx} text={block.text} />;
          }
          if (block.type === 'thinking') {
            return (
              <div
                key={idx}
                className="border-l-2 border-muted-foreground/30 pl-2 italic text-muted-foreground"
              >
                {block.text}
              </div>
            );
          }
          if (block.type === 'tool_use') {
            const result = toolResults?.[block.id];
            return (
              <ToolCallCard
                key={idx}
                name={block.name}
                input={block.input}
                output={result?.output}
                isError={result?.is_error}
              />
            );
          }
          if (block.type === 'image') {
            return <ImageAttachment key={idx} src={block.source?.url || block.url} />;
          }
          return null;
        })}
        {(message.attachments || []).map((att, idx) => (
          <ImageAttachment key={`att-${idx}`} src={att.url || att.src} alt={att.name} />
        ))}
        {message.streaming && (
          <span className="inline-block size-1.5 animate-pulse rounded-full bg-foreground/60" />
        )}
      </div>
    </div>
  );
}

export default ChatMessage;
