// Registry for composer slash commands (the `/` menu alongside skills and
// workflows). Each command owns its parsing; the composer intercepts a
// registered command on submit instead of sending the text as a prompt.

export const SLASH_COMMANDS = [
  {
    name: 'goal',
    hint: '<condition> — or "clear"',
    description:
      'Keep the agent working until the condition is verified by an evaluator model. "/goal clear" cancels.',
  },
];

/** Returns registry entries matching a slash-menu query prefix. */
export function matchSlashCommands(query) {
  const q = (query || '').trim().toLowerCase();
  if (!q) return SLASH_COMMANDS;
  return SLASH_COMMANDS.filter(
    (c) => c.name.startsWith(q) || c.description.toLowerCase().includes(q),
  );
}

/** Parses submitted text into { command, args } when it invokes a registered command. */
export function parseSlashCommand(text) {
  const m = /^\/([a-z][\w-]*)(?:\s+([\s\S]*))?$/i.exec((text || '').trim());
  if (!m) return null;
  const cmd = SLASH_COMMANDS.find((c) => c.name === m[1].toLowerCase());
  if (!cmd) return null;
  return { command: cmd.name, args: (m[2] || '').trim() };
}
