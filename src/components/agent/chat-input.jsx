import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Send, Square, Paperclip } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { cn } from '@/lib/utils';

export function ChatInput({ onSubmit, onAbort, isStreaming, disabled }) {
  const [value, setValue] = useState('');
  const [attachments, setAttachments] = useState([]);
  const textareaRef = useRef(null);

  const autoGrow = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  }, []);

  useEffect(() => {
    autoGrow();
  }, [value, autoGrow]);

  const submit = useCallback(() => {
    const trimmed = value.trim();
    if (!trimmed && attachments.length === 0) return;
    onSubmit?.(trimmed, attachments);
    setValue('');
    setAttachments([]);
  }, [value, attachments, onSubmit]);

  const onKeyDown = (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      submit();
    }
  };

  const onPaste = (e) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const it of items) {
      if (it.kind === 'file' && it.type.startsWith('image/')) {
        const file = it.getAsFile();
        if (!file) continue;
        const reader = new FileReader();
        reader.onload = () => {
          setAttachments((prev) => [
            ...prev,
            { name: file.name || 'pasted-image', url: reader.result, type: file.type },
          ]);
        };
        reader.readAsDataURL(file);
        e.preventDefault();
      }
    }
  };

  const showHint = value.startsWith('/');

  return (
    <div className="border-t border-border bg-background p-2">
      {attachments.length > 0 && (
        <div className="mb-1.5 flex flex-wrap gap-1.5">
          {attachments.map((att, i) => (
            <div
              key={i}
              className="flex items-center gap-1 rounded border border-border bg-muted px-1.5 py-0.5 text-[11px]"
            >
              <Paperclip className="size-3" />
              <span className="max-w-[120px] truncate">{att.name}</span>
              <button
                type="button"
                onClick={() => setAttachments((prev) => prev.filter((_, j) => j !== i))}
                className="ml-1 text-muted-foreground hover:text-foreground"
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}
      {showHint && (
        <div className="mb-1 px-1 text-[10px] text-muted-foreground">
          Slash commands: /clear /help /model
        </div>
      )}
      <div className="flex items-end gap-1.5">
        <Textarea
          ref={textareaRef}
          rows={1}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          placeholder="Ask anything..."
          disabled={disabled}
          className={cn(
            'min-h-[36px] resize-none overflow-y-auto bg-background text-sm',
            'focus-visible:ring-1'
          )}
        />
        {isStreaming ? (
          <Button
            type="button"
            size="icon"
            variant="outline"
            onClick={onAbort}
            className="size-9"
            title="Stop"
          >
            <Square className="size-3.5" />
          </Button>
        ) : (
          <Button
            type="button"
            size="icon"
            onClick={submit}
            disabled={disabled || (!value.trim() && attachments.length === 0)}
            className="size-9"
            title="Send (Cmd/Ctrl+Enter)"
          >
            <Send className="size-3.5" />
          </Button>
        )}
      </div>
      <div className="mt-1 px-1 text-[10px] text-muted-foreground">
        Ctrl+Enter to send
      </div>
    </div>
  );
}

export default ChatInput;
