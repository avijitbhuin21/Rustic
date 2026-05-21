import React, { useEffect, useState } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { useAgent } from '@/state/agent';

export function QuestionPrompt() {
  const pending = useAgent((s) => s.pendingQuestion);
  const respond = useAgent((s) => s.respondQuestion);
  const close = useAgent((s) => s.closeQuestion);
  const [value, setValue] = useState('');

  useEffect(() => {
    setValue('');
  }, [pending?.request_id]);

  const open = !!pending;
  const choices = pending?.choices || [];

  const handleCancel = () => {
    respond(null, { cancelled: true });
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) handleCancel();
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Agent question</DialogTitle>
          <DialogDescription>
            {pending?.question || 'The agent has a question.'}
          </DialogDescription>
        </DialogHeader>
        {choices.length > 0 ? (
          <div className="flex flex-col gap-1.5">
            {choices.map((c, i) => (
              <Button
                key={i}
                variant="outline"
                className="justify-start"
                onClick={() => respond(typeof c === 'string' ? c : c.value || c.label)}
              >
                {typeof c === 'string' ? c : c.label || c.value}
              </Button>
            ))}
          </div>
        ) : (
          <Textarea
            autoFocus
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="Type your answer..."
            className="min-h-[80px]"
          />
        )}
        {choices.length === 0 && (
          <DialogFooter>
            <Button variant="outline" onClick={handleCancel}>
              Cancel
            </Button>
            <Button onClick={() => respond(value)} disabled={!value.trim()}>
              Send
            </Button>
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  );
}

export default QuestionPrompt;
