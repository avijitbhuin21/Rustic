// Per-tool metadata (label, icon path, color category) and helpers for
// formatting a tool's input arguments + output content for display in the
// chat. Pulled out of chat-view.js so the renderer can grow without
// dragging this 130-line table along.

// Native (Rustic-emitted) tool names use snake_case; Claude Code's CLI emits
// PascalCase. We register both in the same table so the renderer doesn't
// have to know which provider produced the call. The Claude Code entries
// reuse the existing icons / colors so a `Used Edit` card looks visually
// identical to a `Used edit_file` card.
export const TOOL_META = {
  read_file:      { label: 'Read file',      iconPath: 'M15 12a3 3 0 11-6 0 3 3 0 016 0zM2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z', color: 'blue' },
  list_directory: { label: 'List directory', iconPath: 'M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z', color: 'blue' },
  grep_search:    { label: 'Search',         iconPath: 'M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z', color: 'blue' },
  run_command:    { label: 'Run command',    iconPath: 'M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z', color: 'orange' },
  edit_file:      { label: 'Edit file',      iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  apply_patch:    { label: 'Edit file',      iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  write_file:     { label: 'Write file',     iconPath: 'M12 5v14M5 12h14', color: 'green' },
  create_file:    { label: 'Create file',    iconPath: 'M12 5v14M5 12h14', color: 'green' },
  chat_message:   { label: 'Message',        iconPath: 'M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z', color: 'purple', special: 'chat_message' },
  spawn_subagent: { label: 'Subagent',       iconPath: 'M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2M9 11a4 4 0 100-8 4 4 0 000 8zM23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75', color: 'purple' },
  wait_for_subagents: { label: 'Wait for subagents', iconPath: 'M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z', color: 'gray' },
  list_active_agents: { label: 'List agents', iconPath: 'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2', color: 'gray' },
  web_search:     { label: 'Web search',     iconPath: 'M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z', color: 'teal' },
  web_fetch:      { label: 'Web fetch',      iconPath: 'M12 2a10 10 0 100 20 10 10 0 000-20zM2 12h20M12 2a15.3 15.3 0 010 20M12 2a15.3 15.3 0 000 20', color: 'teal' },

  // Claude Code (subscription harness) tool names. Reuse existing icons.
  Read:        { label: 'Read file',        iconPath: 'M15 12a3 3 0 11-6 0 3 3 0 016 0zM2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z', color: 'blue' },
  Glob:        { label: 'Glob',              iconPath: 'M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z', color: 'blue' },
  Grep:        { label: 'Search',            iconPath: 'M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z', color: 'blue' },
  Bash:        { label: 'Run command',       iconPath: 'M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z', color: 'orange' },
  BashOutput:  { label: 'Bash output',       iconPath: 'M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z', color: 'orange' },
  KillShell:   { label: 'Kill shell',        iconPath: 'M18 6L6 18M6 6l12 12', color: 'orange' },
  Edit:        { label: 'Edit file',         iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  MultiEdit:   { label: 'Edit file',         iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  Write:       { label: 'Write file',        iconPath: 'M12 5v14M5 12h14', color: 'green' },
  NotebookEdit:{ label: 'Edit notebook',     iconPath: 'M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z', color: 'yellow' },
  TodoWrite:   { label: 'Update todos',      iconPath: 'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2', color: 'gray' },
  Task:        { label: 'Subagent',          iconPath: 'M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2M9 11a4 4 0 100-8 4 4 0 000 8zM23 21v-2a4 4 0 00-3-3.87M16 3.13a4 4 0 010 7.75', color: 'purple' },
  WebFetch:    { label: 'Web fetch',         iconPath: 'M12 2a10 10 0 100 20 10 10 0 000-20zM2 12h20M12 2a15.3 15.3 0 010 20M12 2a15.3 15.3 0 000 20', color: 'teal' },
  WebSearch:   { label: 'Web search',        iconPath: 'M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z', color: 'teal' },
  ExitPlanMode:{ label: 'Exit plan mode',    iconPath: 'M5 13l4 4L19 7', color: 'green' },
  AskUserQuestion: { label: 'Ask user',      iconPath: 'M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z', color: 'purple' },
};

/// Tools whose input is best rendered as a unified diff. Used to drive the
/// special-case in `formatToolInput` and the editor language hint in
/// `chat-view.js` (`'diff'` rather than `'json'`).
export const DIFF_TOOL_NAMES = new Set(['Edit', 'MultiEdit', 'Write']);

export const TOOL_META_DEFAULT = { label: null, iconPath: 'M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z', color: 'gray' };

export function getToolSummary(name, input) {
  const path = input.path || input.file_path || input.directory || '';
  switch (name) {
    case 'read_file':
    case 'Read': {
      let s = path;
      if (input.start_line && input.end_line) s += `:${input.start_line}-${input.end_line}`;
      else if (input.start_line) s += `:${input.start_line}+`;
      // Claude Code uses `offset`+`limit`.
      if (input.offset != null && input.limit != null) s += `:${input.offset}+${input.limit}`;
      return s;
    }
    case 'list_directory': return path;
    case 'grep_search':
    case 'Grep': {
      const pat = input.pattern || input.query || '';
      return pat ? `"${pat}"${path ? '  ' + path : ''}` : path;
    }
    case 'Glob': {
      const pat = input.pattern || '';
      return pat || path;
    }
    case 'run_command':
    case 'Bash': {
      const cmd = input.command || input.cmd || '';
      return cmd.length > 72 ? cmd.slice(0, 69) + '…' : cmd;
    }
    case 'edit_file': case 'apply_patch': case 'write_file': case 'create_file':
    case 'Edit': case 'MultiEdit': case 'Write': case 'NotebookEdit':
      return path;
    case 'web_search':
    case 'WebSearch': {
      const q = (input.query || '').trim();
      return q ? `"${q.length > 72 ? q.slice(0, 69) + '…' : q}"` : '';
    }
    case 'web_fetch':
    case 'WebFetch': {
      const url = (input.url || '').trim();
      return url.length > 72 ? url.slice(0, 69) + '…' : url;
    }
    case 'TodoWrite': {
      const todos = Array.isArray(input.todos) ? input.todos : [];
      return todos.length ? `${todos.length} todo${todos.length === 1 ? '' : 's'}` : '';
    }
    case 'Task': {
      const desc = input.description || input.subagent_type || '';
      return desc.length > 72 ? desc.slice(0, 69) + '…' : desc;
    }
    default: return '';
  }
}

/// Parse the content string of a ToolResult and produce a human-readable
/// form for display. Most tools already return plain text — those pass
/// through unchanged. Anthropic's server-side web_search / web_fetch
/// results are JSON blobs (full of `encrypted_content` etc.), so we
/// extract only the fields worth showing to the user.
export function formatToolOutput(name, rawContent) {
  if (rawContent == null) return '';
  const content = String(rawContent);
  if (name !== 'web_search' && name !== 'web_fetch') return content;

  let parsed;
  try { parsed = JSON.parse(content); } catch { return content; }

  if (name === 'web_search') {
    // Two shapes we handle:
    //   1. Anthropic server-side: array of {type:"web_search_result", title, url, page_age?, encrypted_content}
    //   2. Gemini grounding (synthesized by our provider): {queries, results:[{title,url}]}
    //   3. Client-side Tavily/Brave: already plain text, caught by the !== check above
    if (Array.isArray(parsed)) {
      return parsed
        .map((r, i) => {
          const title = r.title || '(untitled)';
          const url = r.url || '';
          const age = r.page_age ? ` — ${r.page_age}` : '';
          return `${i + 1}. ${title}${age}\n   ${url}`;
        })
        .join('\n\n');
    }
    if (parsed && Array.isArray(parsed.results)) {
      const queries = Array.isArray(parsed.queries) && parsed.queries.length
        ? `Queries: ${parsed.queries.join(' | ')}\n\n`
        : '';
      return queries + parsed.results
        .map((r, i) => `${i + 1}. ${r.title || '(untitled)'}\n   ${r.url || ''}`)
        .join('\n\n');
    }
    return content;
  }

  // web_fetch (Anthropic): {type:"web_fetch_result", url, retrieved_at, content:{type:"document", source:{type:"text", data:"..."}}}
  if (name === 'web_fetch') {
    const url = parsed.url || '';
    const retrieved = parsed.retrieved_at ? ` (retrieved ${parsed.retrieved_at})` : '';
    const body = parsed.content?.source?.data
      || parsed.content?.text
      || (typeof parsed.content === 'string' ? parsed.content : '');
    return `${url}${retrieved}\n\n${body}`.trim();
  }

  return content;
}

export function formatToolInput(name, input) {
  // ── Diff-style rendering for Claude Code's edit-shaped tools ─────────
  // `Edit`     — { file_path, old_string, new_string, replace_all? }
  // `MultiEdit`— { file_path, edits: [{old_string, new_string}, ...] }
  // `Write`    — { file_path, content }
  //
  // The default JSON dump turns a 50-line edit into an unreadable single
  // string with `\n` escapes. A unified-diff `+`/`-` rendering makes the
  // change inspectable from the chat without opening the file.
  if (DIFF_TOOL_NAMES.has(name)) {
    return formatEditAsDiff(name, input);
  }

  const entries = Object.entries(input);
  if (entries.length === 0) return '(no input)';

  // For file ops: show metadata fields first, then large content fields separately
  const bulkKeys = ['content', 'new_content', 'diff', 'patch'];
  const meta = entries.filter(([k]) => !bulkKeys.includes(k));
  const bulk = entries.filter(([k]) => bulkKeys.includes(k));

  if (meta.length === 0 && bulk.length === 0) return JSON.stringify(input, null, 2);

  let out = '';
  for (const [k, v] of meta) {
    out += `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}\n`;
  }
  for (const [k, v] of bulk) {
    const str = typeof v === 'string' ? v : JSON.stringify(v, null, 2);
    const lines = str.split('\n');
    const preview = lines.length > 30 ? lines.slice(0, 30).join('\n') + '\n…' : str;
    out += `\n${k}:\n${preview}\n`;
  }
  return out.trim();
}

/// Render an Edit/MultiEdit/Write tool input as a git-style unified diff so
/// the user can read what's changing. Output is plain text (no ANSI / HTML)
/// so it slots into the existing `<pre>` preview and the scratch editor's
/// `'diff'` syntax mode.
function formatEditAsDiff(name, input) {
  const path = input.file_path || input.path || '';
  let header = path ? `--- a/${path}\n+++ b/${path}\n` : '';

  if (name === 'Write') {
    const content = typeof input.content === 'string' ? input.content : '';
    if (!content) return `${header}(empty file)`;
    const body = content
      .split('\n')
      .map((line) => `+${line}`)
      .join('\n');
    return `${header}${body}`;
  }

  if (name === 'Edit') {
    const old = typeof input.old_string === 'string' ? input.old_string : '';
    const fresh = typeof input.new_string === 'string' ? input.new_string : '';
    return `${header}${diffHunk(old, fresh)}`;
  }

  if (name === 'MultiEdit') {
    const edits = Array.isArray(input.edits) ? input.edits : [];
    if (edits.length === 0) return `${header}(no edits)`;
    const hunks = edits
      .map((e, i) => {
        const old = typeof e.old_string === 'string' ? e.old_string : '';
        const fresh = typeof e.new_string === 'string' ? e.new_string : '';
        return `@@ edit ${i + 1} of ${edits.length} @@\n${diffHunk(old, fresh)}`;
      })
      .join('\n\n');
    return `${header}${hunks}`;
  }

  // Shouldn't reach: caller already gated on DIFF_TOOL_NAMES.
  return JSON.stringify(input, null, 2);
}

/// Produce a `-old / +new` block for a single edit. We don't run a real LCS
/// diff here — Claude Code's Edit tool already chose the smallest unique
/// match, so a "remove the whole old block / insert the whole new block"
/// rendering is honest and unambiguous. A finer LCS would just hide the
/// model's actual instruction.
function diffHunk(oldStr, newStr) {
  const oldLines = oldStr === '' ? [] : oldStr.split('\n').map((l) => `-${l}`);
  const newLines = newStr === '' ? [] : newStr.split('\n').map((l) => `+${l}`);
  if (oldLines.length === 0 && newLines.length === 0) return '(no change)';
  return [...oldLines, ...newLines].join('\n');
}
