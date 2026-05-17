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
  list_subagents:     { label: 'List sub-agents', iconPath: 'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2', color: 'gray' },
  // Legacy name from before the P1.6 rename — kept so saved transcripts
  // that still reference the old name render with a label and icon.
  list_active_agents: { label: 'List sub-agents', iconPath: 'M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2', color: 'gray' },
  send_message:       { label: 'Send message',    iconPath: 'M21 12a9 9 0 11-18 0 9 9 0 0118 0zM8 12h.01M12 12h.01M16 12h.01', color: 'teal' },
  nudge_subagent:     { label: 'Nudge sub-agent', iconPath: 'M13 10V3L4 14h7v7l9-11h-7z', color: 'amber' },
  stop_subagent:      { label: 'Stop sub-agent',  iconPath: 'M21 12a9 9 0 11-18 0 9 9 0 0118 0zm-9-4v4m0 4h.01', color: 'red' },
  web_search:     { label: 'Web search',     iconPath: 'M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z', color: 'teal' },
  web_fetch:      { label: 'Web fetch',      iconPath: 'M12 2a10 10 0 100 20 10 10 0 000-20zM2 12h20M12 2a15.3 15.3 0 010 20M12 2a15.3 15.3 0 000 20', color: 'teal' },
  image_create:   { label: 'Create image',   iconPath: 'M4 5a2 2 0 012-2h12a2 2 0 012 2v14a2 2 0 01-2 2H6a2 2 0 01-2-2V5zm4 7l2 2 4-4 4 6H6l2-4z', color: 'purple' },
  video_create:   { label: 'Create video',   iconPath: 'M15 10l4.553-2.276A1 1 0 0121 8.618v6.764a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z', color: 'purple' },
  animate:        { label: 'Animate image',  iconPath: 'M13 10V3L4 14h7v7l9-11h-7z', color: 'purple' },

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
///
/// Both Claude Code's edit-shaped tools (`Edit` / `MultiEdit` / `Write`) and
/// Codex's `apply_patch` (mapped onto `Edit` in `event_map_codex.rs` with
/// input shape `{ changes: [...] }`) ride this set. The chat card surfaces
/// the file path on the INPUT side and the full diff on the OUTPUT side —
/// see `formatEditPathForInput` / `formatEditDiffForOutput`.
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
    case 'image_create': {
      const p = (input.prompt || '').trim();
      const srcs = Array.isArray(input.image_paths)
        ? input.image_paths.map((x) => String(x || '').trim()).filter(Boolean)
        : [];
      const src = srcs.length > 1 ? `${srcs[0]} (+${srcs.length - 1})` : srcs[0] || '';
      const s = src ? `edit ${src} — ${p}` : p;
      return s.length > 72 ? s.slice(0, 69) + '…' : s;
    }
    case 'video_create': {
      const p = (input.prompt || '').trim();
      return p.length > 72 ? p.slice(0, 69) + '…' : p;
    }
    case 'animate': {
      const img = (input.image_path || '').trim();
      const p = (input.prompt || '').trim();
      const s = img ? `${img} — ${p}` : p;
      return s.length > 72 ? s.slice(0, 69) + '…' : s;
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
  // Edit-shaped tools: show only the file path on the INPUT card (diff goes on OUTPUT).
  if (DIFF_TOOL_NAMES.has(name)) {
    return formatEditPathForInput(name, input);
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

/// INPUT-side rendering for edit-shaped tools. Returns the file path(s)
/// being touched — nothing else — so the card mirrors the user's mental
/// model: "INPUT = which file" / "OUTPUT = what changed".
///
/// Three input shapes feed into this:
///   - Claude Code `Edit` / `MultiEdit` / `Write`: `{ file_path, ... }`
///   - Codex `apply_patch` (relabeled `Edit`): `{ changes: [{ path, ... }] }`
///   - Anything else with a `path` / `file_path`: fall back to that field
export function formatEditPathForInput(_name, input) {
  if (input && Array.isArray(input.changes) && input.changes.length > 0) {
    const paths = input.changes
      .map((c) => (typeof c.path === 'string' ? c.path : ''))
      .filter(Boolean);
    if (paths.length === 1) return paths[0];
    if (paths.length > 1) return paths.join('\n');
  }
  const path =
    (typeof input?.file_path === 'string' && input.file_path) ||
    (typeof input?.path === 'string' && input.path) ||
    '';
  return path || '(no path)';
}

/// OUTPUT-side rendering for edit-shaped tools. Synthesises the diff from
/// the tool *input* (it's where the model declared the change) rather than
/// from the harness's tool-result content — Codex returns a one-line
/// "1 file(s) changed" summary that hides the actual edit, and Claude Code
/// returns a small snippet that doesn't carry the full hunk.
///
/// Falls back to `formatToolOutput(name, rawContent)` when the input shape
/// doesn't match an edit (e.g. a tool we register as diff-shaped but get
/// non-edit fields for) so we never lose the harness's response.
export function formatEditDiffForOutput(name, input, rawContent) {
  if (input && Array.isArray(input.changes) && input.changes.length > 0) {
    return formatCodexFileChanges(input.changes);
  }
  if (input && (typeof input.file_path === 'string' || typeof input.path === 'string')) {
    return formatEditAsDiff(name, input);
  }
  return formatToolOutput(name, rawContent);
}

/// Render Codex's `FileUpdateChange[]` (from a `fileChange` item) as a
/// concatenated unified diff. Each change ships its own `diff` string —
/// "add" changes have an empty diff in some Codex versions, so we synthesise
/// a `+`-prefixed body from the file content when present.
function formatCodexFileChanges(changes) {
  const blocks = [];
  for (const ch of changes) {
    const path = typeof ch.path === 'string' ? ch.path : '(unknown)';
    const kindType = ch?.kind?.type || 'update';
    const header = `--- a/${path}\n+++ b/${path} (${kindType})\n`;
    if (typeof ch.diff === 'string' && ch.diff.length > 0) {
      blocks.push(`${header}${ch.diff}`);
    } else if (typeof ch.content === 'string' && ch.content.length > 0) {
      const body = ch.content
        .split('\n')
        .map((line) => `+${line}`)
        .join('\n');
      blocks.push(`${header}${body}`);
    } else {
      blocks.push(`${header}(no diff body)`);
    }
  }
  return blocks.join('\n\n');
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
