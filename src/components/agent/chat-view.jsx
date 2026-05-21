import React, { useEffect, useMemo, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ScrollArea } from '@/components/ui/scroll-area';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  DropdownMenuSeparator,
  DropdownMenuLabel,
} from '@/components/ui/dropdown-menu';
import { Plus, History, MoreHorizontal, Pencil, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useAgent } from '@/state/agent';
import { confirm } from '@/components/confirm-dialog';
import { ChatMessage } from './chat-message';
import { ChatInput } from './chat-input';
import { CostIndicator } from './cost-indicator';

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

function EmptyState() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-muted-foreground">
      <div className="text-sm font-medium text-foreground">Start a conversation</div>
      <div className="text-xs">
        Ask the agent to read code, run tools, or build something. Press Ctrl+Enter to send.
      </div>
    </div>
  );
}

function groupToolResults(messages) {
  const map = {};
  for (const m of messages || []) {
    for (const block of m.content || []) {
      if (block.type === 'tool_result') {
        map[block.tool_use_id] = {
          output: block.output,
          is_error: block.is_error,
        };
      }
    }
  }
  return map;
}

export function ChatView() {
  const activeTaskId = useAgent((s) => s.activeTaskId);
  const tasks = useAgent((s) => s.tasks);
  const setActiveTask = useAgent((s) => s.setActiveTask);
  const messages = useAgent((s) =>
    s.activeTaskId ? s.messagesByTask[s.activeTaskId] || [] : []
  );
  const isStreaming = useAgent((s) =>
    s.activeTaskId ? !!s.streamingByTask[s.activeTaskId] : false
  );
  const cost = useAgent((s) =>
    s.activeTaskId ? s.costByTask[s.activeTaskId] : null
  );
  const models = useAgent((s) => s.models);
  const selectedProvider = useAgent((s) => s.selectedProvider);
  const selectedModel = useAgent((s) => s.selectedModel);
  const setSelectedModel = useAgent((s) => s.setSelectedModel);
  const sendMessage = useAgent((s) => s.sendMessage);
  const abortActive = useAgent((s) => s.abortActive);
  const ensureTask = useAgent((s) => s.ensureTask);

  const scrollRef = useRef(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const viewport = el.querySelector('[data-radix-scroll-area-viewport]');
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }, [messages, isStreaming]);

  const toolResults = useMemo(() => groupToolResults(messages), [messages]);
  const modelValue = selectedProvider && selectedModel ? `${selectedProvider}::${selectedModel}` : '';

  const onModelChange = async (val) => {
    const [provider, modelId] = val.split('::');
    setSelectedModel(provider, modelId);
    if (activeTaskId && isTauri()) {
      try {
        await invoke('switch_model', { taskId: activeTaskId, providerType: provider, model: modelId });
      } catch (e) {}
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-2">
        <Select value={modelValue} onValueChange={onModelChange}>
          <SelectTrigger size="sm" className="h-7 max-w-[200px] text-xs">
            <SelectValue placeholder="Select model" />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectLabel>Models</SelectLabel>
              {(models || []).length === 0 && (
                <SelectItem value="none" disabled>
                  No models configured
                </SelectItem>
              )}
              {(models || []).map((m) => {
                const provider = m.provider_key || m.provider || 'unknown';
                const id = m.id || m.model_id;
                const label = m.name || m.display_name || id;
                return (
                  <SelectItem key={`${provider}::${id}`} value={`${provider}::${id}`}>
                    {label}
                  </SelectItem>
                );
              })}
            </SelectGroup>
          </SelectContent>
        </Select>
        <div className="ml-auto flex items-center gap-1.5">
          {cost && <CostIndicator cost={cost} />}
          <TaskSwitcher tasks={tasks} activeTaskId={activeTaskId} setActiveTask={setActiveTask} />
          <Button
            variant="ghost"
            size="icon-sm"
            className="size-7"
            title="New task"
            onClick={() => {
              useAgent.setState({ activeTaskId: null });
              ensureTask();
            }}
          >
            <Plus className="size-3.5" />
          </Button>
        </div>
      </div>

      <ScrollArea ref={scrollRef} className="flex-1">
        {messages.length === 0 ? (
          <EmptyState />
        ) : (
          <div className="flex flex-col">
            {messages.map((m) => (
              <ChatMessage key={m.id} message={m} toolResults={toolResults} />
            ))}
          </div>
        )}
      </ScrollArea>

      <ChatInput onSubmit={sendMessage} onAbort={abortActive} isStreaming={isStreaming} />
    </div>
  );
}

function TaskSwitcher({ tasks, activeTaskId, setActiveTask }) {
  const taskList = Array.isArray(tasks) ? tasks : [];

  const handleRename = async (taskId, currentTitle) => {
    const next = window.prompt('Rename task:', currentTitle ?? '');
    if (!next || next === currentTitle || !isTauri()) return;
    try {
      await invoke('rename_task', { taskId, title: next });
      const refreshed = await invoke('list_tasks', {
        projectId: useAgent.getState().activeProject.id,
      });
      useAgent.setState({ tasks: Array.isArray(refreshed) ? refreshed : [] });
    } catch (e) {
      toast.error(String(e));
    }
  };

  const handleDelete = async (taskId, title) => {
    const ok = await confirm({
      title: `Delete task "${title || taskId}"?`,
      description: 'All messages will be removed. This cannot be undone.',
      confirmLabel: 'Delete',
      destructive: true,
    });
    if (!ok || !isTauri()) return;
    try {
      await invoke('delete_task', { taskId });
      const projectId = useAgent.getState().activeProject.id;
      const refreshed = await invoke('list_tasks', { projectId });
      useAgent.setState({
        tasks: Array.isArray(refreshed) ? refreshed : [],
        activeTaskId: useAgent.getState().activeTaskId === taskId ? null : useAgent.getState().activeTaskId,
      });
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" className="size-7" title="Task history">
          <History className="size-3.5" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        <DropdownMenuLabel>Tasks</DropdownMenuLabel>
        {taskList.length === 0 && (
          <DropdownMenuItem disabled>No tasks yet</DropdownMenuItem>
        )}
        {taskList.map((t) => {
          const id = t.id ?? t.task_id;
          const title = t.title || `Task ${id?.slice?.(0, 6)}`;
          const active = id === activeTaskId;
          return (
            <DropdownMenuItem
              key={id}
              onSelect={(e) => {
                e.preventDefault();
                setActiveTask(id);
              }}
              className={active ? 'bg-muted' : ''}
            >
              <span className="flex-1 truncate">{title}</span>
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground hover:bg-muted-foreground/20 hover:text-foreground"
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  handleRename(id, title);
                }}
                title="Rename"
              >
                <Pencil className="size-3" />
              </button>
              <button
                type="button"
                className="rounded p-0.5 text-muted-foreground hover:bg-destructive/20 hover:text-destructive"
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  handleDelete(id, title);
                }}
                title="Delete"
              >
                <Trash2 className="size-3" />
              </button>
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export default ChatView;
