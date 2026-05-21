import React from 'react';
import { AlertCircle, FileEdit } from 'lucide-react';
import { useGit } from '@/state/git';
import { useEditor } from '@/state/editor';

export function StatusBar() {
  const projectGit = useGit((s) => s.projects[s.activeProjectId]);
  const groups      = useEditor((s) => s.groups);
  const activeGroupId = useEditor((s) => s.activeGroupId);
  const cursor      = useEditor((s) => s.cursor);

  const allTabs   = (groups ?? []).flatMap((g) => g.tabs);
  const dirtyCount = allTabs.filter((t) => t.dirty).length;
  const activeGroup = groups.find((g) => g.id === activeGroupId);
  const activeTab   = activeGroup?.tabs.find((t) => t.id === activeGroup.activeId) ?? null;
  const conflicts = projectGit?.conflicts?.length ?? 0;

  return (
    <div className="flex h-6 shrink-0 items-center justify-between border-t border-border bg-background px-2 text-[11px] text-muted-foreground select-none">
      <div className="flex items-center gap-3">
        {conflicts > 0 && (
          <span className="flex items-center gap-1 text-destructive">
            <AlertCircle className="size-3" />
            {conflicts} conflict{conflicts === 1 ? '' : 's'}
          </span>
        )}
        {dirtyCount > 0 && (
          <span className="flex items-center gap-1 text-foreground">
            <FileEdit className="size-3" />
            {dirtyCount} unsaved
          </span>
        )}
      </div>
      <div className="flex items-center gap-3">
        {activeTab && activeTab.kind === 'code' && (
          <>
            <span>Ln {cursor.line}, Col {cursor.column}</span>
            <span>{(activeTab.language ?? 'plaintext').toUpperCase()}</span>
          </>
        )}
        <span>UTF-8</span>
        <span>LF</span>
        <span>Rustic v0.3.1</span>
      </div>
    </div>
  );
}
