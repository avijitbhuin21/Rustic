import React, { useRef } from 'react';
import { create } from 'zustand';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';

// Reuse the pre-fetched Monaco React bundle (monaco-editor.jsx kicks the
// import off at module load) — only the DiffEditor export is needed here.
const DiffEditorLazy = React.lazy(() =>
  import('@monaco-editor/react').then((m) => ({ default: m.DiffEditor }))
);

const useSaveConflictStore = create((set) => ({
  open: false,
  title: '',
  path: '',
  language: 'plaintext',
  diskContent: '',
  bufferContent: '',
  resolver: null,

  request: (opts) =>
    new Promise((resolve) => {
      set({
        open: true,
        title: opts.title ?? 'File changed on disk',
        path: opts.path ?? '',
        language: opts.language ?? 'plaintext',
        diskContent: opts.diskContent ?? '',
        bufferContent: opts.bufferContent ?? '',
        resolver: resolve,
      });
    }),

  resolve: (value) =>
    set((state) => {
      state.resolver?.(value);
      return { open: false, resolver: null, diskContent: '', bufferContent: '' };
    }),
}));

/// Opens the save-conflict diff dialog; resolves to {action:'save',content}, {action:'take-disk'}, or null on cancel.
export function resolveSaveConflict(opts) {
  return useSaveConflictStore.getState().request(opts);
}

export function SaveConflictDialogHost() {
  const open = useSaveConflictStore((s) => s.open);
  const title = useSaveConflictStore((s) => s.title);
  const language = useSaveConflictStore((s) => s.language);
  const diskContent = useSaveConflictStore((s) => s.diskContent);
  const bufferContent = useSaveConflictStore((s) => s.bufferContent);
  const resolve = useSaveConflictStore((s) => s.resolve);
  const diffRef = useRef(null);

  const saveMine = () => {
    const content =
      diffRef.current?.getModifiedEditor?.().getValue?.() ?? bufferContent;
    resolve({ action: 'save', content });
  };

  return (
    <Dialog open={open} onOpenChange={(o) => { if (!o) resolve(null); }}>
      <DialogContent
        showCloseButton={false}
        className="flex h-[80vh] flex-col gap-3 p-5 sm:max-w-[90vw]"
      >
        <DialogHeader className="gap-1.5">
          <DialogTitle className="text-sm font-medium">
            "{title}" changed on disk since you started editing
          </DialogTitle>
          <DialogDescription className="text-[13px] leading-snug">
            Left: the version now on disk. Right: your unsaved version — edit it
            here to merge, then save.
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-hidden rounded-md border border-border/60">
          {open && (
            <React.Suspense fallback={null}>
              <DiffEditorLazy
                height="100%"
                theme="rustic-dark"
                language={language}
                original={diskContent}
                modified={bufferContent}
                onMount={(editor) => { diffRef.current = editor; }}
                options={{
                  originalEditable: false,
                  readOnly: false,
                  renderSideBySide: true,
                  automaticLayout: true,
                  minimap: { enabled: false },
                  scrollBeyondLastLine: false,
                }}
              />
            </React.Suspense>
          )}
        </div>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={() => resolve(null)}>
            Cancel
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => resolve({ action: 'take-disk' })}
          >
            Take Disk Version
          </Button>
          <Button variant="default" size="sm" onClick={saveMine} autoFocus>
            Save My Version
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
