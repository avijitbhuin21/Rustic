import React, { Suspense } from 'react';
import { Skeleton } from '@/components/ui/skeleton';

const DiffEditor = React.lazy(() =>
  import('@monaco-editor/react').then((m) => ({ default: m.DiffEditor }))
);

function Fallback() {
  return (
    <div className="flex h-full w-full gap-2 p-4">
      <div className="flex flex-1 flex-col gap-2">
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-2/3" />
      </div>
      <div className="flex flex-1 flex-col gap-2">
        <Skeleton className="h-4 w-2/3" />
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-3/4" />
      </div>
    </div>
  );
}

export default function DiffPreview({ original = '', modified = '', language = 'plaintext' }) {
  return (
    <Suspense fallback={<Fallback />}>
      <DiffEditor
        height="100%"
        theme="vs-dark"
        original={original}
        modified={modified}
        language={language}
        loading={<Fallback />}
        options={{
          renderSideBySide: true,
          readOnly: true,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          automaticLayout: true,
          fontSize: 13,
        }}
      />
    </Suspense>
  );
}
