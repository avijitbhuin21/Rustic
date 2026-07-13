// In-app patch notes, newest first. Add an entry per release; the What's New
// dialog auto-shows it once when the running app version matches.
export const CHANGELOG = [
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
