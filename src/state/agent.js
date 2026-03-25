import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { uiStore } from './ui.js';

export const agentStore = createStore({
  tasks: {},            // taskId -> { id, projectId, title, status, messages: [], isStreaming }
  activeTaskId: null,
});

// Initialize event listeners
let eventsInitialized = false;

export async function initAgentEvents() {
  if (eventsInitialized) return;
  eventsInitialized = true;

  api.onAgentStream((payload) => {
    const { task_id, text } = payload;
    appendStreamText(task_id, text);
  });

  api.onAgentToolUse((payload) => {
    const { task_id, tool_name, tool_input } = payload;
    appendToolUse(task_id, tool_name, tool_input);
  });

  api.onAgentToolResult((payload) => {
    const { task_id, tool_use_id, output, is_error } = payload;
    appendToolResult(task_id, tool_use_id, output, is_error);
  });

  api.onAgentTaskStatus((payload) => {
    const { task_id, status } = payload;
    updateTaskStatus(task_id, status);
  });
}

export async function createTask(projectId, title) {
  try {
    const info = await api.createTask(projectId, title);
    if (!info) return null;

    const tasks = { ...agentStore.getState('tasks') };
    tasks[info.id] = {
      ...info,
      messages: [],
      isStreaming: false,
    };
    agentStore.setState({ tasks, activeTaskId: info.id });

    // Show secondary sidebar
    uiStore.setState({ secondarySidebarVisible: true });

    return info;
  } catch (e) {
    console.error('Failed to create task:', e);
    return null;
  }
}

export async function sendMessage(taskId, message) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  // Add user message locally
  task.messages = [...task.messages, { role: 'user', content: [{ type: 'text', text: message }] }];
  task.isStreaming = true;
  task.status = 'Running';
  // Add placeholder for assistant response
  task.messages.push({ role: 'assistant', content: [{ type: 'text', text: '' }] });
  agentStore.setState({ tasks: { ...tasks } });

  try {
    await api.sendMessage(taskId, message);
  } catch (e) {
    console.error('Failed to send message:', e);
    task.isStreaming = false;
    task.status = 'Failed';
    agentStore.setState({ tasks: { ...tasks } });
  }
}

function appendStreamText(taskId, text) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  // Find the last assistant message and append text
  const msgs = [...task.messages];
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') {
      const content = [...msgs[i].content];
      const lastBlock = content[content.length - 1];
      if (lastBlock && lastBlock.type === 'text') {
        content[content.length - 1] = { ...lastBlock, text: lastBlock.text + text };
      } else {
        content.push({ type: 'text', text });
      }
      msgs[i] = { ...msgs[i], content };
      break;
    }
  }

  task.messages = msgs;
  agentStore.setState({ tasks: { ...tasks } });
}

function appendToolUse(taskId, toolName, toolInput) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  // Add tool use to the last assistant message
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') {
      msgs[i] = {
        ...msgs[i],
        content: [...msgs[i].content, { type: 'tool_use', name: toolName, input: toolInput }],
      };
      break;
    }
  }
  task.messages = msgs;
  agentStore.setState({ tasks: { ...tasks } });
}

function appendToolResult(taskId, toolUseId, output, isError) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  task.messages = [
    ...task.messages,
    {
      role: 'tool',
      content: [{ type: 'tool_result', tool_use_id: toolUseId, content: output, is_error: isError }],
    },
  ];
  agentStore.setState({ tasks: { ...tasks } });
}

function updateTaskStatus(taskId, status) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  task.status = status;
  task.isStreaming = status === 'Running';
  agentStore.setState({ tasks: { ...tasks } });
}

export function setActiveTask(taskId) {
  agentStore.setState({ activeTaskId: taskId });
  uiStore.setState({ secondarySidebarVisible: true });
}

export async function deleteTaskAction(taskId) {
  try {
    await api.deleteTask(taskId);
    const tasks = { ...agentStore.getState('tasks') };
    delete tasks[taskId];
    const activeId = agentStore.getState('activeTaskId');
    agentStore.setState({
      tasks,
      activeTaskId: activeId === taskId ? null : activeId,
    });
  } catch (e) {
    console.error('Failed to delete task:', e);
  }
}
