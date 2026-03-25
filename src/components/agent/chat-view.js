import { el, icon } from '../../utils/dom.js';
import { agentStore, sendMessage } from '../../state/agent.js';
import * as api from '../../lib/tauri-api.js';

export function createChatView() {
  const container = el('div', { class: 'chat-view' });

  // Messages area
  const messagesArea = el('div', { class: 'chat-messages' });

  // Input area
  const inputArea = el('div', { class: 'chat-input-area' });
  const textarea = el('textarea', {
    class: 'chat-input',
    placeholder: 'Send a message...',
    rows: '2',
  });
  const sendBtn = el('button', { class: 'chat-send-btn', title: 'Send' });
  sendBtn.appendChild(icon('M22 2L11 13M22 2l-7 20-4-9-9-4z', 16));

  sendBtn.addEventListener('click', () => {
    const taskId = agentStore.getState('activeTaskId');
    const text = textarea.value.trim();
    if (taskId && text) {
      sendMessage(taskId, text);
      textarea.value = '';
    }
  });

  textarea.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendBtn.click();
    }
  });

  inputArea.appendChild(textarea);
  inputArea.appendChild(sendBtn);

  container.appendChild(messagesArea);
  container.appendChild(inputArea);

  // Track loaded checkpoints
  let checkpoints = [];

  async function loadCheckpoints(taskId) {
    try {
      checkpoints = (await api.listCheckpoints(taskId)) || [];
    } catch {
      checkpoints = [];
    }
  }

  function hasFileChanges(msg) {
    if (!msg.content) return false;
    return msg.content.some(
      (b) => b.type === 'tool_use' && (b.name === 'write_file' || b.name === 'create_file')
    );
  }

  function findCheckpointForMessage(msgIndex) {
    // Find the checkpoint whose message_index is <= msgIndex
    // Checkpoints are created at user message time, so find the closest one at or before this index
    for (let i = checkpoints.length - 1; i >= 0; i--) {
      if (checkpoints[i].message_index <= msgIndex) {
        return checkpoints[i];
      }
    }
    return null;
  }

  async function handleRevert(checkpoint) {
    // Preview first
    let changes;
    try {
      changes = await api.previewCheckpoint(checkpoint.id);
    } catch (e) {
      console.error('Failed to preview checkpoint:', e);
      return;
    }

    if (!changes || changes.length === 0) return;

    // Build confirmation message
    const fileList = changes
      .map((c) => `${c.change_type === 'delete' ? 'Delete' : 'Restore'}: ${c.file_path}`)
      .join('\n');
    const confirmed = window.confirm(
      `Revert to checkpoint? The following changes will be made:\n\n${fileList}`
    );

    if (!confirmed) return;

    try {
      await api.revertToCheckpoint(checkpoint.id);
    } catch (e) {
      console.error('Failed to revert:', e);
    }
  }

  function render() {
    messagesArea.innerHTML = '';
    const taskId = agentStore.getState('activeTaskId');
    if (!taskId) {
      messagesArea.appendChild(el('div', { class: 'chat-empty' }, 'Select a task to start chatting'));
      return;
    }

    const tasks = agentStore.getState('tasks');
    const task = tasks[taskId];
    if (!task) return;

    // Load checkpoints asynchronously
    loadCheckpoints(taskId).then(() => renderMessages(task));
  }

  function renderMessages(task) {
    messagesArea.innerHTML = '';

    for (let i = 0; i < task.messages.length; i++) {
      const msg = task.messages[i];
      const msgEl = el('div', { class: `chat-message chat-message--${msg.role}` });

      // Role label
      const roleLabel = el('div', { class: 'chat-message__role' },
        msg.role === 'user' ? 'You' : msg.role === 'assistant' ? 'Assistant' : 'Tool'
      );
      msgEl.appendChild(roleLabel);

      // Content blocks
      for (const block of msg.content) {
        if (block.type === 'text' && block.text) {
          const textEl = el('div', { class: 'chat-message__text' });
          textEl.innerHTML = formatText(block.text);
          msgEl.appendChild(textEl);
        } else if (block.type === 'tool_use') {
          const toolEl = el('div', { class: 'chat-tool-use' });
          toolEl.appendChild(el('div', { class: 'chat-tool-use__name' }, `Tool: ${block.name}`));
          const inputPre = el('pre', { class: 'chat-tool-use__input' },
            JSON.stringify(block.input, null, 2)
          );
          toolEl.appendChild(inputPre);
          msgEl.appendChild(toolEl);
        } else if (block.type === 'tool_result') {
          const resultEl = el('div', {
            class: `chat-tool-result ${block.is_error ? 'chat-tool-result--error' : ''}`,
          });
          resultEl.appendChild(el('pre', { class: 'chat-tool-result__content' }, block.content));
          msgEl.appendChild(resultEl);
        }
      }

      // Checkpoint marker for assistant messages with file changes
      if (msg.role === 'assistant' && hasFileChanges(msg)) {
        const cp = findCheckpointForMessage(i);
        if (cp && cp.file_count > 0) {
          const cpMarker = el('div', { class: 'chat-checkpoint' });

          const cpInfo = el('div', { class: 'chat-checkpoint__info' });
          cpInfo.appendChild(icon('M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z', 14));
          cpInfo.appendChild(el('span', {}, `Checkpoint (${cp.file_count} file${cp.file_count !== 1 ? 's' : ''})`));
          cpMarker.appendChild(cpInfo);

          const revertBtn = el('button', { class: 'chat-checkpoint__revert', title: 'Revert to this checkpoint' }, 'Revert');
          revertBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            handleRevert(cp);
          });
          cpMarker.appendChild(revertBtn);

          msgEl.appendChild(cpMarker);
        }
      }

      messagesArea.appendChild(msgEl);
    }

    // Auto-scroll to bottom
    messagesArea.scrollTop = messagesArea.scrollHeight;
  }

  agentStore.subscribe('tasks', render);
  agentStore.subscribe('activeTaskId', render);
  render();

  return container;
}

function formatText(text) {
  // Escape HTML
  let html = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');

  // Code blocks
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, '<pre class="chat-code-block"><code>$2</code></pre>');

  // Inline code
  html = html.replace(/`([^`]+)`/g, '<code class="chat-inline-code">$1</code>');

  // Bold
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');

  // Newlines
  html = html.replace(/\n/g, '<br>');

  return html;
}
