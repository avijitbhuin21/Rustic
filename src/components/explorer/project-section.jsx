import React, { useEffect, useState } from 'react';
import { ChevronRight, FolderGit2, Terminal, X } from 'lucide-react';
import { FileTree } from './file-tree';
import { useExplorer } from '@/state/explorer';
import { useTerminal } from '@/state/terminal';
import { toast } from 'sonner';

export function ProjectSection({ project, onOpenFile }) {
  const expanded = useExplorer((s) => !!s.expandedProjects[project.id]);
  const toggle = useExplorer((s) => s.toggleProjectExpanded);
  const removeProject = useExplorer((s) => s.removeProject);
  // Keep FileTree mounted once it's been opened so state (open folders, cache) survives collapse
  const [everExpanded, setEverExpanded] = useState(expanded);

  useEffect(() => {
    if (expanded) setEverExpanded(true);
  }, [expanded]);

  const handleOpenTerminal = async (e) => {
    e.stopPropagation();
    try {
      await useTerminal.getState().createTerminal({ cwd: project.root_path, label: project.name });
      toast.success(`Terminal opened in ${project.name}`);
    } catch (err) {
      toast.error(String(err));
    }
  };

  const handleRemove = async (e) => {
    e.stopPropagation();
    try {
      await removeProject(project.id);
    } catch (err) {
      toast.error(String(err));
    }
  };

  return (
    <div className="flex flex-col border-b border-border/60 last:border-b-0">
      <div
        onClick={() => toggle(project.id)}
        className="group/project sticky top-0 z-10 flex h-7 cursor-pointer items-center gap-1 border-b border-border/60 bg-muted/60 px-2 text-[11px] font-semibold uppercase tracking-wide text-foreground/90 backdrop-blur hover:bg-muted/80"
      >
        <ChevronRight
          className="size-3 shrink-0 transition-transform duration-200 ease-in-out"
          style={{ transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)' }}
        />
        <FolderGit2 className="size-3 shrink-0" />
        <span className="min-w-0 flex-1 truncate">{project.name}</span>
        <div className="ml-auto flex items-center gap-0.5 opacity-0 transition-opacity group-hover/project:opacity-100">
          <button
            onClick={handleOpenTerminal}
            title="Open terminal in project root"
            className="flex size-5 items-center justify-center rounded hover:bg-foreground/10"
          >
            <Terminal className="size-3" />
          </button>
          <button
            onClick={handleRemove}
            title="Remove project from workspace"
            className="flex size-5 items-center justify-center rounded hover:bg-destructive/20 hover:text-destructive"
          >
            <X className="size-3" />
          </button>
        </div>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateRows: expanded ? '1fr' : '0fr',
          transition: 'grid-template-rows 220ms ease',
        }}
      >
        <div style={{ overflow: 'hidden' }}>
          {everExpanded && (
            <FileTree rootPath={project.root_path} onOpenFile={onOpenFile} />
          )}
        </div>
      </div>
    </div>
  );
}
