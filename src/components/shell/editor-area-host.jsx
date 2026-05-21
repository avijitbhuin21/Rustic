import React from 'react';
import { useEditor } from '@/state/editor';
import { ResizablePanelGroup, ResizablePanel, ResizableHandle } from '@/components/ui/resizable';
import EditorPane from '@/components/editor/editor-pane';

export function EditorAreaHost() {
  const groups = useEditor((s) => s.groups ?? []);

  if (groups.length === 1) {
    return <EditorPane groupId={groups[0].id} />;
  }

  return (
    <ResizablePanelGroup direction="horizontal" className="h-full w-full">
      {groups.map((group, i) => (
        <React.Fragment key={group.id}>
          {i > 0 && <ResizableHandle />}
          <ResizablePanel id={group.id} defaultSize="50%" minSize="15%">
            <EditorPane groupId={group.id} />
          </ResizablePanel>
        </React.Fragment>
      ))}
    </ResizablePanelGroup>
  );
}
