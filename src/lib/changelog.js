// In-app patch notes, newest first. Add an entry per release; the What's New
// dialog auto-shows it once when the running app version matches.
export const CHANGELOG = [
  {
    version: '0.7.6',
    date: 'October 2026',
    entries: [
      { tag: 'new', text: 'Themes — a new theme manager with built-in Obsidian, Basalt, Hearth, Petal, Midnight, Onyx plus style starters Neo Brutal, Grove and Neon Grid. Import, download and edit `.rustic-theme.json` files; every theme with custom CSS is scanned and asks for your trust before it applies.' },
      { tag: 'new', text: 'The agent can list, read, create and remove themes for you (new built-in `rustic-themes` skill); anything it writes waits for your approval.' },
      { tag: 'new', text: 'Sync redesign — Cloud & Sync lives in the terminal island with a five-panel browser (machines, projects, files, preview, your explorer), per-file pull/upload, drag and drop both ways, and one approval per batch.' },
      { tag: 'new', text: 'Transfers tray — see size, progress, speed and ETA for every sync transfer, cancel it, and open the destination when done.' },
      { tag: 'improved', text: 'Sync no longer overwrites on name clashes — you choose rename, auto-number or replace. Paired machines must run the same Rustic version and show "update required" otherwise.' },
      { tag: 'new', text: 'Queued messages — messages sent while the agent is busy now line up in a new Queued tab on the chat dock, where you can send one now or discard it. Multiple messages queue in order instead of overwriting each other.' },
      { tag: 'improved', text: 'Settings help text moved behind (i) info icons for a cleaner layout.' },
    ],
  },
  {
    version: '0.7.5',
    date: 'October 2026',
    entries: [
      { tag: 'fixed', text: 'Chats no longer stall when a model (e.g. Xiaomi MiMo, Qwen3-Coder) writes its tool calls as plain text — Rustic now recognises and runs them instead of silently ending the turn.' },
      { tag: 'improved', text: 'Sync — pushing or pulling a project is only blocked by agents running in that project, not by agents working anywhere else.' },
      { tag: 'new', text: 'CLI agents — Claude Code, Codex and Antigravity show as greyed-out icons when not installed; click one to install it in a terminal, and it lights up once ready.' },
      { tag: 'improved', text: 'Rustic Server keeps everything you install (npm globals, CLI agents, pipx/uv tools) and their logins on the persistent volume, so they survive redeploys.' },
      { tag: 'improved', text: 'MCP — remote servers configured with `serverUrl` / `httpUrl` (Google Stitch, Gemini CLI style) are accepted, and auth failures show the real error instead of a misleading 405.' },
      { tag: 'improved', text: 'Desktop window can be dragged, snapped and maximised (double-click) from any empty spot in the top bar.' },
      { tag: 'fixed', text: 'Rules editor scrolls long rules instead of pushing the Save button off-screen.' },
      { tag: 'fixed', text: "What's New now shows the current release's notes." },
    ],
  },
  {
    version: '0.7.0',
    date: 'October 2026',
    entries: [
      { tag: 'new', text: 'Cloud & Sync rework — a "My machine" view plus Sync / Backends tabs, per-machine Push, Pull and Sharing with approval prompts and share allowlists, and Cloudflare tunnel / port forwarding for reaching machines outside your network.' },
      { tag: 'new', text: 'Rustic Server is now a full sync peer, so a cloud deployment can push and pull with your desktop like any other machine.' },
      { tag: 'new', text: 'Multiple remote backends — each opens in its own window with its own keychain entry, and files can be dragged and dropped between backend windows.' },
      { tag: 'improved', text: 'MCP — paste standard `mcpServers` JSON anywhere (Settings or via the agent). Add / Configure dialogs are now two-column with a Test button to check the connection before saving.' },
      { tag: 'new', text: 'Chat image viewer — step through images in a conversation with previous / next.' },
      { tag: 'improved', text: 'Settings lists use a cleaner connected-row layout throughout.' },
      { tag: 'fixed', text: 'LAN sync works offline again (fixed port 47820 with peer discovery probing).' },
      { tag: 'fixed', text: 'Opening a remote backend window no longer freezes the app on Windows.' },
    ],
  },
  {
    version: '0.6.0',
    date: 'October 2026',
    entries: [
      { tag: 'new', text: 'Per-project MCP servers — enable or disable servers per project; Rustic keeps `.mcp.json`, Gemini CLI and Codex configs in sync so external agents see the same servers.' },
      { tag: 'new', text: 'Cloud settings — remote and LAN sync between machines, with metadata (tasks, settings) merged rather than overwritten.' },
      { tag: 'improved', text: 'Sync transfers are streamed in parallel, resumable chunks, so large projects sync faster and survive dropped connections.' },
      { tag: 'new', text: 'Agent extension tools — the agent can list, install and uninstall skills, workflows and MCP servers itself (with your approval where needed).' },
      { tag: 'improved', text: 'Live file refresh — files changed on disk outside Rustic update in the explorer and editor automatically.' },
      { tag: 'fixed', text: 'Chat reliability — fixes for ask-user prompts, stream interruptions and stalled responses.' },
      { tag: 'new', text: 'Releases now ship for Windows, macOS (Apple Silicon and Intel) and Linux.' },
    ],
  },
  {
    version: '0.5.2',
    date: 'September 2026',
    entries: [
      { tag: 'new', text: 'Agent `sleep` tool — the agent can now wait a fixed number of seconds (server warm-up, polling, cooldowns) without spawning a shell that gets parked in a background terminal. The tool card shows a live countdown and Stop cancels it instantly.' },
      { tag: 'fixed', text: 'GPT-6 and newer OpenAI models now route through the Responses API, fixing the "Function tools with reasoning_effort are not supported" 400 on /chat/completions.' },
      { tag: 'fixed', text: 'OpenAI reasoning effort — the Medium tier was silently sent as "low". Tiers now map to the effort of the same name, GPT-6+ can use xhigh / max, and raw reasoning text from newer models appears in the thinking panel.' },
      { tag: 'fixed', text: 'Source Control — switching branches no longer looks frozen: the picker closes immediately and shows a spinner, the branch name updates as soon as git reports it, and a stuck git call can no longer wedge the panel until restart.' },
      { tag: 'new', text: 'Per-model request parameters — Register / Edit model → "Request parameters (advanced)" lets you force, omit or override provider fields; rejected parameters are learned automatically from the provider\u2019s 400 and remembered.' },
      { tag: 'improved', text: 'Explorer, Search and Source Control now auto-expand and scroll to the active file\u2019s project when opened.' },
      { tag: 'fixed', text: 'PDF text extraction runs in a separate worker process, so a malformed PDF can no longer crash the app.' },
      { tag: 'improved', text: 'Reliability — chat history is persisted incrementally, messages sent mid-run are queued instead of dropped, oversized images are stubbed rather than failing the request, and provider connections time out after 20s instead of hanging.' },
      { tag: 'improved', text: 'Touch and tablet support — long-press context menus, larger hit targets and overflow menus on coarse-pointer devices.' },
    ],
  },
  {
    version: '0.5.1',
    date: 'July 2026',
    entries: [
      { tag: 'improved', text: 'Adding a folder that is already in your workspace no longer re-imports it — Rustic tells you it is already there and highlights the existing project instead.' },
      { tag: 'new', text: 'Projects whose folder has been renamed, moved or deleted outside the app are now removed from the Explorer automatically, so you no longer have to clean up dead entries by hand. Their task history is kept — re-add the folder and it comes back.' },
      { tag: 'new', text: 'Audio preview — click an audio file to play it inline in the editor.' },
      { tag: 'improved', text: 'Preview zoom — images, PDFs, SVG and HTML previews share consistent zoom / fit controls.' },
    ],
  },
  {
    version: '0.5.0',
    date: 'July 2026',
    entries: [
      { tag: 'improved', text: 'Faster project indexing — the code-intelligence index now builds in parallel across CPU cores, so symbol search and code navigation become available sooner on large projects.' },
      { tag: 'improved', text: 'Snappier app — database reads are tuned with a larger cache and memory-mapped IO, cutting momentary hitches when opening chats and switching projects.' },
      { tag: 'improved', text: 'Smoother chat scrolling — long transcripts no longer re-render messages you have already seen, reducing jank on lengthy conversations.' },
    ],
  },
  {
    version: '0.4.9',
    date: 'July 2026',
    entries: [
      { tag: 'new', text: 'Cloud Sync — push your entire environment (projects, agent tasks & chat history, API keys) to your rustic-server, or pull it back down. Settings → General → Cloud Sync.' },
      { tag: 'new', text: 'Incremental sync — projects unchanged since the last sync are skipped automatically, so repeat syncs are fast.' },
      { tag: 'improved', text: 'Sync uploads are zstd-compressed for smaller, faster transfers.' },
      { tag: 'improved', text: 'Agent search — grep_search now runs on ripgrep\u2019s engine: faster, parallel-safe, with instant binary-file detection.' },
      { tag: 'fixed', text: 'Parallel agents no longer overwrite each other — when two agents edit the same file at once, both changes are kept instead of the last writer silently erasing the first.' },
      { tag: 'new', text: 'Automatic 3-way merge for concurrent edits, with a structural safety check: a merge that would leave a call to a deleted function, a duplicate declaration, or a stale call signature is never accepted silently.' },
      { tag: 'new', text: 'Conflict resolver — when two edits genuinely cannot be combined, a dedicated agent reconciles them (or asks you) instead of failing the write. Write tool cards now show an auto-merged / resolved / conflict badge.' },
      { tag: 'fixed', text: 'Remote backend — connecting to a remote server no longer loses the window controls (minimize / maximize / close); the window switches to the native title bar.' },
    ],
  },
  {
    version: '0.4.8',
    date: 'July 2026',
    entries: [
      { tag: 'new', text: 'Right dock — a second dynamic island on the right edge. Open Explorer, Search, Source Control, or the Agent tree as a floating panel, independent of the left sidebar.' },
      { tag: 'new', text: "What's New dialog — release notes now pop up once after every update (you're looking at it). Re-open anytime from Settings → General." },
      { tag: 'fixed', text: 'Agent memory — the agent no longer loses track of earlier context and decisions during long tasks.' },
      { tag: 'fixed', text: 'Premature context condensing — auto-condense no longer kicks in too early and trims recent history.' },
      { tag: 'improved', text: 'Terminal — commands now run in the background, so long-running commands no longer block the chat.' },
      { tag: 'fixed', text: 'grep & glob tools — more accurate matching and saner file filtering.' },
      { tag: 'improved', text: 'Batch operations reworked across core tools (read, edit, create, search) for faster multi-file work.' },
      { tag: 'improved', text: 'Async workflows — smoother coordination of background work and sub-agents.' },
      { tag: 'improved', text: 'Chat repair — handles more provider edge cases and recovers broken conversations more reliably.' },
    ],
  },
];

export const LATEST_NOTES = CHANGELOG[0];

/** Returns the changelog entry for an exact app version, or null. */
export function notesForVersion(version) {
  return CHANGELOG.find((e) => e.version === version) ?? null;
}
