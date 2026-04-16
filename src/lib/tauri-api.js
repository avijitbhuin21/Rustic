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

export async function saveFile(bufferId) {
  const inv = await getInvoke();
  return inv('save_file', { bufferId });
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

// Search commands
export async function searchInProject(projectId, pattern, isRegex, caseSensitive, wholeWord, includeGlob, excludeGlob) {
  const inv = await getInvoke();
  return inv('search_in_project', { projectId, pattern, isRegex, caseSensitive, wholeWord, includeGlob, excludeGlob });
}

export async function searchGlobal(pattern, isRegex, caseSensitive, wholeWord, includeGlob, excludeGlob) {
  const inv = await getInvoke();
  return inv('search_global', { pattern, isRegex, caseSensitive, wholeWord, includeGlob, excludeGlob });
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

export async function sendMessage(taskId, message, thinkingBudget) {
  const inv = await getInvoke();
  return inv('send_message', { taskId, message, thinkingBudget: thinkingBudget ?? null });
}

export async function listTasks(projectId) {
  const inv = await getInvoke();
  return inv('list_tasks', { projectId });
}

export async function getTaskMessages(taskId) {
  const inv = await getInvoke();
  return inv('get_task_messages', { taskId });
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

export async function setAiProvider(providerType, apiKey, model, baseUrl, largeContext, customMaxOutputTokens, customInputCost, customOutputCost) {
  const inv = await getInvoke();
  return inv('set_ai_provider', {
    providerType, apiKey, model, baseUrl,
    largeContext: largeContext ?? null,
    customMaxOutputTokens: customMaxOutputTokens ?? null,
    customInputCost: customInputCost ?? null,
    customOutputCost: customOutputCost ?? null,
  });
}

export async function fetchAiModels(providerType, apiKey, baseUrl) {
  const inv = await getInvoke();
  return inv('fetch_ai_models', { providerType, apiKey, baseUrl });
}

export async function getAiConfig() {
  const inv = await getInvoke();
  return inv('get_ai_config');
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

export async function onAgentToolUse(callback) {
  const l = await getListen();
  return l('agent-tool-use', (event) => callback(event.payload));
}

export async function onAgentToolResult(callback) {
  const l = await getListen();
  return l('agent-tool-result', (event) => callback(event.payload));
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

export async function respondToPermission(taskId, requestId, approved) {
  const inv = await getInvoke();
  return inv('respond_to_permission', { taskId, requestId, approved });
}

export async function respondToQuestion(taskId, requestId, answer) {
  const inv = await getInvoke();
  return inv('respond_to_question', { taskId, requestId, answer });
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

export async function extendTurnBudget(taskId, additional) {
  const inv = await getInvoke();
  return inv('extend_turn_budget', { taskId, additional });
}

export async function onAgentTurnBudgetWarning(callback) {
  const l = await getListen();
  return l('agent-turn-budget-warning', (event) => callback(event.payload));
}

export async function onAgentMemoryUpdated(callback) {
  const l = await getListen();
  return l('agent-memory-updated', (event) => callback(event.payload));
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

// LSP commands
export async function lspNotifyOpen(bufferId) {
  const inv = await getInvoke();
  return inv('lsp_notify_open', { bufferId });
}

export async function lspNotifyChange(bufferId, version) {
  const inv = await getInvoke();
  return inv('lsp_notify_change', { bufferId, version });
}

export async function lspNotifySave(bufferId) {
  const inv = await getInvoke();
  return inv('lsp_notify_save', { bufferId });
}

export async function lspNotifyClose(bufferId) {
  const inv = await getInvoke();
  return inv('lsp_notify_close', { bufferId });
}

export async function getCompletions(bufferId, line, col) {
  const inv = await getInvoke();
  return inv('get_completions', { bufferId, line, col });
}

export async function getHover(bufferId, line, col) {
  const inv = await getInvoke();
  return inv('get_hover', { bufferId, line, col });
}

export async function gotoDefinition(bufferId, line, col) {
  const inv = await getInvoke();
  return inv('goto_definition', { bufferId, line, col });
}

export async function formatDocument(bufferId) {
  const inv = await getInvoke();
  return inv('format_document', { bufferId });
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

// Checkpoint commands
export async function listCheckpoints(taskId) {
  const inv = await getInvoke();
  return inv('list_checkpoints', { taskId });
}

export async function revertToCheckpoint(checkpointId) {
  const inv = await getInvoke();
  return inv('revert_to_checkpoint', { checkpointId });
}

export async function previewCheckpoint(checkpointId) {
  const inv = await getInvoke();
  return inv('preview_checkpoint', { checkpointId });
}

export async function getCheckpointDiff(taskId, checkpointId) {
  const inv = await getInvoke();
  return inv('get_checkpoint_diff', { taskId, checkpointId });
}

export async function truncateTaskMessages(taskId, messageIndex) {
  const inv = await getInvoke();
  return inv('truncate_task_messages', { taskId, messageIndex });
}

// Preview / binary file commands
export async function readFileBase64(path) {
  const inv = await getInvoke();
  return inv('read_file_base64', { path });
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
export async function addMcpServer(name, transportType, command, args, url) {
  const inv = await getInvoke();
  return inv('add_mcp_server', { name, transportType, command, args, url });
}

export async function removeMcpServer(id) {
  const inv = await getInvoke();
  return inv('remove_mcp_server', { id });
}

export async function listMcpServers() {
  const inv = await getInvoke();
  return inv('list_mcp_servers');
}

export async function testMcpServer(id) {
  const inv = await getInvoke();
  return inv('test_mcp_server', { id });
}

export async function importMcpJson(projectId) {
  const inv = await getInvoke();
  return inv('import_mcp_json', { projectId });
}

// === Skills ===

export async function listSkills(projectId) {
  const inv = await getInvoke();
  return inv('list_skills', { projectId });
}

export async function getSkillBody(projectId, name) {
  const inv = await getInvoke();
  return inv('get_skill_body', { projectId, name });
}

export async function createSkill(projectId, name, description, body) {
  const inv = await getInvoke();
  return inv('create_skill', { projectId, name, description, body });
}

export async function deleteSkill(projectId, name) {
  const inv = await getInvoke();
  return inv('delete_skill', { projectId, name });
}

export async function installSkill(projectId, source) {
  const inv = await getInvoke();
  return inv('install_skill', { projectId, source });
}

// === Workflows ===

export async function listWorkflows(projectId) {
  const inv = await getInvoke();
  return inv('list_workflows', { projectId });
}

export async function getWorkflowBody(projectId, name) {
  const inv = await getInvoke();
  return inv('get_workflow_body', { projectId, name });
}

export async function createWorkflow(projectId, name, description, body) {
  const inv = await getInvoke();
  return inv('create_workflow', { projectId, name, description, body });
}

export async function deleteWorkflow(projectId, name) {
  const inv = await getInvoke();
  return inv('delete_workflow', { projectId, name });
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
