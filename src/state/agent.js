import { createStore } from './store.js';
import * as api from '../lib/tauri-api.js';
import { uiStore } from './ui.js';
import { refreshAffectedDirectory, refreshProject, workspaceStore } from './workspace.js';

export const agentStore = createStore({
  tasks: {},            // taskId -> { id, projectId, title, status, messages: [], isStreaming, cost }
  activeTaskId: null,
  permissionRequests: {}, // taskId -> [{ request_id, operation, description, preview, countdown }]
  turnBudgetWarnings: {}, // taskId -> { turns_remaining }
  subagents: {},          // taskId -> { agentId -> { agentId, model, status, output } }
  toolProgress: {},       // tool_use_id -> { progress_text }
  todos: {},              // taskId -> [{ content, status }]
  pendingQuestions: {},   // taskId -> { request_id, question }
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
    const { task_id, tool_use_id, tool_name, tool_input } = payload;
    appendToolUse(task_id, tool_use_id, tool_name, tool_input);
  });

  api.onAgentToolResult((payload) => {
    const { task_id, tool_use_id, output, is_error } = payload;
    appendToolResult(task_id, tool_use_id, output, is_error);
    // Clear progress when result arrives
    const progress = { ...agentStore.getState('toolProgress') };
    delete progress[tool_use_id];
    agentStore.setState('toolProgress', progress);
    _maybeRefreshFileTree(task_id, tool_use_id);
  });

  api.onAgentToolProgress((payload) => {
    const { tool_use_id, progress_text } = payload;
    const progress = { ...agentStore.getState('toolProgress') };
    progress[tool_use_id] = { progress_text };
    agentStore.setState('toolProgress', progress);
  });

  api.onAgentTaskStatus((payload) => {
    const { task_id, status } = payload;
    updateTaskStatus(task_id, status);
  });

  api.onAgentTaskComplete((payload) => {
    const { task_id, diff } = payload;
    appendTaskComplete(task_id, diff);
    _refreshProjectForTask(task_id);
  });

  api.onAgentPermissionRequest((payload) => {
    addPermissionRequest(payload);
  });

  api.onAgentCostUpdate((payload) => {
    const { task_id, cost } = payload;
    updateTaskCost(task_id, cost);
  });

  api.onAgentRequestUsage((payload) => {
    const { task_id, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens } = payload;
    const lastRequestUsage = { ...(agentStore.getState('lastRequestUsage') || {}) };
    lastRequestUsage[task_id] = {
      input: inputTokens,
      output: outputTokens,
      cacheRead: cacheReadTokens,
      cacheWrite: cacheWriteTokens,
      ts: Date.now(),
    };
    agentStore.setState({ lastRequestUsage });
    // Also log for quick console-level visibility.
    console.log(
      `[agent:${task_id}] request — in=${inputTokens} out=${outputTokens} cache_read=${cacheReadTokens} cache_write=${cacheWriteTokens}`
    );
  });

  api.onAgentTurnBudgetWarning((payload) => {
    const { task_id, turns_remaining } = payload;
    const warnings = { ...agentStore.getState('turnBudgetWarnings') };
    if (turns_remaining === 0) {
      // Limit reached — clear warning (status change handles the UI)
      delete warnings[task_id];
    } else {
      warnings[task_id] = { turns_remaining };
    }
    agentStore.setState({ turnBudgetWarnings: warnings });
  });

  api.onAgentTodoUpdated((payload) => {
    const { task_id, todos: items } = payload;
    const todos = { ...agentStore.getState('todos') };
    todos[task_id] = items;
    agentStore.setState({ todos });
  });

  api.onAgentTitleChanged((payload) => {
    const { task_id, title } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      tasks[task_id] = { ...task, title };
      agentStore.setState({ tasks });
    }
  });

  api.onAgentQuestionRequest((payload) => {
    const { task_id, request_id, question } = payload;
    handleQuestionRequest(task_id, request_id, question);
  });

  api.onAgentMemoryUpdated(() => {
    showMemoryToast();
  });

  api.onAgentThinkingDelta((payload) => {
    const { task_id, text } = payload;
    appendThinkingDelta(task_id, text);
  });

  api.onAgentThinkingDone((payload) => {
    const { task_id, duration_secs } = payload;
    stampThinkingDuration(task_id, duration_secs);
  });

  api.onAgentContextCondenseStarted((payload) => {
    const { task_id } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      task.messages = [
        ...task.messages,
        { role: 'system', content: [{ type: 'context_condense', status: 'running' }] },
      ];
      agentStore.setState({ tasks: { ...tasks } });
    }
  });

  api.onAgentContextCondenseCompleted((payload) => {
    const { task_id, original_messages, condensed_to } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      // Find the running condense marker and update it
      const msgs = [...task.messages];
      let found = false;
      for (let i = msgs.length - 1; i >= 0; i--) {
        const b = msgs[i].content?.[0];
        if (b?.type === 'context_condense' && b.status === 'running') {
          msgs[i] = {
            ...msgs[i],
            content: [{ type: 'context_condense', status: 'completed', original_messages, condensed_to }],
          };
          found = true;
          break;
        }
      }
      if (!found) {
        msgs.push({
          role: 'system',
          content: [{ type: 'context_condense', status: 'completed', original_messages, condensed_to }],
        });
      }
      task.messages = msgs;
      agentStore.setState({ tasks: { ...tasks } });
    }
  });

  api.onAgentModelSwitched((payload) => {
    const { task_id, from_model, to_model, provider_type } = payload;
    const tasks = { ...agentStore.getState('tasks') };
    const task = tasks[task_id];
    if (task) {
      task.model = to_model;
      task.provider_type = provider_type;
      // Append the ModelSwitch marker to the local message list so the chat view re-renders
      task.messages = [
        ...task.messages,
        {
          role: 'user',
          content: [{ type: 'model_switch', from_model, to_model }],
        },
      ];
      agentStore.setState({ tasks: { ...tasks } });
    }
  });

  initSubagentEvents();
}

async function initSubagentEvents() {
  api.onAgentSubagentSpawned((payload) => {
    const { task_id, agent_id, model, prompt } = payload;
    console.log('[subagent] spawned:', agent_id, 'model:', model, 'task:', task_id);
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    taskAgents[agent_id] = { agentId: agent_id, model, status: 'running', output: '', prompt: prompt || '' };
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentCompleted((payload) => {
    const { task_id, agent_id, summary } = payload;
    console.log('[subagent] completed:', agent_id, 'summary_len:', summary?.length);
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      taskAgents[agent_id] = { ...taskAgents[agent_id], status: 'completed', output: taskAgents[agent_id].output + (summary ? '\n\n' + summary : '') };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentFailed((payload) => {
    const { task_id, agent_id, error } = payload;
    console.log('[subagent] failed:', agent_id, 'error:', error);
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      taskAgents[agent_id] = { ...taskAgents[agent_id], status: 'failed', output: taskAgents[agent_id].output + '\n\nError: ' + error };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentTextDelta((payload) => {
    const { task_id, agent_id, text } = payload;
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      taskAgents[agent_id] = { ...taskAgents[agent_id], output: taskAgents[agent_id].output + text };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });

  api.onAgentSubagentCostUpdate((payload) => {
    const { task_id, agent_id, cost } = payload;
    const subagents = { ...agentStore.getState('subagents') };
    const taskAgents = { ...(subagents[task_id] || {}) };
    if (taskAgents[agent_id]) {
      taskAgents[agent_id] = { ...taskAgents[agent_id], cost };
    }
    subagents[task_id] = taskAgents;
    agentStore.setState({ subagents });
  });
}

function showMemoryToast() {
  const existing = document.getElementById('memory-toast');
  if (existing) {
    clearTimeout(existing._timeout);
    existing.remove();
  }
  const toast = document.createElement('div');
  toast.id = 'memory-toast';
  toast.className = 'memory-toast';
  toast.textContent = 'Memory updated';
  document.body.appendChild(toast);
  // Trigger animation
  requestAnimationFrame(() => toast.classList.add('memory-toast--visible'));
  toast._timeout = setTimeout(() => {
    toast.classList.remove('memory-toast--visible');
    setTimeout(() => toast.remove(), 300);
  }, 2500);
}

export async function createTask(projectId, projectName, projectRoot, title) {
  try {
    const info = await api.createTask(projectId, projectName, projectRoot, title);
    if (!info) return null;

    // Load project defaults (permission level is applied by the backend;
    // thinking effort needs to be applied on the frontend via projectDefaults)
    let projectDefaults = null;
    try {
      projectDefaults = await api.getProjectDefaults(projectId);
    } catch {}

    const tasks = { ...agentStore.getState('tasks') };
    tasks[info.id] = {
      ...info,
      messages: [],
      isStreaming: false,
      // Apply persisted permission level from project defaults
      permissionLevel: projectDefaults?.permission_level || undefined,
      // Attach project defaults so the chat-view can read thinking_effort
      projectDefaults: projectDefaults || null,
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

export async function sendMessage(taskId, message, thinkingBudget, images) {
  const tasks = { ...agentStore.getState('tasks') };
  const oldTask = tasks[taskId];
  if (!oldTask) return;

  // Create a new task object to ensure the store detects the change
  const task = { ...oldTask };
  tasks[taskId] = task;

  // Auto-title from first user message (first 60 chars, stripped of newlines)
  // Check for prior user text messages rather than empty messages array,
  // since non-user messages like model_switch markers may already exist
  const hasUserMessage = task.messages.some(m => m.role === 'user' && m.content?.some(c => c.type === 'text'));
  if (!hasUserMessage) {
    const autoTitle = message.replace(/\s+/g, ' ').trim().slice(0, 60);
    if (autoTitle) {
      task.title = autoTitle;
      if (task.info) task.info = { ...task.info, title: autoTitle };
      api.renameTask(taskId, autoTitle).catch(() => {});
    }
  }

  // Build local user message content (text + images for display)
  const userContent = [{ type: 'text', text: message }];
  if (images?.length) {
    for (const img of images) {
      userContent.push({ type: 'image', media_type: img.media_type, data: img.data });
    }
  }

  // Add user message locally
  task.messages = [...task.messages, { role: 'user', content: userContent }];
  task.isStreaming = true;
  task.status = 'Running';
  // Add placeholder for assistant response
  task.messages = [...task.messages, { role: 'assistant', content: [{ type: 'text', text: '' }] }];
  agentStore.setState({ tasks });

  try {
    await api.sendMessage(taskId, message, thinkingBudget, images);
  } catch (e) {
    console.error('Failed to send message:', e);
    task.isStreaming = false;
    task.status = 'Failed';

    // Surface the error in the chat so the user can see why it failed
    // (especially useful when the task's provider has been deleted).
    const errText = typeof e === 'string' ? e : (e?.message || String(e));
    task.messages = [
      ...task.messages,
      {
        role: 'assistant',
        content: [{ type: 'text', text: `⚠️ ${errText}` }],
      },
    ];

    agentStore.setState({ tasks: { ...tasks } });
  }
}

function appendStreamText(taskId, text) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

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

function appendThinkingDelta(taskId, text) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') {
      const content = [...msgs[i].content];
      const lastBlock = content[content.length - 1];
      if (lastBlock && lastBlock.type === 'thinking') {
        content[content.length - 1] = { ...lastBlock, thinking: lastBlock.thinking + text };
      } else {
        content.push({ type: 'thinking', thinking: text });
      }
      msgs[i] = { ...msgs[i], content };
      break;
    }
  }

  task.messages = msgs;
  agentStore.setState({ tasks: { ...tasks } });
}

function stampThinkingDuration(taskId, durationSecs) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  // Find the last thinking block without a duration in the last assistant message
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') {
      const content = [...msgs[i].content];
      for (let j = content.length - 1; j >= 0; j--) {
        if (content[j].type === 'thinking' && !content[j].duration_secs) {
          content[j] = { ...content[j], duration_secs: durationSecs };
          msgs[i] = { ...msgs[i], content };
          task.messages = msgs;
          agentStore.setState({ tasks: { ...tasks } });
          return;
        }
      }
      break;
    }
  }
}

function appendToolUse(taskId, toolUseId, toolName, toolInput) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const msgs = [...task.messages];
  // Add tool use to the last assistant message
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].role === 'assistant') {
      msgs[i] = {
        ...msgs[i],
        content: [...msgs[i].content, { type: 'tool_use', id: toolUseId, name: toolName, input: toolInput }],
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

  // When the backend resumes (Running again), clear any pending question
  if (status === 'Running') {
    task.pendingQuestion = null;
    const questions = { ...agentStore.getState('pendingQuestions') };
    delete questions[taskId];
    agentStore.setState({ tasks: { ...tasks }, pendingQuestions: questions });
    return;
  }

  // Clear turn budget warning when task stops running
  const warnings = { ...agentStore.getState('turnBudgetWarnings') };
  delete warnings[taskId];
  agentStore.setState({ tasks: { ...tasks }, turnBudgetWarnings: warnings });
}

function handleQuestionRequest(taskId, requestId, question) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  const questions = { ...agentStore.getState('pendingQuestions') };
  questions[taskId] = { request_id: requestId, question };

  tasks[taskId] = {
    ...task,
    status: 'WaitingForInput',
    isStreaming: false,
    pendingQuestion: { request_id: requestId, question },
  };
  agentStore.setState({ tasks, pendingQuestions: questions });
}

export async function respondToAgentQuestion(taskId, requestId, answer) {
  // Optimistically set task back to running
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    tasks[taskId] = {
      ...task,
      status: 'Running',
      isStreaming: true,
      pendingQuestion: null,
    };
    const questions = { ...agentStore.getState('pendingQuestions') };
    delete questions[taskId];
    agentStore.setState({ tasks, pendingQuestions: questions });
  }

  try {
    await api.respondToQuestion(taskId, requestId, answer);
  } catch (e) {
    console.error('Failed to respond to question:', e);
  }
}

function appendTaskComplete(taskId, diff) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;

  // Always stop streaming — this is the primary purpose of the call.
  task.isStreaming = false;
  task.status = 'Completed';

  // Guard: don't append a second task_complete message if the outer task and
  // the inner event-processor both fire agent-task-complete (rare but possible).
  const alreadyComplete = task.messages.some((m) => m.role === 'task_complete');
  if (!alreadyComplete) {
    task.messages = [
      ...task.messages,
      {
        role: 'task_complete',
        content: [{ type: 'task_complete', diff }],
      },
    ];
  }

  agentStore.setState({ tasks: { ...tasks } });
}

function updateTaskCost(taskId, cost) {
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (!task) return;
  task.cost = cost;
  agentStore.setState({ tasks: { ...tasks } });
}

function addPermissionRequest(payload) {
  const { task_id, request_id, operation, description, preview } = payload;
  const requests = { ...agentStore.getState('permissionRequests') };
  const taskRequests = [...(requests[task_id] || [])];
  taskRequests.push({ request_id, operation, description, preview });
  requests[task_id] = taskRequests;
  agentStore.setState({ permissionRequests: requests });
}

export function removePermissionRequest(taskId, requestId) {
  const requests = { ...agentStore.getState('permissionRequests') };
  requests[taskId] = (requests[taskId] || []).filter((r) => r.request_id !== requestId);
  agentStore.setState({ permissionRequests: requests });
}

export async function respondToPermission(taskId, requestId, approved) {
  removePermissionRequest(taskId, requestId);
  try {
    await api.respondToPermission(taskId, requestId, approved);
  } catch (e) {
    console.error('Failed to respond to permission:', e);
  }
}

export function setActiveTask(taskId) {
  agentStore.setState({ activeTaskId: taskId });
  uiStore.setState({ secondarySidebarVisible: true });
  // Load history from DB if messages are empty (e.g. clicking a past task)
  loadTaskHistory(taskId);
}

/**
 * Fetch persisted messages and cost from the backend and hydrate the task.
 * Only fetches if the task currently has no messages loaded.
 */
export async function loadTaskHistory(taskId) {
  if (!taskId) return;
  const tasks = agentStore.getState('tasks');
  const task = tasks[taskId];
  if (!task) return;
  if (task.messages && task.messages.length > 0) return; // already loaded

  try {
    // Load messages and cost in parallel
    const [messages, cost] = await Promise.all([
      api.getTaskMessages(taskId).catch(() => []),
      api.getTaskCost(taskId).catch(() => null),
    ]);
    const updated = { ...agentStore.getState('tasks') };
    if (updated[taskId]) {
      const patch = { ...updated[taskId] };
      if (messages && messages.length > 0) patch.messages = messages;
      if (cost) patch.cost = cost;
      updated[taskId] = patch;
      agentStore.setState({ tasks: updated });
    }
  } catch (e) {
    console.error('Failed to load task history:', e);
  }
}

/**
 * Change permission mode for a task.
 * Returns true if the change was applied, false on error.
 */
export async function setTaskPermissions(taskId, level) {
  try {
    await api.setTaskPermissions(taskId, level);
    // When leaving FullAuto, reset sensitive access to off
    if (level !== 'FullAuto') {
      await api.setTaskSensitiveAccess(taskId, false);
    }
  } catch (e) {
    console.error('Failed to set permissions:', e);
    return false;
  }

  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    task.permissionLevel = level;
    if (level !== 'FullAuto') task.sensitiveAccess = false;
    agentStore.setState({ tasks: { ...tasks } });
  }
  return true;
}

/**
 * Toggle sensitive file access for a FullAuto task.
 */
export async function setTaskSensitiveAccess(taskId, allowed) {
  try {
    await api.setTaskSensitiveAccess(taskId, allowed);
  } catch (e) {
    console.error('Failed to set sensitive access:', e);
    return false;
  }
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    task.sensitiveAccess = allowed;
    agentStore.setState({ tasks: { ...tasks } });
  }
  return true;
}

/**
 * Truncate the chat history for a task back to before `messageIndex`,
 * optionally reverting file changes to the snapshot taken at that message.
 *
 * After this call the in-memory messages array ends at `messageIndex - 1`
 * so the user can re-type and re-send the original message.
 */
export async function retryFromCheckpoint(taskId, messageIndex, checkpointId, revertFiles) {
  // 1. Optionally revert files to the snapshot
  if (revertFiles && checkpointId) {
    try {
      await api.revertToCheckpoint(checkpointId);
    } catch (e) {
      console.error('Failed to revert checkpoint files:', e);
    }
  }

  // 2. Truncate DB messages from this index onwards
  try {
    await api.truncateTaskMessages(taskId, messageIndex);
  } catch (e) {
    console.error('Failed to truncate task messages:', e);
  }

  // 3. Truncate in-memory messages and reset streaming state
  const tasks = { ...agentStore.getState('tasks') };
  const task = tasks[taskId];
  if (task) {
    tasks[taskId] = {
      ...task,
      messages: task.messages.slice(0, messageIndex),
      isStreaming: false,
    };
    agentStore.setState({ tasks });
  }
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

const FILE_MUTATING_TOOLS = new Set([
  'create_file',
  'edit_file',
  'apply_patch',
]);

// Tools that may modify files but we can't determine which path was affected
const BROAD_MUTATING_TOOLS = new Set([
  'run_command',
]);

function _getTaskProjectRoot(taskId) {
  const tasks = agentStore.getState('tasks');
  const task = tasks[taskId];
  if (!task) return null;
  const projectId = task.project_id || task.projectId;
  if (!projectId) return null;
  const projects = workspaceStore.getState('projects');
  const project = projects.find((p) => String(p.id) === String(projectId));
  return project ? project.root_path : null;
}

function _maybeRefreshFileTree(taskId, toolUseId) {
  const tasks = agentStore.getState('tasks');
  const task = tasks[taskId];
  if (!task) return;

  for (let i = task.messages.length - 1; i >= 0; i--) {
    const msg = task.messages[i];
    if (msg.role !== 'assistant') continue;
    for (const block of msg.content || []) {
      if (block.type === 'tool_use' && block.id === toolUseId) {
        const root = _getTaskProjectRoot(taskId);
        if (!root) return;

        if (BROAD_MUTATING_TOOLS.has(block.name)) {
          // run_command etc. — can't know which files changed, do a full refresh
          refreshProject(root);
          return;
        }

        if (!FILE_MUTATING_TOOLS.has(block.name)) return;

        const relPath = block.input?.path;
        if (relPath) {
          const sep = root.includes('/') ? '/' : '\\';
          const absPath = root.replace(/[\\/]+$/, '') + sep + relPath.replace(/^[\\/]+/, '');
          refreshAffectedDirectory(absPath);
        } else {
          refreshProject(root);
        }
        return;
      }
    }
    break;
  }
}

function _refreshProjectForTask(taskId) {
  const root = _getTaskProjectRoot(taskId);
  if (root) refreshProject(root);
}
