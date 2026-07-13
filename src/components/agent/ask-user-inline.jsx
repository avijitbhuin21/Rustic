import React, { useEffect, useMemo, useRef, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { useAgent } from '@/state/agent';
import { cn } from '@/lib/utils';
import {
  Check,
  CheckCircle2,
  XCircle,
  HelpCircle,
  ImagePlus,
  X,
  ChevronDown,
  ChevronLeft,
  Plus,
} from 'lucide-react';
import { extractImagesFromClipboard, readFileAsBase64 } from '@/lib/clipboard-image';

const OTHER_SENTINEL = '__rustic_ask_user_other__';
const AUTO_ADVANCE_MS = 280;

const slideVariants = {
  enter: (dir) => ({ opacity: 0, x: dir * 28 }),
  center: { opacity: 1, x: 0 },
  exit: (dir) => ({ opacity: 0, x: dir * -28 }),
};

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
  const [dir, setDir] = useState(1);

  const [images, setImages] = useState([]);
  const fileInputRef = useRef(null);
  const imgIdRef = useRef(0);
  const advanceTimer = useRef(null);

  useEffect(() => () => clearTimeout(advanceTimer.current), []);

  const goTo = (idx) => {
    clearTimeout(advanceTimer.current);
    setDir(idx >= activeTab ? 1 : -1);
    setActiveTab(idx);
  };

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
        /* skip unreadable file */
      }
    }
  };

  const removeImage = (id) => setImages((imgs) => imgs.filter((i) => i.id !== id));

  const onPaste = (e) => {
    const pasted = extractImagesFromClipboard(e.clipboardData);
    if (pasted.length === 0) return;
    e.preventDefault();
    addFiles(pasted.map((p) => p.file));
  };

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
  const isComplete = safeQuestions.length > 0 && completeFlags.every(Boolean);

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
        if (v === OTHER_SENTINEL) out[q.id] = o;
        else out[q.id] = typeof v === 'string' ? v : '';
      }
    }
    return out;
  }

  function scheduleAdvance(fromIdx) {
    if (fromIdx >= safeQuestions.length - 1) return;
    clearTimeout(advanceTimer.current);
    advanceTimer.current = setTimeout(() => {
      setDir(1);
      setActiveTab(fromIdx + 1);
    }, AUTO_ADVANCE_MS);
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
        <div className="rounded-lg border border-dashed border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
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
  const answeredCount = completeFlags.filter(Boolean).length;

  return (
    <div className="relative py-1 pl-7">
      <span className="absolute left-1.5 top-2.5 grid size-3.5 place-items-center rounded-full bg-background">
        <HelpCircle className="size-3 text-primary" />
      </span>
      <motion.div
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: 'easeOut' }}
        className="overflow-hidden rounded-lg border border-border/70 bg-card shadow-sm"
        onPaste={onPaste}
      >
        <div className="flex items-center justify-between gap-3 border-b border-border/60 px-3 py-2">
          <div className="text-xs font-medium">
            {multi ? 'Questions from the agent' : 'Question from the agent'}
          </div>
          {multi && (
            <div className="flex items-center gap-2">
              <div className="flex items-center gap-1.5">
                {safeQuestions.map((q, qi) => (
                  <button
                    key={q.id ?? qi}
                    type="button"
                    onClick={() => goTo(qi)}
                    title={q.text}
                    className={cn(
                      'size-2 rounded-full transition-all duration-200',
                      qi === activeIdx
                        ? 'scale-125 bg-primary ring-2 ring-primary/25'
                        : completeFlags[qi]
                          ? 'bg-primary/60 hover:bg-primary'
                          : 'bg-muted-foreground/25 hover:bg-muted-foreground/50',
                    )}
                    aria-label={`Question ${qi + 1}`}
                  />
                ))}
              </div>
              <span className="text-[10px] tabular-nums text-muted-foreground">
                {answeredCount}/{safeQuestions.length}
              </span>
            </div>
          )}
        </div>

        <div className="px-3 py-3">
          <AnimatePresence mode="wait" custom={dir} initial={false}>
            <motion.div
              key={activeQuestion?.id ?? activeIdx}
              custom={dir}
              variants={slideVariants}
              initial="enter"
              animate="center"
              exit="exit"
              transition={{ duration: 0.16, ease: 'easeOut' }}
            >
              {activeQuestion && (
                <QuestionBody
                  question={activeQuestion}
                  value={draft[activeQuestion.id]}
                  otherValue={other[activeQuestion.id] || ''}
                  onChange={(v) => setDraft((d) => ({ ...d, [activeQuestion.id]: v }))}
                  onOtherChange={(v) => setOther((o) => ({ ...o, [activeQuestion.id]: v }))}
                  onSinglePicked={() => scheduleAdvance(activeIdx)}
                />
              )}
            </motion.div>
          </AnimatePresence>
        </div>

        {images.length > 0 && (
          <div className="flex flex-wrap gap-2 px-3 pb-2">
            {images.map((img) => (
              <motion.div
                key={img.id}
                initial={{ opacity: 0, scale: 0.85 }}
                animate={{ opacity: 1, scale: 1 }}
                className="group relative"
              >
                <img
                  src={img.url}
                  alt={img.name}
                  className="size-14 rounded-md border border-border/60 object-cover"
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
              </motion.div>
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

        <div className="flex items-center justify-between gap-2 border-t border-border/60 px-2 py-1.5">
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              className="size-7 p-0 text-muted-foreground"
              onClick={() => fileInputRef.current?.click()}
              title="Attach image (or paste)"
              aria-label="Attach image"
            >
              <ImagePlus className="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground"
              onClick={onCancel}
            >
              Dismiss
            </Button>
          </div>
          <div className="flex items-center gap-1.5">
            {multi && activeIdx > 0 && (
              <Button
                variant="ghost"
                size="sm"
                className="size-7 p-0 text-muted-foreground"
                onClick={() => goTo(activeIdx - 1)}
                title="Previous question"
                aria-label="Previous question"
              >
                <ChevronLeft className="size-3.5" />
              </Button>
            )}
            {multi && activeIdx < safeQuestions.length - 1 ? (
              <Button size="sm" className="h-7 px-3 text-xs" onClick={() => goTo(activeIdx + 1)}>
                Next
              </Button>
            ) : (
              <>
                {!isComplete && safeQuestions.length > 0 && (
                  <span className="text-[10px] text-muted-foreground">
                    {completeFlags.filter((f) => !f).length} unanswered
                  </span>
                )}
                <Button
                  size="sm"
                  className="h-7 px-3 text-xs"
                  onClick={onSubmit}
                  disabled={safeQuestions.length > 0 ? !isComplete : false}
                >
                  Send
                </Button>
              </>
            )}
          </div>
        </div>
      </motion.div>
    </div>
  );
}

function normalizeOption(opt) {
  /** Coerces a string or {value,label,description} option into a stable shape. */
  if (opt && typeof opt === 'object') {
    const value = String(opt.value ?? opt.label ?? '');
    const label = String(opt.label ?? opt.value ?? '');
    const description = opt.description != null ? String(opt.description) : '';
    return { value, label, description };
  }
  const str = String(opt ?? '');
  return { value: str, label: str, description: '' };
}

function Chip({ selected, onClick, children, title }) {
  /** Pill-shaped selectable option button. */
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.96 }}
      onClick={onClick}
      title={title}
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-xs transition-colors duration-150',
        selected
          ? 'border-primary bg-primary text-primary-foreground shadow-sm'
          : 'border-border bg-background text-foreground hover:border-primary/40 hover:bg-accent',
      )}
    >
      {selected && <Check className="size-3 shrink-0" />}
      <span className="max-w-[280px] truncate">{children}</span>
    </motion.button>
  );
}

function OtherChip({ selected, text, onSelect, onTextChange, onClear }) {
  /** Dashed "Other" chip that expands into an inline text input when active. */
  const inputRef = useRef(null);
  const [editing, setEditing] = useState(false);
  const open = editing || selected || text.length > 0;

  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  if (!open) {
    return (
      <motion.button
        type="button"
        whileTap={{ scale: 0.96 }}
        onClick={() => {
          setEditing(true);
          onSelect();
        }}
        className="inline-flex items-center gap-1.5 rounded-full border border-dashed border-border px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
      >
        <Plus className="size-3" />
        Other
      </motion.button>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, width: 0 }}
      animate={{ opacity: 1, width: 'auto' }}
      className={cn(
        'inline-flex items-center gap-1 overflow-hidden rounded-full border py-0.5 pl-3 pr-1 transition-colors',
        selected && text.trim() ? 'border-primary bg-primary/5' : 'border-border bg-background',
      )}
    >
      <input
        ref={inputRef}
        value={text}
        onChange={(e) => onTextChange(e.target.value)}
        onBlur={() => {
          setEditing(false);
          if (!text.trim()) onClear();
        }}
        placeholder="Other..."
        className="w-36 bg-transparent text-xs outline-none placeholder:text-muted-foreground"
      />
      <button
        type="button"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          setEditing(false);
          onTextChange('');
          onClear();
        }}
        className="grid size-5 place-items-center rounded-full text-muted-foreground hover:text-foreground"
        aria-label="Clear"
      >
        <X className="size-3" />
      </button>
    </motion.div>
  );
}

function OptionRow({ selected, onClick, label, description, multiKind }) {
  /** Full-width selectable row used when options carry descriptions. */
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.99 }}
      onClick={onClick}
      className={cn(
        'flex w-full items-start gap-2.5 rounded-md border px-3 py-2 text-left transition-colors duration-150',
        selected
          ? 'border-primary/60 bg-primary/5'
          : 'border-border bg-background hover:border-primary/30 hover:bg-accent',
      )}
    >
      <span
        className={cn(
          'mt-0.5 grid size-4 shrink-0 place-items-center border transition-colors',
          multiKind ? 'rounded' : 'rounded-full',
          selected ? 'border-primary bg-primary text-primary-foreground' : 'border-muted-foreground/40',
        )}
      >
        {selected && <Check className="size-3" />}
      </span>
      <span className="flex min-w-0 flex-col">
        <span className="break-words text-sm">{label}</span>
        {description && (
          <span className="break-words text-xs text-muted-foreground">{description}</span>
        )}
      </span>
    </motion.button>
  );
}

function QuestionBody({ question, value, otherValue, onChange, onOtherChange, onSinglePicked }) {
  /** Renders one question's prompt plus its kind-specific answer controls. */
  const opts = (Array.isArray(question.options) ? question.options : []).map(normalizeOption);
  const kind = question.kind;
  const hasDescriptions = opts.some((o) => o.description);

  const toggleMulti = (optValue) => {
    const arr = Array.isArray(value) ? value.slice() : [];
    const idx = arr.indexOf(optValue);
    if (idx >= 0) arr.splice(idx, 1);
    else arr.push(optValue);
    onChange(arr);
  };

  const pickSingle = (optValue) => {
    onChange(optValue);
    onOtherChange('');
    onSinglePicked?.();
  };

  const otherSelected =
    kind === 'multi'
      ? Array.isArray(value) && value.includes(OTHER_SENTINEL)
      : value === OTHER_SENTINEL;

  const selectOther = () => {
    if (kind === 'multi') {
      const arr = Array.isArray(value) ? value.slice() : [];
      if (!arr.includes(OTHER_SENTINEL)) arr.push(OTHER_SENTINEL);
      onChange(arr);
    } else {
      onChange(OTHER_SENTINEL);
    }
  };

  const clearOther = () => {
    onOtherChange('');
    if (kind === 'multi') {
      const arr = Array.isArray(value) ? value.filter((x) => x !== OTHER_SENTINEL) : [];
      onChange(arr);
    } else if (value === OTHER_SENTINEL) {
      onChange('');
    }
  };

  return (
    <div className="space-y-2.5">
      <div className="break-words text-sm font-medium leading-snug">{question.text}</div>

      {kind === 'free_text' ? (
        <Textarea
          autoFocus
          value={otherValue}
          onChange={(e) => onOtherChange(e.target.value)}
          placeholder="Type your answer..."
          className="min-h-[72px] resize-none bg-background text-sm"
        />
      ) : hasDescriptions ? (
        <div className="space-y-1.5">
          {opts.map((opt) => {
            const selected =
              kind === 'multi'
                ? Array.isArray(value) && value.includes(opt.value)
                : value === opt.value;
            return (
              <OptionRow
                key={opt.value}
                selected={selected}
                multiKind={kind === 'multi'}
                label={opt.label}
                description={opt.description}
                onClick={() =>
                  kind === 'multi' ? toggleMulti(opt.value) : pickSingle(opt.value)
                }
              />
            );
          })}
          <div className="pt-0.5">
            <OtherChip
              selected={otherSelected}
              text={otherValue}
              onSelect={selectOther}
              onTextChange={(v) => {
                onOtherChange(v);
                if (v && !otherSelected) selectOther();
              }}
              onClear={clearOther}
            />
          </div>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-1.5">
          {opts.map((opt) => {
            const selected =
              kind === 'multi'
                ? Array.isArray(value) && value.includes(opt.value)
                : value === opt.value;
            return (
              <Chip
                key={opt.value}
                selected={selected}
                title={opt.label}
                onClick={() =>
                  kind === 'multi' ? toggleMulti(opt.value) : pickSingle(opt.value)
                }
              >
                {opt.label}
              </Chip>
            );
          })}
          <OtherChip
            selected={otherSelected}
            text={otherValue}
            onSelect={selectOther}
            onTextChange={(v) => {
              onOtherChange(v);
              if (v && !otherSelected) selectOther();
            }}
            onClear={clearOther}
          />
        </div>
      )}
    </div>
  );
}

const ANSWER_CLAMP_CHARS = 400;

function AnswerText({ text }) {
  /** Renders one answer with word-breaking and a Show more clamp for long text. */
  const [expanded, setExpanded] = useState(false);
  const isLong = text.length > ANSWER_CLAMP_CHARS || text.split('\n').length > 6;
  const shown = !isLong || expanded ? text : `${text.slice(0, ANSWER_CLAMP_CHARS)}\u2026`;
  return (
    <div className="min-w-0">
      <div className="whitespace-pre-wrap break-words text-sm [overflow-wrap:anywhere]">{shown}</div>
      {isLong && (
        <button
          type="button"
          onClick={() => setExpanded((e) => !e)}
          className="mt-0.5 inline-flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground"
        >
          <ChevronDown className={cn('size-3 transition-transform', expanded && 'rotate-180')} />
          {expanded ? 'Show less' : 'Show more'}
        </button>
      )}
    </div>
  );
}

const CHIP_ANSWER_MAX_CHARS = 60;

function AnswerChips({ values }) {
  /** Renders a list of short answers as compact pill chips. */
  return (
    <div className="flex flex-wrap gap-1">
      {values.map((v, i) => (
        <span
          key={i}
          className="inline-flex max-w-full items-center gap-1 rounded-full border border-border/60 bg-muted/60 px-2 py-0.5 text-xs"
        >
          <Check className="size-2.5 shrink-0 text-primary" />
          <span className="truncate">{v}</span>
        </span>
      ))}
    </div>
  );
}

function AnsweredView({ questions, answers }) {
  return (
    <div className="relative py-1 pl-7">
      <span className="absolute left-1.5 top-2.5 grid size-3.5 place-items-center rounded-full bg-background">
        <CheckCircle2 className="size-3 text-green-500" />
      </span>
      <div className="min-w-0 space-y-2.5 overflow-hidden rounded-lg border border-border/60 bg-card/50 px-3 py-2.5">
        {questions.map((q, qi) => {
          const a = answers[q.id];
          const values = Array.isArray(a)
            ? a.map((x) => String(x))
            : a != null && String(a).trim().length > 0
              ? [String(a)]
              : [];
          const allShort =
            values.length > 0 &&
            values.every((v) => v.length <= CHIP_ANSWER_MAX_CHARS && !v.includes('\n'));
          return (
            <div key={q.id ?? qi} className="min-w-0 space-y-1">
              <div className="break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
                {q.text}
              </div>
              {values.length === 0 ? (
                <div className="text-xs italic text-muted-foreground">(no answer)</div>
              ) : allShort ? (
                <AnswerChips values={values} />
              ) : (
                <AnswerText text={values.join(', ')} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

export default AskUserInline;
