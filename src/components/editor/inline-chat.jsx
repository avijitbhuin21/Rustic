// Inline chat (Ctrl+I) for the code editor.
//
// A prompt box opens above the selection (or at the cursor). The picked model
// either rewrites the selection — shown as an inline diff to Accept or
// Discard — or answers a question about it, shown as text without touching
// the file. Nothing is written to the buffer until Accept.
//
// Placement follows Monaco's zone-widget pattern: a view zone reserves the
// vertical space and an overlay widget, kept level with that zone, holds the
// interactive DOM (view-zone nodes themselves sit behind the text layer).

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { ModelPopover } from '@/components/agent/prompt-box';
import { MarkdownBlock } from '@/components/agent/markdown-block';
import { useInlineChat, useInlineChatModel } from '@/state/inline-chat';
import { isTauriAvailable, MOD_LABEL } from '@/lib/platform';

const DELTA_EVENT = 'inline-edit-delta';
// Context sent with the request, in lines around the selection. The backend
// applies its own character caps on top.
const CONTEXT_LINES_BEFORE = 400;
const CONTEXT_LINES_AFTER = 150;
const MAX_WIDGET_WIDTH = 720;
const MAX_DIFF_HEIGHT = 320;
const MAX_PROMPT_HEIGHT = 120;

/** Random id tagging one request's stream events (randomUUID is missing on plain-http origins). */
function newRequestId() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `inline-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** First `count` lines of `text`, for diffing a rewrite that is still streaming. */
function headLines(text, count) {
  const lines = text.split('\n');
  return lines.length <= count ? text : lines.slice(0, count).join('\n');
}

/** Read-only inline diff of the selection against the proposed rewrite. */
function InlineDiff({ monaco, editor, original, modified, streaming, language }) {
  const hostRef = useRef(null);
  const modelsRef = useRef(null);
  const [height, setHeight] = useState(40);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return undefined;
    const fontInfo = editor.getOption(monaco.editor.EditorOption.fontInfo);
    const sourceOptions = editor.getModel()?.getOptions();
    const originalModel = monaco.editor.createModel('', language);
    const modifiedModel = monaco.editor.createModel('', language);
    if (sourceOptions) {
      const indent = { tabSize: sourceOptions.tabSize, insertSpaces: sourceOptions.insertSpaces };
      originalModel.updateOptions(indent);
      modifiedModel.updateOptions(indent);
    }
    const diff = monaco.editor.createDiffEditor(host, {
      renderSideBySide: false,
      useInlineViewWhenSpaceIsLimited: false,
      // Indentation changes are exactly what the user needs to see.
      ignoreTrimWhitespace: false,
      readOnly: true,
      originalEditable: false,
      automaticLayout: true,
      renderOverviewRuler: false,
      hideUnchangedRegions: { enabled: false },
      minimap: { enabled: false },
      stickyScroll: { enabled: false },
      scrollBeyondLastLine: false,
      lineNumbers: 'off',
      glyphMargin: false,
      folding: false,
      renderLineHighlight: 'none',
      // Its pending timer rejects with "Canceled" when the diff is disposed.
      occurrencesHighlight: 'off',
      contextmenu: false,
      overviewRulerLanes: 0,
      fontFamily: fontInfo.fontFamily,
      fontSize: fontInfo.fontSize,
      lineHeight: fontInfo.lineHeight,
      fontLigatures: editor.getOption(monaco.editor.EditorOption.fontLigatures),
      scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
    });
    diff.setModel({ original: originalModel, modified: modifiedModel });
    modelsRef.current = { original: originalModel, modified: modifiedModel };

    const modifiedEditor = diff.getModifiedEditor();
    const syncHeight = () => {
      setHeight(Math.min(MAX_DIFF_HEIGHT, Math.max(fontInfo.lineHeight + 4, modifiedEditor.getContentHeight())));
    };
    const subs = [modifiedEditor.onDidContentSizeChange(syncHeight), diff.onDidUpdateDiff(syncHeight)];
    syncHeight();

    return () => {
      modelsRef.current = null;
      subs.forEach((s) => s.dispose());
      diff.dispose();
      originalModel.dispose();
      modifiedModel.dispose();
    };
  }, [monaco, editor, language]);

  useEffect(() => {
    const models = modelsRef.current;
    if (!models) return;
    // While the rewrite streams, compare it against only as much of the
    // original as has been produced, so the not-yet-written tail doesn't show
    // as deleted.
    const shownOriginal = streaming ? headLines(original, modified.split('\n').length) : original;
    if (models.original.getValue() !== shownOriginal) models.original.setValue(shownOriginal);
    if (models.modified.getValue() !== modified) models.modified.setValue(modified);
  }, [original, modified, streaming]);

  return <div ref={hostRef} style={{ height }} />;
}

/** Inline chat widget for one session, anchored at `session.range` in `editor`. */
export function InlineChat({ editor, monaco, tab, session, onClose }) {
  const LF = monaco.editor.EndOfLinePreference.LF;
  const [container] = useState(() => {
    const el = document.createElement('div');
    el.setAttribute('data-inline-chat', '');
    el.style.position = 'absolute';
    return el;
  });
  const targetRef = useRef(null);
  const requestRef = useRef(null);
  const originalRef = useRef('');
  const listenReadyRef = useRef(null);
  const textareaRef = useRef(null);

  const [prompt, setPrompt] = useState('');
  const [phase, setPhase] = useState('idle'); // idle | running | done | error
  const [result, setResult] = useState(null); // { mode, text, truncated }
  const [error, setError] = useState(null);
  const [modelOpen, setModelOpen] = useState(false);
  const model = useInlineChatModel();
  const setModelPick = useInlineChat((s) => s.setModelPick);
  const hasSelection =
    session.range.startLineNumber !== session.range.endLineNumber ||
    session.range.startColumn !== session.range.endColumn;

  // Mount the zone + overlay widget and keep them sized to the content.
  useLayoutEffect(() => {
    const target = editor.createDecorationsCollection([{
      range: session.range,
      options: {
        className: 'inline-chat-target',
        stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
      },
    }]);
    targetRef.current = target;

    // Monaco resets `top` on an overlay widget's own node, so the positioned
    // container lives inside an unpositioned shell.
    const shell = document.createElement('div');
    shell.appendChild(container);
    const widget = {
      getId: () => `rustic.inlineChat.${session.id}`,
      getDomNode: () => shell,
      getPosition: () => null,
    };
    editor.addOverlayWidget(widget);

    const zone = {
      afterLineNumber: session.range.startLineNumber - 1,
      heightInPx: 0,
      domNode: document.createElement('div'),
      onDomNodeTop: (top) => { container.style.top = `${top}px`; },
    };
    let zoneId = null;
    editor.changeViewZones((acc) => { zoneId = acc.addZone(zone); });

    const layout = () => {
      const info = editor.getLayoutInfo();
      const room = info.contentWidth - info.verticalScrollbarWidth - 12;
      container.style.left = `${info.contentLeft}px`;
      container.style.width = `${Math.max(220, Math.min(room, MAX_WIDGET_WIDTH))}px`;
    };
    layout();
    const layoutSub = editor.onDidLayoutChange(layout);

    let revealed = false;
    const observer = new ResizeObserver(() => {
      const next = Math.ceil(container.getBoundingClientRect().height);
      if (!next || next === zone.heightInPx) return;
      zone.heightInPx = next;
      editor.changeViewZones((acc) => acc.layoutZone(zoneId));
      if (!revealed) {
        // The zone sits above the selection's first line; if that line is at
        // the top of the viewport the widget would open just out of view.
        revealed = true;
        const zoneTop = editor.getTopForLineNumber(session.range.startLineNumber, true);
        if (zoneTop < editor.getScrollTop()) editor.setScrollTop(zoneTop);
      }
    });
    observer.observe(container);

    // Keys typed in the widget must not reach the surrounding editor's
    // keybinding listener (it sits on an ancestor node). Stopped on the shell,
    // one level above the portal container React itself listens on.
    const stopKey = (e) => e.stopPropagation();
    const keyEvents = ['keydown', 'keyup', 'keypress'];
    keyEvents.forEach((type) => shell.addEventListener(type, stopKey));

    return () => {
      keyEvents.forEach((type) => shell.removeEventListener(type, stopKey));
      observer.disconnect();
      layoutSub.dispose();
      editor.changeViewZones((acc) => { if (zoneId !== null) acc.removeZone(zoneId); });
      editor.removeOverlayWidget(widget);
      target.clear();
      targetRef.current = null;
    };
  }, [editor, monaco, container, session]);

  // Stream snapshots for the request in flight; cancel it if the widget goes away.
  useEffect(() => {
    let unlisten = null;
    let disposed = false;
    listenReadyRef.current = listen(DELTA_EVENT, (event) => {
      const payload = event?.payload;
      if (!payload || payload.requestId !== requestRef.current) return;
      setResult({ mode: payload.mode, text: payload.text ?? '', truncated: false });
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
      const id = requestRef.current;
      requestRef.current = null;
      if (id) invoke('cancel_inline_edit', { requestId: id }).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const id = requestAnimationFrame(() => textareaRef.current?.focus());
    return () => cancelAnimationFrame(id);
  }, []);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, MAX_PROMPT_HEIGHT)}px`;
  }, [prompt]);

  const cancelRequest = useCallback(() => {
    const id = requestRef.current;
    if (!id) return;
    requestRef.current = null;
    invoke('cancel_inline_edit', { requestId: id }).catch(() => {});
  }, []);

  const close = useCallback(() => {
    cancelRequest();
    onClose();
    editor.focus();
  }, [cancelRequest, onClose, editor]);

  const stop = useCallback(() => {
    cancelRequest();
    setResult(null);
    setPhase('idle');
    textareaRef.current?.focus();
  }, [cancelRequest]);

  const submit = useCallback(async () => {
    const instruction = prompt.trim();
    const textModel = editor.getModel();
    if (!instruction || !textModel || requestRef.current) return;
    if (!isTauriAvailable()) {
      setError('Inline chat needs the Rustic backend.');
      setPhase('error');
      return;
    }
    if (!model) {
      setError('Pick a model first.');
      setPhase('error');
      return;
    }
    const range = targetRef.current?.getRange(0) ?? session.range;
    const selection = textModel.getValueInRange(range, LF);
    const firstLine = Math.max(1, range.startLineNumber - CONTEXT_LINES_BEFORE);
    const lastLine = Math.min(textModel.getLineCount(), range.endLineNumber + CONTEXT_LINES_AFTER);
    const prefix = textModel.getValueInRange({
      startLineNumber: firstLine,
      startColumn: 1,
      endLineNumber: range.startLineNumber,
      endColumn: range.startColumn,
    }, LF);
    const suffix = textModel.getValueInRange({
      startLineNumber: range.endLineNumber,
      startColumn: range.endColumn,
      endLineNumber: lastLine,
      endColumn: textModel.getLineMaxColumn(lastLine),
    }, LF);

    const requestId = newRequestId();
    requestRef.current = requestId;
    originalRef.current = selection;
    setResult(null);
    setError(null);
    setPhase('running');
    try {
      await listenReadyRef.current;
      const out = await invoke('inline_edit', {
        providerType: model.provider,
        model: model.modelId,
        input: {
          requestId,
          path: tab.path ?? null,
          language: tab.language ?? null,
          prefix,
          selection,
          suffix,
          instruction,
        },
      });
      if (requestRef.current !== requestId) return;
      requestRef.current = null;
      setResult({ mode: out.mode, text: out.text ?? '', truncated: !!out.truncated });
      setPhase('done');
    } catch (err) {
      if (requestRef.current !== requestId) return;
      requestRef.current = null;
      setResult(null);
      setError(String(err?.message ?? err));
      setPhase('error');
    }
  }, [prompt, editor, model, session, tab.path, tab.language, LF]);

  const accept = useCallback(() => {
    const textModel = editor.getModel();
    const range = targetRef.current?.getRange(0);
    if (!textModel || !range || result?.mode !== 'edit') return;
    if (textModel.getValueInRange(range, LF) !== originalRef.current) {
      toast.error('The text changed after this suggestion was made. Send the request again.');
      return;
    }
    const lines = result.text.split('\n');
    const endLine = range.startLineNumber + lines.length - 1;
    const endColumn = lines.length === 1
      ? range.startColumn + lines[0].length
      : lines[lines.length - 1].length + 1;
    editor.pushUndoStop();
    const applied = editor.executeEdits('inline-chat', [{ range, text: result.text, forceMoveMarkers: true }]);
    editor.pushUndoStop();
    if (!applied) {
      toast.error('This file is read-only.');
      return;
    }
    editor.setPosition({ lineNumber: endLine, column: endColumn });
    onClose();
    editor.focus();
  }, [editor, result, onClose, LF]);

  const editReady = phase === 'done' && result?.mode === 'edit';

  const onKeyDown = (e) => {
    // Portalled children (the model popover) bubble here through React but
    // are not inside the widget; their keys are theirs.
    if (!container.contains(e.target)) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
      return;
    }
    if (e.key !== 'Enter' || e.nativeEvent?.isComposing) return;
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      if (editReady) accept();
      return;
    }
    if (e.target === textareaRef.current && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  return createPortal(
    <div
      onKeyDown={onKeyDown}
      className="flex flex-col gap-1.5 rounded-lg border border-border bg-popover p-2 font-sans text-xs leading-normal text-popover-foreground shadow-md"
    >
      <textarea
        ref={textareaRef}
        rows={1}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        spellCheck={false}
        aria-label="Inline chat instruction"
        placeholder={hasSelection
          ? 'Edit the selection, or ask about it…'
          : 'Describe what to insert here, or ask a question…'}
        className="w-full resize-none bg-transparent px-1 py-0.5 text-xs leading-relaxed text-foreground outline-none placeholder:text-muted-foreground"
      />

      {error && (
        <div className="select-text px-1 text-[11px] leading-snug text-destructive">{error}</div>
      )}

      {result?.mode === 'answer' && (
        <div className="max-h-64 select-text overflow-y-auto rounded-md border border-border/60 bg-background/40 px-2 py-1">
          <MarkdownBlock text={result.text} />
        </div>
      )}

      {result?.mode === 'edit' && (
        <div className="overflow-hidden rounded-md border border-border/60">
          <InlineDiff
            monaco={monaco}
            editor={editor}
            original={originalRef.current}
            modified={result.text}
            streaming={phase === 'running'}
            language={tab.language || 'plaintext'}
          />
        </div>
      )}

      {phase === 'done' && result?.truncated && (
        <div className="px-1 text-[11px] leading-snug text-warning">
          The model reached its output limit, so this may be incomplete.
        </div>
      )}

      <div className="flex items-center justify-between gap-2">
        <div className="inline-flex min-w-0 items-center overflow-hidden rounded-md border border-border/60 bg-muted/20">
          <ModelPopover
            open={modelOpen}
            onOpenChange={setModelOpen}
            selection={model}
            onSelect={setModelPick}
            side="bottom"
          />
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {phase === 'running' ? (
            <>
              <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                <Loader2 className="size-3 animate-spin" />
                Generating
              </span>
              <Button variant="ghost" size="xs" onClick={stop}>Stop</Button>
            </>
          ) : editReady ? (
            <>
              <Button variant="ghost" size="xs" onClick={close}>
                Discard
                <span className="text-muted-foreground">Esc</span>
              </Button>
              <Button size="xs" onClick={accept}>
                Accept
                <span className="opacity-70">{MOD_LABEL}⏎</span>
              </Button>
            </>
          ) : (
            <>
              <Button variant="ghost" size="xs" onClick={close}>
                Close
                <span className="text-muted-foreground">Esc</span>
              </Button>
              <Button size="xs" onClick={submit} disabled={!prompt.trim()}>
                Send
                <span className="opacity-70">⏎</span>
              </Button>
            </>
          )}
        </div>
      </div>
    </div>,
    container,
  );
}
