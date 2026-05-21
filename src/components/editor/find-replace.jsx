import React from 'react';
import { Search, Replace } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';

export function FindReplace({ editorRef }) {
  const runFind = () => {
    editorRef?.current?.getAction('actions.find')?.run();
  };
  const runReplace = () => {
    editorRef?.current?.getAction('editor.action.startFindReplaceAction')?.run();
  };

  return (
    <div className="flex items-center gap-1">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button size="icon-xs" variant="ghost" onClick={runFind}>
            <Search />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom">Find (Ctrl+F)</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button size="icon-xs" variant="ghost" onClick={runReplace}>
            <Replace />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom">Replace (Ctrl+H)</TooltipContent>
      </Tooltip>
    </div>
  );
}

export default FindReplace;
