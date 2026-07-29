// In-app patch notes, newest first. Add an entry per release; the What's New
// dialog auto-shows it once when the running app version matches.
export const CHANGELOG = [
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
