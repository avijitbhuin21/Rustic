import React, { useMemo, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';
import { CheckCircle2, XCircle, HelpCircle, ImagePlus, X } from 'lucide-react';
import { extractImagesFromClipboard, readFileAsBase64 } from '@/lib/clipboard-image';

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

  const [activeTab, setActiveTab] = useState(0);

  // Images the user attaches to their answer (whole-response, not per-question).
  // Each entry: { id, name, mediaType, base64Data, url(dataUrl for thumbnail) }.
  const [images, setImages] = useState([]);
  const fileInputRef = useRef(null);
  const imgIdRef = useRef(0);

  const addFiles = async (files) => {
    const list = Array.from(files || []).filter(
      (f) => f && (f.type || '').startsWith('image/'),
    );
    for (const file of list) {
      try {
        const { base64, dataUrl } = await readFileAsBase64(file);
        const id = `ask-img-${imgIdRef.current++}`;
        setImages((imgs) => [
          ...imgs,
          { id, name: file.name || 'image', mediaType: file.type || 'image/png', base64Data: base64, url: dataUrl },
        ]);
      } catch {
        // Skip unreadable files rather than failing the whole attach.
      }
    }
  };

  const removeImage = (id) => setImages((imgs) => imgs.filter((i) => i.id !== id));

  // Catch Ctrl+V pastes anywhere in the dialog (works when a text field inside
  // is focused — the common case for free_text answers). Options-only questions
  // can still attach via the "Attach image" button.
  const onPaste = (e) => {
    const pasted = extractImagesFromClipboard(e.clipboardData);
    if (pasted.length === 0) return;
    e.preventDefault();
    addFiles(pasted.map((p) => p.file));
  };

  // Is a single question answered? Drives both the Send gate and the per-tab
  // checkmark.
  function questionComplete(q) {
    const v = draft[q.id];
    const o = (other[q.id] || '').trim();
    if (q.kind === 'multi') {
      const arr = Array.isArray(v) ? v : [];
      const hasReal = arr.some((x) => x !== OTHER_SENTINEL);
      return hasReal || o.length > 0;
    }
    if (v === OTHER_SENTINEL) return o.length > 0;
    return (typeof v === 'string' && v.trim().length > 0) || o.length > 0;
  }

  const completeFlags = useMemo(
    () => safeQuestions.map((q) => questionComplete(q)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [draft, other, safeQuestions],
  );
  const isComplete =
    safeQuestions.length > 0 && completeFlags.every(Boolean);

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
    respond(requestId, buildAnswers(), { cancelled: false, images });
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

  const multi = safeQuestions.length > 1;
  const activeIdx = Math.min(activeTab, safeQuestions.length - 1);
  const activeQuestion = safeQuestions[activeIdx];

  return (
    <div className="relative py-1 pl-7">
      <span className="absolute left-1.5 top-2.5 grid size-3.5 place-items-center rounded-full bg-background">
        <HelpCircle className="size-3 text-blue-500" />
      </span>
      <div className="overflow-hidden rounded-lg border border-blue-500/30 bg-blue-500/5" onPaste={onPaste}>
        <div className="flex items-center justify-between gap-2 border-b border-blue-500/20 px-3 py-2">
          <div className="text-xs font-medium text-blue-700 dark:text-blue-300">
            The agent {multi ? `has ${safeQuestions.length} questions` : 'has a question'}
          </div>
          {multi && (
            <div className="text-[10px] tabular-nums text-muted-foreground">
              {completeFlags.filter(Boolean).length}/{safeQuestions.length} answered
            </div>
          )}
        </div>

        {/* One tab per question. Click to jump; a check marks answered ones. */}
        {multi && (
          <div className="flex gap-1 overflow-x-auto border-b border-blue-500/20 px-2 py-1.5">
            {safeQuestions.map((q, qi) => {
              const done = completeFlags[qi];
              const active = qi === activeIdx;
              return (
                <button
                  key={q.id ?? qi}
                  type="button"
                  onClick={() => setActiveTab(qi)}
                  className={cn(
                    'flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-xs transition-colors',
                    active
                      ? 'bg-blue-500/15 font-medium text-blue-700 dark:text-blue-300'
                      : 'text-muted-foreground hover:bg-muted/60',
                  )}
                  title={q.text}
                >
                  {done ? (
                    <CheckCircle2 className="size-3 text-green-500" />
                  ) : (
                    <span
                      className={cn(
                        'grid size-3.5 place-items-center rounded-full border text-[9px]',
                        active ? 'border-blue-500 text-blue-600' : 'border-muted-foreground/40',
                      )}
                    >
                      {qi + 1}
                    </span>
                  )}
                  <span className="max-w-[120px] truncate">{q.text}</span>
                </button>
              );
            })}
          </div>
        )}

        <div className="px-3 py-3">
          {activeQuestion && (
            <QuestionRow
              key={activeQuestion.id ?? activeIdx}
              question={activeQuestion}
              value={draft[activeQuestion.id]}
              otherValue={other[activeQuestion.id] || ''}
              onChange={(v) => setDraft((d) => ({ ...d, [activeQuestion.id]: v }))}
              onOtherChange={(v) => setOther((o) => ({ ...o, [activeQuestion.id]: v }))}
            />
          )}
        </div>

        {/* Optional image attachments for the whole response. Paste (Ctrl+V) or
            pick files; thumbnails show below with a remove button. */}
        <div className="space-y-2 px-3 pb-2">
          {images.length > 0 && (
            <div className="flex flex-wrap gap-2">
              {images.map((img) => (
                <div key={img.id} className="group relative">
                  <img
                    src={img.url}
                    alt={img.name}
                    className="size-14 rounded border border-border/60 object-cover"
                  />
                  <button
                    type="button"
                    onClick={() => removeImage(img.id)}
                    className="absolute -right-1.5 -top-1.5 grid size-4 place-items-center rounded-full border border-border bg-background text-muted-foreground hover:text-destructive"
                    aria-label="Remove image"
                    title="Remove image"
                  >
                    <X className="size-2.5" />
                  </button>
                </div>
              ))}
            </div>
          )}
          <input
            ref={fileInputRef}
            type="file"
            accept="image/*"
            multiple
            className="hidden"
            onChange={(e) => {
              addFiles(e.target.files);
              e.target.value = '';
            }}
          />
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground"
            onClick={() => fileInputRef.current?.click()}
          >
            <ImagePlus className="mr-1 size-3.5" /> Attach image
          </Button>
        </div>

        <div className="flex items-center justify-between gap-2 border-t border-blue-500/20 px-3 py-2">
          <Button variant="ghost" size="sm" onClick={onCancel}>
            Cancel
          </Button>
          <div className="flex items-center gap-2">
            {multi && activeIdx > 0 && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setActiveTab(activeIdx - 1)}
              >
                Previous
              </Button>
            )}
            {multi && activeIdx < safeQuestions.length - 1 ? (
              <Button size="sm" onClick={() => setActiveTab(activeIdx + 1)}>
                Next
              </Button>
            ) : (
              <Button
                size="sm"
                onClick={onSubmit}
                disabled={!isComplete && images.length === 0}
              >
                Send
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// Normalize a single option entry into { value, label, description }. The
// ask_user schema specifies plain strings, but models frequently emit
// `{ label, description }` (or `{ value, label }`) objects instead. Rendering
// such an object directly as a React child throws (React error #31), so coerce
// every entry to a stable string value plus display label/description here.
function normalizeOption(opt) {
  if (opt && typeof opt === 'object') {
    const value = String(opt.value ?? opt.label ?? '');
    const label = String(opt.label ?? opt.value ?? '');
    const description = opt.description != null ? String(opt.description) : '';
    return { value, label, description };
  }
  const str = String(opt ?? '');
  return { value: str, label: str, description: '' };
}

function QuestionRow({ question, value, otherValue, onChange, onOtherChange }) {
  const opts = (Array.isArray(question.options) ? question.options : []).map(normalizeOption);
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
            const checked = Array.isArray(value) && value.includes(opt.value);
            return (
              <label
                key={opt.value}
                className="flex cursor-pointer items-start gap-2 rounded px-1 py-0.5 hover:bg-muted/50"
              >
                <Checkbox
                  className="mt-0.5"
                  checked={checked}
                  onCheckedChange={(c) => {
                    const arr = Array.isArray(value) ? value.slice() : [];
                    if (c) {
                      if (!arr.includes(opt.value)) arr.push(opt.value);
                    } else {
                      const idx = arr.indexOf(opt.value);
                      if (idx >= 0) arr.splice(idx, 1);
                    }
                    onChange(arr);
                  }}
                />
                <span className="flex flex-col">
                  <span className="text-sm">{opt.label}</span>
                  {opt.description && (
                    <span className="text-xs text-muted-foreground">{opt.description}</span>
                  )}
                </span>
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
              key={opt.value}
              className="flex cursor-pointer items-start gap-2 rounded px-1 py-0.5 hover:bg-muted/50"
            >
              <input
                type="radio"
                name={`ask-${id}`}
                value={opt.value}
                checked={value === opt.value}
                onChange={() => {
                  onChange(opt.value);
                  onOtherChange('');
                }}
                className="mt-0.5 size-3.5 accent-primary"
              />
              <span className="flex flex-col">
                <span className="text-sm">{opt.label}</span>
                {opt.description && (
                  <span className="text-xs text-muted-foreground">{opt.description}</span>
                )}
              </span>
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
