import React, { useEffect, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { FolderPlus, RefreshCw, ListCollapse } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { Skeleton } from '@/components/ui/skeleton';
import { useExplorer } from '@/state/explorer';
import { cn } from '@/lib/utils';
import { ProjectSection } from './project-section';

export function Explorer({ onOpenFile }) {
  const projects = useExplorer((s) => s.projects);
  const loading = useExplorer((s) => s.loading);
  const error = useExplorer((s) => s.error);
  const loadProjects = useExplorer((s) => s.loadProjects);
  const addProject = useExplorer((s) => s.addProject);
  const collapseAllProjects = useExplorer((s) => s.collapseAllProjects);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  const handleAddProject = async () => {
    try {
      const path = await open({ directory: true, multiple: false });
      if (typeof path === 'string') await addProject(path);
    } catch (err) {
      console.error('add project failed:', err);
    }
  };

  const handleCollapseAll = () => collapseAllProjects();

  const [spinning, setSpinning] = useState(false);
  const handleRefresh = async () => {
    setSpinning(true);
    const minDelay = new Promise((r) => setTimeout(r, 700));
    try {
      await Promise.all([loadProjects(), minDelay]);
    } finally {
      setSpinning(false);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-border/60 px-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
          Explorer
        </span>
        <div className="flex items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={handleAddProject}>
                <FolderPlus className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" align="start" sideOffset={4} className="px-2 py-1">Add Project Folder</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" onClick={handleCollapseAll}>
                <ListCollapse className="size-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">Collapse All</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                onClick={handleRefresh}
                disabled={spinning}
              >
                <RefreshCw className={cn('size-3', spinning && 'animate-spin')} />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" align="end" sideOffset={4} className="px-2 py-1">Refresh</TooltipContent>
          </Tooltip>
        </div>
      </div>
      <div className="explorer-scroll min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
        {loading && projects.length === 0 && (
          <div className="flex flex-col gap-1 px-2 py-2">
            <Skeleton className="h-5 w-3/4" />
            <Skeleton className="ml-3 h-4 w-2/3" />
            <Skeleton className="ml-3 h-4 w-1/2" />
            <Skeleton className="ml-3 h-4 w-4/5" />
            <Skeleton className="h-5 w-2/3" />
            <Skeleton className="ml-3 h-4 w-1/2" />
          </div>
        )}
        {error && (
          <div className="px-3 py-4 text-xs text-destructive">Error: {error}</div>
        )}
        {!loading && projects.length === 0 && !error && (
          <div className="flex flex-col items-start gap-2 px-3 py-4 text-xs text-muted-foreground">
            <span>No projects added yet.</span>
            <Button variant="outline" size="sm" onClick={handleAddProject}>
              <FolderPlus className="size-3" />
              Add Folder
            </Button>
          </div>
        )}
        {projects.map((p) => (
          <ProjectSection key={p.id} project={p} onOpenFile={onOpenFile} />
        ))}
      </div>
    </div>
  );
}
