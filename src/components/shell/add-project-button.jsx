import React from 'react';
import { FolderPlus } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { useExplorer } from '@/state/explorer';

export function AddProjectButton() {
  const addProject = useExplorer((s) => s.addProject);

  async function handleClick() {
    try {
      const path = await open({ directory: true, multiple: false });
      if (typeof path === 'string') await addProject(path);
    } catch (err) {
      console.error('add project failed:', err);
    }
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon-xs" onClick={handleClick}>
          <FolderPlus className="size-3" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4} className="px-2 py-1">
        Add Project Folder
      </TooltipContent>
    </Tooltip>
  );
}
