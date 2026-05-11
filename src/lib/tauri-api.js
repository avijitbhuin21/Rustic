/**
 * Typed wrappers around Tauri invoke() calls.
 * Falls back gracefully when not running in Tauri.
 */

let invoke;
let listen;

async function getInvoke() {
  if (invoke) return invoke;
  try {
    const mod = await import('@tauri-apps/api/core');
    invoke = mod.invoke;
    return invoke;
  } catch {
    // Not in Tauri — return a mock
    invoke = async (cmd, args) => {
      console.log(`[mock invoke] ${cmd}`, args);
      return null;
    };
    return invoke;
  }
}

async function getListen() {
  if (listen) return listen;
  try {
    const mod = await import('@tauri-apps/api/event');
    listen = mod.listen;
    return listen;
  } catch {
    listen = async () => () => {};
    return listen;
  }
}

// Shell: open external URL in default browser
export async function openUrl(url) {
  try {
    const shell = await import('@tauri-apps/plugin-shell');
    await shell.open(url);
  } catch {
    // Fallback for non-Tauri or missing plugin
    window.open(url, '_blank');
  }
}

export async function addProject(path) {
  const inv = await getInvoke();
  return inv('add_project', { path });
}

export async function removeProject(projectId) {
  const inv = await getInvoke();
  return inv('remove_project', { projectId });
}

export async function listProjects() {
  const inv = await getInvoke();
  return inv('list_projects');
}

export async function readDir(path) {
  const inv = await getInvoke();
  return inv('read_dir', { path });
}

export async function listProjectFiles(rootPath, maxFiles) {
  const inv = await getInvoke();
  return inv('list_project_files', { rootPath, maxFiles: maxFiles ?? null });
}

export async function readFileContent(path) {
  const inv = await getInvoke();
  return inv('read_file_content', { path });
}

export async function createFile(dirPath, name) {
  const inv = await getInvoke();
  return inv('create_file', { dirPath, name });
}

export async function createFolder(dirPath, name) {
  const inv = await getInvoke();
  return inv('create_folder', { dirPath, name });
}

export async function renameEntry(oldPath, newName) {
  const inv = await getInvoke();
  return inv('rename_entry', { oldPath, newName });
}

export async function deleteEntry(path) {
  const inv = await getInvoke();
  return inv('delete_entry', { path });
}

/**
 * Recursively copy a file or directory into `dstDir`. If `newName` is null
 * the source's basename is used. The backend auto-renames on collision
 * (`foo.txt` → `foo (1).txt`) so paste never overwrites existing files.
 * Returns the final created path.
 */
export async function copyEntry(srcPath, dstDir, newName = null) {
  const inv = await getInvoke();
  return inv('copy_entry', { srcPath, dstDir, newName });
}

/**
 * Stat a path on disk. Returns `[name, isDir]` if the path exists, or `null`
 * otherwise. Used to validate paths read from the OS clipboard before
 * attempting to paste them.
 */
export async function statPath(path) {
  const inv = await getInvoke();
  return inv('stat_path', { path });
}

/**
 * Read absolute file paths from the OS clipboard. On Windows this catches
 * the CF_HDROP file list that Explorer puts on the clipboard when the user
 * presses Ctrl+C on a file (something the webview's `navigator.clipboard`
 * cannot see). Returns an array of absolute paths, possibly empty.
 */
export async function readClipboardFiles() {
  const inv = await getInvoke();
  return inv('read_clipboard_files');
}

/**
 * Write a list of absolute file paths to the OS clipboard as a "file list"
 * (CF_HDROP on Windows, NSFilenamesPboardType on macOS, text/uri-list on
 * Linux). After this runs, the user can paste actual file copies into any
 * other app — Windows Explorer, Outlook, Slack, Finder, etc. The `cut` flag
 * sets the "Preferred DropEffect" on Windows so Explorer knows to do a move
 * instead of a copy.
 */
export async function writeClipboardFiles(paths, cut = false) {
  const inv = await getInvoke();
  return inv('write_clipboard_files', { paths, cut });
}





export async function revealInFileManager(path) {
  const inv = await getInvoke();
  return inv('reveal_in_file_manager', { path });
}

// Editor commands
export async function openFile(path) {
  const inv = await getInvoke();
  return inv('open_file', { path });
}

export async function openScratchBuffer(title, content, language = null) {
  const inv = await getInvoke();
  return inv('open_scratch_buffer', { title, content, language });
}

export async function getVisibleLines(bufferId, start, end) {
  const inv = await getInvoke();
  return inv('get_visible_lines', { bufferId, start, end });
}

export async function highlightBuffer(bufferId) {
  const inv = await getInvoke();
  return inv('highlight_buffer', { bufferId });
}

export async function highlightRange(bufferId, startLine, endLine) {
  const inv = await getInvoke();
  return inv('highlight_range', { bufferId, startLine, endLine });
}

export async function editBuffer(bufferId, line, col, newText, deleteCount) {
  const inv = await getInvoke();
  return inv('edit_buffer', { bufferId, line, col, newText, deleteCount });
}

export async function formatBuffer(bufferId, indentSize = 4) {
  const inv = await getInvoke();
  return inv('format_buffer', { bufferId, indentSize });
}

export async function saveFile(bufferId, force = false) {
  const inv = await getInvoke();
  return inv('save_file', { bufferId, force });
}

export async function bufferExternalChange(bufferId) {
  const inv = await getInvoke();
  return inv('buffer_external_change', { bufferId });
}

export async function reloadBuffer(bufferId) {
  const inv = await getInvoke();
  return inv('reload_buffer', { bufferId });
}

export async function confirmQuit() {
  const inv = await getInvoke();
  return inv('confirm_quit');
}

// Returns the absolute path to the rotating-log directory. Used by future
// "Reveal logs folder" / opt-in crash-report flows.
export async function getLogsDir() {
  const inv = await getInvoke();
  return inv('get_logs_dir');
}

// List rotating log files on disk, newest first. Each entry is
// { path, name, date, size_bytes } — `date` is YYYY-MM-DD or null for the
// active (un-rotated) file.
export async function listLogFiles() {
  const inv = await getInvoke();
  return inv('list_log_files');
}

// Read one log file. The backend rejects any path outside the logs dir.
export async function readLogFile(path) {
  const inv = await getInvoke();
  return inv('read_log_file', { path });
}

// Set per-model capability flags. Pass `null` for any field to leave it
// alone; passing `null` for *every* field removes the override entirely
// (reverts the model to defaults of `true`). Persisted to the SQLite
// ai_config row, so future requests respect the flags.
export async function setModelCapabilities(
  modelId,
  { supportsTemperature = null, supportsReasoningEffort = null } = {},
) {
  const inv = await getInvoke();
  return inv('set_model_capabilities', {
    modelId,
    supportsTemperature,
    supportsReasoningEffort,
  });
}

// Read every per-model capability override. Returns a map of model_id →
// { supports_temperature: bool }.
export async function getModelCapabilities() {
  const inv = await getInvoke();
  return inv('get_model_capabilities');
}

/// Subscribe to a Tauri event. Returns an unsubscribe function.
export async function onEvent(name, handler) {
  const lst = await getListen();
  return lst(name, handler);
}

export async function undoEdit(bufferId) {
  const inv = await getInvoke();
  return inv('undo_edit', { bufferId });
}

export async function redoEdit(bufferId) {
  const inv = await getInvoke();
  return inv('redo_edit', { bufferId });
}

export async function closeBuffer(bufferId) {
  const inv = await getInvoke();
  return inv('close_buffer', { bufferId });
}

// Terminal commands
export async function createTerminal(cwd, label, isAgent = false, shellProgram = null) {
  const inv = await getInvoke();
  return inv('create_terminal', { cwd, label, isAgent, shellProgram });
}

export async function detectShells() {
  const inv = await getInvoke();
  return inv('detect_shells');
}

export async function writeTerminal(sessionId, data) {
  const inv = await getInvoke();
  return inv('write_terminal', { sessionId, data });
}

export async function resizeTerminal(sessionId, cols, rows) {
  const inv = await getInvoke();
  return inv('resize_terminal', { sessionId, cols, rows });
}

export async function closeTerminal(sessionId) {
  const inv = await getInvoke();
  return inv('close_terminal', { sessionId });
}

export async function listTerminals() {
  const inv = await getInvoke();
  return inv('list_terminals');
}

export async function onTerminalOutput(callback) {
  const l = await getListen();
  return l('terminal-output', (event) => callback(event.payload));
}

export async function onTerminalListChanged(callback) {
  const l = await getListen();
  return l('terminal-list-changed', () => callback());
}

// Search commands. The backend streams results — `startSearch` returns a
// numeric `search_id` immediately and pushes `search-event` Tauri events as
// each file is matched. Callers subscribe via `onSearchEvent` and filter by id.
// Bumping a new search implicitly cancels the previous one (the backend's
// active-id counter changes), but the explicit `cancelSearch` is exposed for
// the clear-results button.
export async function startSearch(scope, pattern, isRegex, caseSensitive, wholeWord, includeGlob, excludeGlob) {
  const inv = await getInvoke();
  return inv('start_search', { scope, pattern, isRegex, caseSensitive, wholeWord, includeGlob, excludeGlob });
}

export async function cancelSearch() {
  const inv = await getInvoke();
  return inv('cancel_search');
}

export async function onSearchEvent(callback) {
  const l = await getListen();
  return l('search-event', (event) => callback(event.payload));
}

export async function replaceInFile(path, pattern, replacement, isRegex, caseSensitive, wholeWord) {
  const inv = await getInvoke();
  return inv('replace_in_file', { path, pattern, replacement, isRegex, caseSensitive, wholeWord });
}

// Git commands
export async function gitStatus(projectId) {
  const inv = await getInvoke();
  return inv('git_status', { projectId });
}

export async function gitStage(projectId, paths) {
  const inv = await getInvoke();
  return inv('git_stage', { projectId, paths });
}

export async function gitUnstage(projectId, paths) {
  const inv = await getInvoke();
  return inv('git_unstage', { projectId, paths });
}

export async function gitCommit(projectId, message) {
  const inv = await getInvoke();
  return inv('git_commit', { projectId, message });
}

export async function gitDiscard(projectId, paths) {
  const inv = await getInvoke();
  return inv('git_discard', { projectId, paths });
}

export async function gitDiff(projectId, path) {
  const inv = await getInvoke();
  return inv('git_diff', { projectId, path });
}

export async function gitDiffStaged(projectId) {
  const inv = await getInvoke();
  return inv('git_diff_staged', { projectId });
}

export async function gitBranches(projectId) {
  const inv = await getInvoke();
  return inv('git_branches', { projectId });
}

export async function gitInit(projectId) {
  const inv = await getInvoke();
  return inv('git_init', { projectId });
}

export async function gitPush(projectId) {
  const inv = await getInvoke();
  return inv('git_push', { projectId });
}

export async function gitPull(projectId) {
  const inv = await getInvoke();
  return inv('git_pull', { projectId });
}

export async function gitFetch(projectId) {
  const inv = await getInvoke();
  return inv('git_fetch', { projectId });
}

export async function gitAheadBehind(projectId) {
  const inv = await getInvoke();
  return inv('git_ahead_behind', { projectId });
}

export async function gitCheckoutBranch(projectId, branch) {
  const inv = await getInvoke();
  return inv('git_checkout_branch', { projectId, branch });
}

export async function gitCreateBranch(projectId, branch, checkout) {
  const inv = await getInvoke();
  return inv('git_create_branch', { projectId, branch, checkout });
}

export async function gitRebase(projectId, ontoBranch) {
  const inv = await getInvoke();
  return inv('git_rebase', { projectId, ontoBranch });
}

export async function gitRebaseContinue(projectId) {
  const inv = await getInvoke();
  return inv('git_rebase_continue', { projectId });
}

export async function gitRebaseAbort(projectId) {
  const inv = await getInvoke();
  return inv('git_rebase_abort', { projectId });
}

export async function gitGetConflicts(projectId) {
  const inv = await getInvoke();
  return inv('git_get_conflicts', { projectId });
}

export async function gitResolveConflict(projectId, path, side) {
  const inv = await getInvoke();
  return inv('git_resolve_conflict', { projectId, path, side });
}

export async function gitMergeCommit(projectId) {
  const inv = await getInvoke();
  return inv('git_merge_commit', { projectId });
}

export async function gitSetToken(token) {
  const inv = await getInvoke();
  return inv('git_set_token', { token });
}

export async function gitGetToken() {
  const inv = await getInvoke();
  return inv('git_get_token');
}

export async function gitAddToGitignore(projectId, pattern) {
  const inv = await getInvoke();
  return inv('git_add_to_gitignore', { projectId, pattern });
}

export async function gitAddRemote(projectId, name, url) {
  const inv = await getInvoke();
  return inv('git_add_remote', { projectId, name, url });
}

export async function gitGetRemoteUrl(projectId) {
  const inv = await getInvoke();
  return inv('git_get_remote_url', { projectId });
}

export async function getDefaultProjectsDir() {
  const inv = await getInvoke();
  return inv('get_default_projects_dir');
}

export async function gitClone(url, targetDir) {
  const inv = await getInvoke();
  return inv('git_clone', { url, targetDir: targetDir || null });
}

// Git log / history
export async function gitLog(projectId, maxCount = 50) {
  const inv = await getInvoke();
  return inv('git_log', { projectId, maxCount });
}

export async function gitCommitFiles(projectId, oid) {
  const inv = await getInvoke();
  return inv('git_commit_files', { projectId, oid });
}

export async function gitCommitFileDiff(projectId, oid, path) {
  const inv = await getInvoke();
  return inv('git_commit_file_diff', { projectId, oid, path });
}

export async function gitUnpushedCommits(projectId, maxCount = 100) {
  const inv = await getInvoke();
  return inv('git_unpushed_commits', { projectId, maxCount });
}

export async function gitUndoLastCommit(projectId) {
  const inv = await getInvoke();
  return inv('git_undo_last_commit', { projectId });
}

// GitHub OAuth
export async function githubDeviceCode() {
  const inv = await getInvoke();
  return inv('github_device_code');
}

export async function githubPollToken(deviceCode) {
  const inv = await getInvoke();
  return inv('github_poll_token', { deviceCode });
}

export async function githubGetUser() {
  const inv = await getInvoke();
  return inv('github_get_user');
}

// Agent commands
export async function createTask(projectId, projectName, projectRoot, title) {
  const inv = await getInvoke();
  return inv('create_task', { projectId, projectName, projectRoot, title });
}

export async function sendMessage(taskId, message, thinkingBudget, images) {
  const inv = await getInvoke();
  return inv('send_message', {
    taskId,
    message,
    thinkingBudget: thinkingBudget ?? null,
    images: images?.length ? images : null,
  });
}

export async function listTasks(projectId) {
  const inv = await getInvoke();
  return inv('list_tasks', { projectId });
}

export async function getTaskMessages(taskId) {
  const inv = await getInvoke();
  return inv('get_task_messages', { taskId });
}

export async function getTaskTodos(taskId) {
  const inv = await getInvoke();
  return inv('get_task_todos', { taskId });
}

export async function getSubagentRecords(taskId) {
  const inv = await getInvoke();
  return inv('get_subagent_records', { taskId });
}

export async function deleteTask(taskId) {
  const inv = await getInvoke();
  return inv('delete_task', { taskId });
}

export async function deleteTasksForProject(projectId) {
  const inv = await getInvoke();
  return inv('delete_tasks_for_project', { projectId });
}

export async function renameTask(taskId, title) {
  const inv = await getInvoke();
  return inv('rename_task', { taskId, title });
}

export async function setAiProvider(
  providerType, apiKey, model, baseUrl, largeContext,
  customMaxOutputTokens, customInputCost, customOutputCost,
  customCachedInputCost = null, customCachedOutputCost = null,
  customContextWindow = null, customThinkingBudget = null, name = null
) {
  const inv = await getInvoke();
  return inv('set_ai_provider', {
    providerType, apiKey, model, baseUrl,
    largeContext: largeContext ?? null,
    customMaxOutputTokens: customMaxOutputTokens ?? null,
    customInputCost: customInputCost ?? null,
    customOutputCost: customOutputCost ?? null,
    customCachedInputCost: customCachedInputCost ?? null,
    customCachedOutputCost: customCachedOutputCost ?? null,
    customContextWindow: customContextWindow ?? null,
    customThinkingBudget: customThinkingBudget ?? null,
    name: name ?? null,
  });
}

export async function removeAiProvider(providerKey) {
  const inv = await getInvoke();
  return inv('remove_ai_provider', { providerKey });
}

/**
 * List Claude Code slash commands the user can invoke. Returns builtin CLI
 * commands plus user-global (`~/.claude/commands/*.md`) and project-scoped
 * (`<root>/.claude/commands/*.md`) entries, with project overriding user.
 * @param {string | null} [projectRoot] absolute path; null for Global chats
 * @returns {Promise<Array<{name: string, description: string, source: 'builtin' | 'user' | 'project'}>>}
 */
export async function listClaudeCodeSlashCommands(projectRoot = null) {
  const inv = await getInvoke();
  return inv('list_claude_code_slash_commands', { projectRoot: projectRoot || null });
}

/**
 * Static list of Claude Code model aliases (`sonnet` / `opus` / `haiku`).
 * The CLI accepts these directly via `--model <alias>` and resolves to the
 * latest tier — kept backend-side as the single source of truth so any
 * future validation logic doesn't drift.
 * @returns {Promise<string[]>}
 */
export async function listClaudeCodeModels() {
  const inv = await getInvoke();
  return inv('list_claude_code_models');
}

/**
 * Live-fetch the model IDs Codex's `app-server` advertises via the
 * `model/list` JSON-RPC method. Spawns an ephemeral `codex app-server`
 * process for the handshake. Errors bubble up so the caller can render a
 * useful "CLI not found / not signed in" message instead of an empty list.
 * @param {string | null} [binaryPath] absolute path override; null/empty = use PATH
 * @returns {Promise<string[]>}
 */
export async function listCodexModels(binaryPath = null) {
  const inv = await getInvoke();
  return inv('list_codex_models', { binaryPath: binaryPath || null });
}

/**
 * Probe a harness CLI's install + auth state.
 * @param {'ClaudeCode' | 'Codex'} kind
 * @param {string | null} [binaryPath] absolute path override; null/empty = use PATH
 * @returns {Promise<
 *   { status: 'not_installed', reason: string }
 *   | { status: 'not_authenticated', version: string | null }
 *   | { status: 'authenticated', version: string | null }
 *   | { status: 'probe_failed', detail: string }
 * >}
 */
/**
 * Snapshot the live harness task IDs (Claude Code CLI sessions in the
 * `HarnessRegistry`). The agent panel polls this to render the live-agent
 * counter in the header (plan §B.14) and the per-project "agents active"
 * banner (plan §B.6).
 */
export async function harnessActiveTaskIds() {
  const inv = await getInvoke();
  return inv('harness_active_task_ids');
}

/**
 * Multi-client queue events (plan §B.9). Today's single-window Tauri build
 * doesn't have a second viewer, so these are forward-compat — the call
 * round-trips through the backend so any future viewer of the same task
 * can mirror the queue state by listening on `agent-input-queued`.
 *
 * `preview` is a short truncated copy of the message body — full text
 * stays in the originating window. `imageCount` is the number of attached
 * images; `queueDepth` is the resulting queue length after this entry.
 */
export async function notifyInputQueued(taskId, preview, imageCount, queueDepth) {
  const inv = await getInvoke();
  return inv('notify_input_queued', {
    taskId,
    preview: preview || '',
    imageCount: imageCount | 0,
    queueDepth: queueDepth | 0,
  });
}

export async function notifyInputDelivered(taskId, count) {
  const inv = await getInvoke();
  return inv('notify_input_delivered', { taskId, count: count | 0 });
}

export async function onAgentInputQueued(callback) {
  const l = await getListen();
  return l('agent-input-queued', (event) => callback(event.payload));
}

export async function onAgentInputDelivered(callback) {
  const l = await getListen();
  return l('agent-input-delivered', (event) => callback(event.payload));
}

export async function probeHarnessAuth(kind, binaryPath = null) {
  const inv = await getInvoke();
  return inv('probe_harness_auth', { kind, binaryPath: binaryPath || null });
}

export async function fetchAiModels(providerType, apiKey, baseUrl, forceRefresh = false, includeAll = false) {
  const inv = await getInvoke();
  return inv('fetch_ai_models', { providerType, apiKey, baseUrl, forceRefresh, includeAll });
}

/// Built-in model registry (Anthropic / OpenAI / Gemini specs). Used by the
/// Register-model modal as a template list so the user can pick a known model
/// and copy its context/cost specs into a Compatible-provider entry.
export async function listKnownModels() {
  const inv = await getInvoke();
  return inv('list_known_models');
}

export async function getAiConfig() {
  const inv = await getInvoke();
  return inv('get_ai_config');
}

// Configure the cheaper / faster sub-agent model. `providerKey` must match
// an existing connected provider (e.g. "Claude", "Compatible:groq"); `model`
// is the model id sent on the API request. While set, the main agent's
// `spawn_subagent` schema gains a `model_tier` parameter so it can pick
// per-spawn between the main chat model and this one.
export async function setSubagentConfig(providerKey, model) {
  const inv = await getInvoke();
  return inv('set_subagent_config', { providerKey, model });
}

// Remove the sub-agent override. After this, every `spawn_subagent` call
// uses the main chat model and the schema no longer exposes the choice.
export async function clearSubagentConfig() {
  const inv = await getInvoke();
  return inv('clear_subagent_config');
}

export async function getToolConfig() {
  const inv = await getInvoke();
  return inv('get_tool_config');
}

export async function setToolConfig(config) {
  const inv = await getInvoke();
  return inv('set_tool_config', { config });
}

export async function setPermissions(projectId, level) {
  const inv = await getInvoke();
  return inv('set_permissions', { projectId, level });
}

export async function setTaskPermissions(taskId, level) {
  const inv = await getInvoke();
  return inv('set_task_permissions', { taskId, level });
}

// Agent events
export async function onAgentStream(callback) {
  const l = await getListen();
  return l('agent-stream', (event) => callback(event.payload));
}

/// Fired when the Global orchestrator spawns a sub-task in a project.
/// Payload: { task_id, project_id, title, prompt }. Frontend should create
/// the task entry + dispatch the first send_message so the task actually
/// starts running.
export async function onOrchestratorSpawnedTask(callback) {
  const l = await getListen();
  return l('orchestrator-spawned-task', (event) => callback(event.payload));
}

export async function onAgentToolUse(callback) {
  const l = await getListen();
  return l('agent-tool-use', (event) => callback(event.payload));
}

export async function onAgentToolResult(callback) {
  const l = await getListen();
  return l('agent-tool-result', (event) => callback(event.payload));
}

export async function onAgentToolUseStart(callback) {
  const l = await getListen();
  return l('agent-tool-use-start', (event) => callback(event.payload));
}

export async function onAgentToolUseInputDelta(callback) {
  const l = await getListen();
  return l('agent-tool-use-input-delta', (event) => callback(event.payload));
}

export async function onAgentToolUseStop(callback) {
  const l = await getListen();
  return l('agent-tool-use-stop', (event) => callback(event.payload));
}

export async function onAgentToolProgress(callback) {
  const l = await getListen();
  return l('agent-tool-progress', (event) => callback(event.payload));
}

export async function onAgentTaskStatus(callback) {
  const l = await getListen();
  return l('agent-task-status', (event) => callback(event.payload));
}

export async function onAgentTaskComplete(callback) {
  const l = await getListen();
  return l('agent-task-complete', (event) => callback(event.payload));
}

export async function onAgentPermissionRequest(callback) {
  const l = await getListen();
  return l('agent-permission-request', (event) => callback(event.payload));
}

export async function abortTask(taskId) {
  const inv = await getInvoke();
  return inv('abort_task', { taskId });
}

/**
 * @param {string} taskId
 * @param {string} requestId
 * @param {boolean | 'accept' | 'acceptForSession' | 'deny'} decision
 *   Either the legacy boolean (true=allow, false=deny) used by native
 *   API-key providers, or the three-variant string used by harness providers
 *   to surface the "Allow for session" middle option.
 */
export async function respondToPermission(taskId, requestId, decision) {
  const inv = await getInvoke();
  if (typeof decision === 'string') {
    return inv('respond_to_permission', { taskId, requestId, approved: null, decision });
  }
  return inv('respond_to_permission', { taskId, requestId, approved: !!decision, decision: null });
}

export async function setTaskSensitiveAccess(taskId, allowed) {
  const inv = await getInvoke();
  return inv('set_task_sensitive_access', { taskId, allowed });
}

export async function getTaskCost(taskId) {
  const inv = await getInvoke();
  return inv('get_task_cost', { taskId });
}

export async function onAgentCostUpdate(callback) {
  const l = await getListen();
  return l('agent-cost-update', (event) => callback(event.payload));
}

export async function onAgentRequestUsage(callback) {
  const l = await getListen();
  return l('agent-request-usage', (event) => callback(event.payload));
}

export async function onAgentMemoryUpdated(callback) {
  const l = await getListen();
  return l('agent-memory-updated', (event) => callback(event.payload));
}

export async function onAgentFileTracked(callback) {
  const l = await getListen();
  return l('agent-file-tracked', (event) => callback(event.payload));
}

export async function fhListFiles(projectRoot, messageId) {
  const inv = await getInvoke();
  return inv('fh_list_files', { projectRoot, messageId });
}

export async function fhFileDiff(projectRoot, messageId, path) {
  const inv = await getInvoke();
  return inv('fh_file_diff', { projectRoot, messageId, path });
}

export async function fhListSnapshots(taskId) {
  const inv = await getInvoke();
  return inv('fh_list_snapshots', { taskId });
}

export async function fhListTaskNetChanges(projectRoot, taskId) {
  const inv = await getInvoke();
  return inv('fh_list_task_net_changes', { projectRoot, taskId });
}

export async function fhPlanRevertFromMessage(projectRoot, messageId) {
  const inv = await getInvoke();
  return inv('fh_plan_revert_from_message', { projectRoot, messageId });
}

export async function fhRevertFromMessage(projectRoot, messageId) {
  const inv = await getInvoke();
  return inv('fh_revert_from_message', { projectRoot, messageId });
}

export async function fhPlanRevertTask(projectRoot, taskId) {
  const inv = await getInvoke();
  return inv('fh_plan_revert_task', { projectRoot, taskId });
}

export async function fhRevertTask(projectRoot, taskId) {
  const inv = await getInvoke();
  return inv('fh_revert_task', { projectRoot, taskId });
}

export async function truncateTaskMessages(taskId, keepCount) {
  const inv = await getInvoke();
  return inv('truncate_task_messages', { taskId, keepCount });
}

export async function fhRevert(projectRoot, messageId) {
  const inv = await getInvoke();
  return inv('fh_revert', { projectRoot, messageId });
}

export async function getMemory(projectId) {
  const inv = await getInvoke();
  return inv('get_memory', { projectId });
}

export async function clearMemory(projectId) {
  const inv = await getInvoke();
  return inv('clear_memory', { projectId });
}

export async function switchModel(taskId, providerType, model) {
  const inv = await getInvoke();
  return inv('switch_model', { taskId, providerType, model });
}

export async function onAgentModelSwitched(callback) {
  const l = await getListen();
  return l('agent-model-switched', (event) => callback(event.payload));
}

export async function onAgentThinkingDelta(callback) {
  const l = await getListen();
  return l('agent-thinking-delta', (event) => callback(event.payload));
}

export async function onAgentThinkingDone(callback) {
  const l = await getListen();
  return l('agent-thinking-done', (event) => callback(event.payload));
}

export async function getProjectDefaults(projectId) {
  const inv = await getInvoke();
  return inv('get_project_defaults', { projectId });
}

export async function saveProjectDefaults(projectId, defaults) {
  const inv = await getInvoke();
  return inv('save_project_defaults', { projectId, defaults });
}

// Settings commands
export async function getSettings() {
  const inv = await getInvoke();
  return inv('get_settings');
}

export async function updateSettings(settings) {
  const inv = await getInvoke();
  return inv('update_settings', { settings });
}

export async function getActiveTheme() {
  const inv = await getInvoke();
  return inv('get_active_theme');
}

export async function listThemes() {
  const inv = await getInvoke();
  return inv('list_themes');
}

export async function importTheme(path) {
  const inv = await getInvoke();
  return inv('import_theme', { path });
}

export async function importKeybindings(path) {
  const inv = await getInvoke();
  return inv('import_keybindings', { path });
}

export async function detectVscodeKeybindings() {
  const inv = await getInvoke();
  return inv('detect_vscode_keybindings');
}

// Preview / binary file commands
export async function readFileBase64(path) {
  const inv = await getInvoke();
  return inv('read_file_base64', { path });
}

export async function writeFileBase64(path, data) {
  const inv = await getInvoke();
  return inv('write_file_base64', { path, data });
}

export async function readHexChunk(path, offset, length) {
  const inv = await getInvoke();
  return inv('read_hex_chunk', { path, offset, length });
}

export async function getFileSize(path) {
  const inv = await getInvoke();
  return inv('get_file_size', { path });
}

// External file drop listener (Tauri v2 intercepts OS file drops at native level)
export async function onFileDrop(callback) {
  const l = await getListen();
  return l('tauri://drag-drop', (event) => {
    console.log('[DnD] tauri://drag-drop event', event.payload);
    callback(event.payload);
  });
}

export async function onFileDragOver(callback) {
  const l = await getListen();
  return l('tauri://drag-over', (event) => callback(event.payload));
}

export async function onFileDragLeave(callback) {
  const l = await getListen();
  return l('tauri://drag-leave', (event) => callback(event.payload));
}

// File system watcher event
export async function onFsChange(callback) {
  const l = await getListen();
  return l('rustic:fs-change', (event) => callback(event.payload));
}

// MCP commands
// Servers are stored in JSON files (matches Claude Code):
//   user scope:    <app_data_dir>/mcp.json
//   project scope: <project_root>/.mcp.json
export async function readMcpJson(scope, projectId) {
  const inv = await getInvoke();
  return inv('read_mcp_json', { scope, projectId: projectId ?? null });
}

export async function saveMcpJson(scope, projectId, content) {
  const inv = await getInvoke();
  return inv('save_mcp_json', { scope, projectId: projectId ?? null, content });
}

export async function listMcpServers(projectId) {
  const inv = await getInvoke();
  return inv('list_mcp_servers', { projectId: projectId ?? null });
}

export async function testMcpServer(id) {
  const inv = await getInvoke();
  return inv('test_mcp_server', { id });
}

export async function removeMcpServer(id) {
  const inv = await getInvoke();
  return inv('remove_mcp_server', { id });
}

// === Skills (global) ===

export async function listSkills() {
  const inv = await getInvoke();
  return inv('list_skills');
}

export async function getSkillBody(name) {
  const inv = await getInvoke();
  return inv('get_skill_body', { name });
}

export async function createSkill(name, body) {
  const inv = await getInvoke();
  return inv('create_skill', { name, body });
}

export async function updateSkill(originalName, name, body) {
  const inv = await getInvoke();
  return inv('update_skill', { originalName, name, body });
}

export async function deleteSkill(name) {
  const inv = await getInvoke();
  return inv('delete_skill', { name });
}

export async function listRepoSkills(source) {
  const inv = await getInvoke();
  return inv('list_repo_skills', { source });
}

export async function installRepoSkills(source, paths, names = null) {
  const inv = await getInvoke();
  return inv('install_repo_skills', { source, paths, names });
}

export async function previewRepoSkill(source, path) {
  const inv = await getInvoke();
  return inv('preview_repo_skill', { source, path });
}

// === Workflows (global) ===

export async function listWorkflows() {
  const inv = await getInvoke();
  return inv('list_workflows');
}

export async function getWorkflowBody(name) {
  const inv = await getInvoke();
  return inv('get_workflow_body', { name });
}

export async function createWorkflow(name, body) {
  const inv = await getInvoke();
  return inv('create_workflow', { name, body });
}

export async function updateWorkflow(originalName, name, body) {
  const inv = await getInvoke();
  return inv('update_workflow', { originalName, name, body });
}

export async function deleteWorkflow(name) {
  const inv = await getInvoke();
  return inv('delete_workflow', { name });
}

export async function listRepoWorkflows(source) {
  const inv = await getInvoke();
  return inv('list_repo_workflows', { source });
}

export async function installRepoWorkflows(source, paths, names = null) {
  const inv = await getInvoke();
  return inv('install_repo_workflows', { source, paths, names });
}

export async function previewRepoWorkflow(source, path) {
  const inv = await getInvoke();
  return inv('preview_repo_workflow', { source, path });
}

// === Rules (global definitions, per-project activation) ===

export async function listRules(projectRoot = null) {
  const inv = await getInvoke();
  return inv('list_rules', { projectRoot });
}

export async function getRuleBody(name) {
  const inv = await getInvoke();
  return inv('get_rule_body', { name });
}

export async function createRule(name, body) {
  const inv = await getInvoke();
  return inv('create_rule', { name, body });
}

export async function updateRule(originalName, name, body) {
  const inv = await getInvoke();
  return inv('update_rule', { originalName, name, body });
}

export async function deleteRule(name) {
  const inv = await getInvoke();
  return inv('delete_rule', { name });
}

export async function setRuleActivation(name, state, projectRoot = null) {
  const inv = await getInvoke();
  return inv('set_rule_activation', { name, state, projectRoot });
}

// === Sub-agent events ===

export async function onAgentSubagentSpawned(callback) {
  const l = await getListen();
  return l('agent-subagent-spawned', (event) => callback(event.payload));
}

export async function onAgentSubagentCompleted(callback) {
  const l = await getListen();
  return l('agent-subagent-completed', (event) => callback(event.payload));
}

export async function onAgentSubagentFailed(callback) {
  const l = await getListen();
  return l('agent-subagent-failed', (event) => callback(event.payload));
}

export async function onAgentSubagentTextDelta(callback) {
  const l = await getListen();
  return l('agent-subagent-text-delta', (event) => callback(event.payload));
}

export async function onAgentSubagentCostUpdate(callback) {
  const l = await getListen();
  return l('agent-subagent-cost-update', (event) => callback(event.payload));
}

export async function onAgentSubagentToolUse(callback) {
  const l = await getListen();
  return l('agent-subagent-tool-use', (event) => callback(event.payload));
}

export async function onAgentSubagentToolResult(callback) {
  const l = await getListen();
  return l('agent-subagent-tool-result', (event) => callback(event.payload));
}

export async function onAgentQuestionRequest(callback) {
  const l = await getListen();
  return l('agent-question-request', (event) => callback(event.payload));
}

export async function onAgentTodoUpdated(callback) {
  const l = await getListen();
  return l('agent-todo-updated', (event) => callback(event.payload));
}

export async function onAgentTitleChanged(callback) {
  const l = await getListen();
  return l('agent-title-changed', (event) => callback(event.payload));
}

export async function onAgentContextCondenseStarted(callback) {
  const l = await getListen();
  return l('agent-context-condense-started', (event) => callback(event.payload));
}

export async function onAgentContextCondenseCompleted(callback) {
  const l = await getListen();
  return l('agent-context-condense-completed', (event) => callback(event.payload));
}
