import React, { useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';
import { CheckCircle2, XCircle, HelpCircle } from 'lucide-react';

// Render an ask_user request inline in the chat. Three question kinds:
//   - single    → radio buttons (+ optional "Other" free-text)
//   - multi     → checkboxes (+ optional "Other" free-text)
//   - free_text → textarea
// On submit, builds an answers map `{ [question.id]: <string|string[]> }`
// and forwards it to the backend via `respondQuestion(requestId, answers)`.
// Once answered or cancelled, swaps to a read-only summary so the chat
// keeps a record of what was decided.

const OTHER_SENTINEL = '__rustic_ask_user_other__';

export function AskUserInline({ requestId, questions, answered, answers, cancelled }) {
  const respond = useAgent((s) => s.respondQuestion);

  const safeQuestions = Array.isArray(questions) ? questions : [];

  const [draft, setDraft] = useState(() => {
    const d = {};
    for (const q of safeQuestions) {
      d[q.id] = q.kind === 'multi' ? [] : '';
    }
    return d;
  });
  const [other, setOther] = useState({});

  const isComplete = useMemo(() => {
    if (safeQuestions.length === 0) return false;
    return safeQuestions.every((q) => {
      const v = draft[q.id];
      const o = (other[q.id] || '').trim();
      if (q.kind === 'multi') {
        const arr = Array.isArray(v) ? v : [];
        const hasReal = arr.some((x) => x !== OTHER_SENTINEL);
        return hasReal || o.length > 0;
      }
      if (v === OTHER_SENTINEL) return o.length > 0;
      return (typeof v === 'string' && v.trim().length > 0) || o.length > 0;
    });
  }, [draft, other, safeQuestions]);

  function buildAnswers() {
    const out = {};
    for (const q of safeQuestions) {
      const v = draft[q.id];
      const o = (other[q.id] || '').trim();
      if (q.kind === 'multi') {
        const arr = Array.isArray(v) ? v.filter((x) => x !== OTHER_SENTINEL) : [];
        if (o.length > 0) arr.push(o);
        out[q.id] = arr;
      } else if (q.kind === 'free_text') {
        out[q.id] = o || (typeof v === 'string' ? v : '');
      } else {
        // single
        if (v === OTHER_SENTINEL) out[q.id] = o;
        else out[q.id] = typeof v === 'string' ? v : '';
      }
    }
    return out;
  }

  function onSubmit() {
    respond(requestId, buildAnswers(), { cancelled: false });
  }

  function onCancel() {
    respond(requestId, null, { cancelled: true });
  }

  if (cancelled) {
    return (
      <div className="relative py-1 pl-7">
        <span className="absolute left-1.5 top-2.5 grid size-3.5 place-items-center rounded-full bg-background">
          <XCircle className="size-3 text-muted-foreground" />
        </span>
        <div className="rounded-lg border border-dashed border-muted-foreground/30 bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
          Question dismissed.
        </div>
      </div>
    );
  }

  if (answered) {
    return <AnsweredView questions={safeQuestions} answers={answers || {}} />;
  }

  return (
    <div className="relative py-1 pl-7">
      <span className="absolute left-1.5 top-2.5 grid size-3.5 place-items-center rounded-full bg-background">
        <HelpCircle className="size-3 text-blue-500" />
      </span>
      <div className="space-y-3 rounded-lg border border-blue-500/30 bg-blue-500/5 px-3 py-3">
        <div className="text-xs font-medium text-blue-700 dark:text-blue-300">
          The agent {safeQuestions.length > 1 ? `has ${safeQuestions.length} questions` : 'has a question'}
        </div>
        <div className="space-y-4">
          {safeQuestions.map((q, qi) => (
            <QuestionRow
              key={q.id ?? qi}
              question={q}
              value={draft[q.id]}
              otherValue={other[q.id] || ''}
              onChange={(v) => setDraft((d) => ({ ...d, [q.id]: v }))}
              onOtherChange={(v) => setOther((o) => ({ ...o, [q.id]: v }))}
            />
          ))}
        </div>
        <div className="flex items-center justify-end gap-2 pt-1">
          <Button variant="ghost" size="sm" onClick={onCancel}>
            Cancel
          </Button>
          <Button size="sm" onClick={onSubmit} disabled={!isComplete}>
            Send
          </Button>
        </div>
      </div>
    </div>
  );
}

function QuestionRow({ question, value, otherValue, onChange, onOtherChange }) {
  const opts = Array.isArray(question.options) ? question.options : [];
  const kind = question.kind;
  const id = question.id;

  return (
    <div className="space-y-2">
      <div className="text-sm font-medium leading-snug">{question.text}</div>

      {kind === 'free_text' ? (
        <Textarea
          autoFocus
          value={otherValue}
          onChange={(e) => onOtherChange(e.target.value)}
          placeholder="Type your answer..."
          className="min-h-[64px] text-sm"
        />
      ) : kind === 'multi' ? (
        <div className="space-y-1.5">
          {opts.map((opt) => {
            const checked = Array.isArray(value) && value.includes(opt);
            return (
              <label
                key={opt}
                className="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 hover:bg-muted/50"
              >
                <Checkbox
                  checked={checked}
                  onCheckedChange={(c) => {
                    const arr = Array.isArray(value) ? value.slice() : [];
                    if (c) {
                      if (!arr.includes(opt)) arr.push(opt);
                    } else {
                      const idx = arr.indexOf(opt);
                      if (idx >= 0) arr.splice(idx, 1);
                    }
                    onChange(arr);
                  }}
                />
                <span className="text-sm">{opt}</span>
              </label>
            );
          })}
          <div className="flex items-center gap-2 pt-1">
            <Checkbox
              checked={Array.isArray(value) && value.includes(OTHER_SENTINEL)}
              onCheckedChange={(c) => {
                const arr = Array.isArray(value) ? value.slice() : [];
                if (c) {
                  if (!arr.includes(OTHER_SENTINEL)) arr.push(OTHER_SENTINEL);
                } else {
                  const idx = arr.indexOf(OTHER_SENTINEL);
                  if (idx >= 0) arr.splice(idx, 1);
                  onOtherChange('');
                }
                onChange(arr);
              }}
            />
            <Input
              value={otherValue}
              onChange={(e) => {
                onOtherChange(e.target.value);
                const arr = Array.isArray(value) ? value.slice() : [];
                if (e.target.value && !arr.includes(OTHER_SENTINEL)) {
                  arr.push(OTHER_SENTINEL);
                  onChange(arr);
                }
              }}
              placeholder="Other..."
              className="h-7 text-sm"
            />
          </div>
        </div>
      ) : (
        // single
        <div className="space-y-1.5">
          {opts.map((opt) => (
            <label
              key={opt}
              className="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 hover:bg-muted/50"
            >
              <input
                type="radio"
                name={`ask-${id}`}
                value={opt}
                checked={value === opt}
                onChange={() => {
                  onChange(opt);
                  onOtherChange('');
                }}
                className="size-3.5 accent-primary"
              />
              <span className="text-sm">{opt}</span>
            </label>
          ))}
          <div className="flex items-center gap-2 pt-1">
            <input
              type="radio"
              name={`ask-${id}`}
              checked={value === OTHER_SENTINEL}
              onChange={() => onChange(OTHER_SENTINEL)}
              className="size-3.5 accent-primary"
            />
            <Input
              value={otherValue}
              onChange={(e) => {
                onOtherChange(e.target.value);
                if (e.target.value && value !== OTHER_SENTINEL) onChange(OTHER_SENTINEL);
              }}
              placeholder="Other..."
              className="h-7 text-sm"
            />
          </div>
        </div>
      )}
    </div>
  );
}

function AnsweredView({ questions, answers }) {
  return (
    <div className="relative py-1 pl-7">
      <span className="absolute left-1.5 top-2.5 grid size-3.5 place-items-center rounded-full bg-background">
        <CheckCircle2 className="size-3 text-green-500" />
      </span>
      <div className="space-y-2 rounded-lg border border-muted-foreground/20 bg-muted/30 px-3 py-2">
        {questions.map((q, qi) => {
          const a = answers[q.id];
          const display = Array.isArray(a)
            ? a.length > 0
              ? a.join(', ')
              : '(none)'
            : a && String(a).trim().length > 0
              ? String(a)
              : '(empty)';
          return (
            <div key={q.id ?? qi} className="space-y-0.5">
              <div className="text-xs text-muted-foreground">{q.text}</div>
              <div className="text-sm">{display}</div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export default AskUserInline;
