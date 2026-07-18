import { save as saveFileDialog } from '@tauri-apps/plugin-dialog';
import { writeTextFileScoped } from '@/lib/fs-io';

/** Joins a project-relative path onto the project root using the root's native separator. */
function toAbsolutePath(projectRoot, relative) {
  if (!relative) return null;
  if (/^([a-zA-Z]:[\\/]|\\\\|\/)/.test(relative)) return relative;
  if (!projectRoot) return relative;
  const sep = projectRoot.includes('\\') && !projectRoot.includes('/') ? '\\' : '/';
  const root = projectRoot.replace(/[\\/]+$/, '');
  const tail = relative.replace(/^[\\/]+/, '');
  return `${root}${sep}${sep === '\\' ? tail.replace(/\//g, '\\') : tail}`;
}

/** Rewrites relative .rustic/ image references inside message text to absolute paths. */
function absolutizeTextRefs(text, projectRoot) {
  if (typeof text !== 'string' || !projectRoot) return text;
  return text.replace(/(^|\n)- (\.rustic\/[^\n]+)/g, (_m, lead, rel) => {
    return `${lead}- ${toAbsolutePath(projectRoot, rel)}`;
  });
}

/** Converts one chat message into a JSON-safe export record with absolute image paths. */
function exportMessage(msg, projectRoot) {
  const images = (msg.attachments || []).map((a) => ({
    name: a.name || null,
    media_type: a.mediaType || null,
    path: a.path || toAbsolutePath(projectRoot, a.relativePath) || null,
  }));
  const content = (Array.isArray(msg.content) ? msg.content : [])
    .map((b) => {
      if (!b || typeof b !== 'object') return b;
      if (b.type === 'text') {
        return { type: 'text', text: absolutizeTextRefs(b.text, projectRoot) };
      }
      if (b.type === 'image') {
        return {
          type: 'image',
          media_type: b.media_type || b.mediaType || null,
          name: b.name || null,
        };
      }
      if (b.type === 'thinking') {
        return { type: 'thinking', text: b.text ?? b.thinking ?? '', duration_secs: b.durationSecs ?? b.duration_secs ?? 0 };
      }
      if (b.type === 'tool_use') {
        return { type: 'tool_use', id: b.id, name: b.name, input: b.input };
      }
      if (b.type === 'tool_result') {
        return { type: 'tool_result', tool_use_id: b.tool_use_id, output: b.output ?? b.content ?? '', is_error: !!b.is_error };
      }
      return b;
    });
  return {
    role: msg.role,
    content,
    ...(images.length > 0 ? { images } : {}),
    ...(msg.turnUsage ? { turn_usage: msg.turnUsage } : {}),
  };
}

/** Exports a chat transcript as JSON via a save-file dialog; returns the saved path or null when cancelled. */
export async function exportChatAsJson({ taskId, taskTitle, messages, project, cost }) {
  const safeTitle = (taskTitle || 'chat')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60) || 'chat';
  const target = await saveFileDialog({
    title: 'Export chat history',
    defaultPath: `${safeTitle}.json`,
    filters: [{ name: 'JSON', extensions: ['json'] }],
  });
  if (!target) return null;

  const payload = {
    exported_at: new Date().toISOString(),
    task: { id: taskId || null, title: taskTitle || null },
    project: project ? { id: project.id, name: project.name, root: project.root } : null,
    ...(cost ? { cost } : {}),
    messages: (messages || []).map((m) => exportMessage(m, project?.root)),
  };
  await writeTextFileScoped(target, JSON.stringify(payload, null, 2));
  return target;
}
