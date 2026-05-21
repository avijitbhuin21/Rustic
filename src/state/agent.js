import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export const PLACEHOLDER_PROJECT = {
  id: '',
  name: '',
  root: '',
};

const AGENT_EVENTS = [
  'agent-stream-text',
  'agent-tool-use',
  'agent-tool-result',
  'agent-cost-update',
  'agent-status',
  'agent-task-complete',
  'agent-permission-request',
  'agent-question-request',
  'agent-thinking-delta',
  'agent-todo-updated',
  'agent-title-changed',
];

function safeInvoke(cmd, args) {
  try {
    return invoke(cmd, args);
  } catch (e) {
    return Promise.reject(e);
  }
}

function isTauriAvailable() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export const useAgent = create((set, get) => ({
  activeProject: { id: '', name: '', root: '' },
  tasks: [],
  activeTaskId: null,
  messagesByTask: {},
  todosByTask: {},
  costByTask: {},
  statusByTask: {},
  streamingByTask: {},
  thinkingByTask: {},
  pendingPermission: null,
  pendingQuestion: null,
  models: [],
  selectedModel: null,
  selectedProvider: null,
  listenersBound: false,
  initialized: false,

  setActiveTask: (taskId) => set({ activeTaskId: taskId }),

  setActiveProject: (project) => {
    const next = project ?? { id: '', name: '', root: '' };
    const prev = get().activeProject;
    if (prev.id === next.id) return;
    set({
      activeProject: next,
      tasks: [],
      activeTaskId: null,
      messagesByTask: {},
      todosByTask: {},
      costByTask: {},
      statusByTask: {},
      streamingByTask: {},
      thinkingByTask: {},
      initialized: false,
    });
  },

  appendUserMessage: (taskId, text, attachments = []) => {
    const msg = {
      id: `local-${Date.now()}`,
      role: 'user',
      content: [{ type: 'text', text }],
      attachments,
      timestamp: Date.now(),
    };
    set((s) => ({
      messagesByTask: {
        ...s.messagesByTask,
        [taskId]: [...(s.messagesByTask[taskId] || []), msg],
      },
    }));
  },

  appendAssistantText: (taskId, delta) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant' && last.streaming) {
        const content = [...(last.content || [])];
        const lastBlock = content[content.length - 1];
        if (lastBlock && lastBlock.type === 'text') {
          content[content.length - 1] = { ...lastBlock, text: (lastBlock.text || '') + delta };
        } else {
          content.push({ type: 'text', text: delta });
        }
        list[list.length - 1] = { ...last, content };
      } else {
        list.push({
          id: `assist-${Date.now()}`,
          role: 'assistant',
          content: [{ type: 'text', text: delta }],
          streaming: true,
          timestamp: Date.now(),
        });
      }
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
        streamingByTask: { ...s.streamingByTask, [taskId]: true },
      };
    });
  },

  appendThinking: (taskId, delta) => {
    set((s) => ({
      thinkingByTask: {
        ...s.thinkingByTask,
        [taskId]: (s.thinkingByTask[taskId] || '') + delta,
      },
    }));
  },

  addToolUse: (taskId, toolUseId, name, input) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.role === 'assistant') {
        last.streaming = false;
      }
      list.push({
        id: `tool-${toolUseId}`,
        role: 'assistant',
        content: [{ type: 'tool_use', id: toolUseId, name, input }],
        timestamp: Date.now(),
      });
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  addToolResult: (taskId, toolUseId, output, isError) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      list.push({
        id: `tool-result-${toolUseId}`,
        role: 'tool',
        content: [{ type: 'tool_result', tool_use_id: toolUseId, output, is_error: !!isError }],
        timestamp: Date.now(),
      });
      return { messagesByTask: { ...s.messagesByTask, [taskId]: list } };
    });
  },

  finishStream: (taskId) => {
    set((s) => {
      const list = s.messagesByTask[taskId] ? [...s.messagesByTask[taskId]] : [];
      const last = list[list.length - 1];
      if (last && last.streaming) {
        list[list.length - 1] = { ...last, streaming: false };
      }
      return {
        messagesByTask: { ...s.messagesByTask, [taskId]: list },
        streamingByTask: { ...s.streamingByTask, [taskId]: false },
      };
    });
  },

  setCost: (taskId, cost) =>
    set((s) => ({ costByTask: { ...s.costByTask, [taskId]: cost } })),

  setStatus: (taskId, status) =>
    set((s) => ({ statusByTask: { ...s.statusByTask, [taskId]: status } })),

  setTodos: (taskId, todos) =>
    set((s) => ({ todosByTask: { ...s.todosByTask, [taskId]: todos } })),

  setTitle: (taskId, title) =>
    set((s) => ({
      tasks: s.tasks.map((t) => (t.id === taskId ? { ...t, title } : t)),
    })),

  openPermission: (req) => set({ pendingPermission: req }),
  closePermission: () => set({ pendingPermission: null }),
  openQuestion: (req) => set({ pendingQuestion: req }),
  closeQuestion: () => set({ pendingQuestion: null }),

  setModels: (models) => set({ models }),
  setSelectedModel: (provider, modelId) =>
    set({ selectedProvider: provider, selectedModel: modelId }),

  async ensureTask() {
    const state = get();
    if (state.activeTaskId) return state.activeTaskId;
    if (!isTauriAvailable()) {
      const id = `mock-${Date.now()}`;
      set((s) => ({
        tasks: [...s.tasks, { id, title: 'New Task' }],
        activeTaskId: id,
      }));
      return id;
    }
    const project = get().activeProject;
    if (!project.id) {
      const id = `local-${Date.now()}`;
      set((s) => ({
        tasks: [...s.tasks, { id, title: 'New Task' }],
        activeTaskId: id,
      }));
      return id;
    }
    try {
      const task = await safeInvoke('create_task', {
        projectId: project.id,
        projectName: project.name,
        projectRoot: project.root,
        title: 'New Task',
      });
      set((s) => ({
        tasks: [...s.tasks, task],
        activeTaskId: task.id,
      }));
      return task.id;
    } catch (e) {
      const id = `local-${Date.now()}`;
      set((s) => ({
        tasks: [...s.tasks, { id, title: 'New Task' }],
        activeTaskId: id,
      }));
      return id;
    }
  },

  async sendMessage(text, attachments = []) {
    const state = get();
    const taskId = await state.ensureTask();
    state.appendUserMessage(taskId, text, attachments);
    if (!isTauriAvailable()) return;
    try {
      await safeInvoke('send_message', {
        taskId,
        message: text,
        thinkingBudget: null,
        images: attachments ?? [],
      });
    } catch (e) {
      // streaming will surface errors via events
    }
  },

  async abortActive() {
    const taskId = get().activeTaskId;
    if (!taskId || !isTauriAvailable()) return;
    try {
      await safeInvoke('abort_task', { taskId });
    } catch (e) {}
  },

  async respondPermission(approved) {
    const req = get().pendingPermission;
    if (!req) return;
    set({ pendingPermission: null });
    if (!isTauriAvailable()) return;
    try {
      await safeInvoke('respond_to_permission', {
        taskId: req.task_id,
        requestId: req.request_id,
        approved,
      });
    } catch (e) {}
  },

  async respondQuestion(userInput, opts = {}) {
    const req = get().pendingQuestion;
    if (!req) return;
    set({ pendingQuestion: null });
    if (!isTauriAvailable()) return;
    const cancelled = !!opts.cancelled;
    try {
      await safeInvoke('respond_to_ask_user', {
        requestId: req.request_id,
        // `answers` is a serde_json::Value on the Rust side: passing the raw
        // string is accepted as a JSON string scalar; null when cancelled.
        answers: cancelled ? null : userInput,
        cancelled,
      });
    } catch (e) {}
  },

  async loadInitial() {
    if (get().initialized) return;
    set({ initialized: true });
    if (!isTauriAvailable()) return;
    try {
      const projectId = get().activeProject.id;
      if (!projectId) {
        set({ tasks: [] });
        return;
      }
      const tasks = await safeInvoke('list_tasks', { projectId });
      set({ tasks: Array.isArray(tasks) ? tasks : [] });
    } catch (e) {}
    try {
      const known = await safeInvoke('list_known_models');
      set({ models: Array.isArray(known) ? known : [] });
    } catch (e) {}
  },

  async bindListeners() {
    if (get().listenersBound) return () => {};
    set({ listenersBound: true });
    if (!isTauriAvailable()) return () => {};

    const handlers = {
      'agent-stream-text': (p) => get().appendAssistantText(p.task_id, p.text || ''),
      'agent-thinking-delta': (p) => get().appendThinking(p.task_id, p.text || ''),
      'agent-tool-use': (p) =>
        get().addToolUse(p.task_id, p.tool_use_id, p.tool_name, p.tool_input),
      'agent-tool-result': (p) =>
        get().addToolResult(p.task_id, p.tool_use_id, p.output, p.is_error),
      'agent-cost-update': (p) => get().setCost(p.task_id, p.cost),
      'agent-status': (p) => get().setStatus(p.task_id, p.status),
      'agent-task-complete': (p) => {
        get().finishStream(p.task_id);
        get().setStatus(p.task_id, 'complete');
      },
      'agent-permission-request': (p) => get().openPermission(p),
      'agent-question-request': (p) => get().openQuestion(p),
      'agent-todo-updated': (p) => get().setTodos(p.task_id, p.todos || []),
      'agent-title-changed': (p) => get().setTitle(p.task_id, p.title),
    };

    const unlisteners = await Promise.all(
      AGENT_EVENTS.map((name) =>
        listen(name, (evt) => {
          const handler = handlers[name];
          if (handler) handler(evt.payload || {});
        })
      )
    );

    return () => {
      unlisteners.forEach((un) => {
        try {
          un();
        } catch (e) {}
      });
      set({ listenersBound: false });
    };
  },
}));
