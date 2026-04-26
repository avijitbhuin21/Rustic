// Central command registry. Components register commands by id, and the
// keybinding dispatcher (and command palette) execute them by id. Keeping
// the catalog in one place lets the Shortcuts settings UI list every
// command the app knows about without each component re-exporting handlers.

const commands = new Map(); // id -> { id, title, category, run, allowInInput }

export function registerCommand(cmd) {
  if (!cmd?.id || typeof cmd.run !== 'function') {
    throw new Error('registerCommand requires { id, run }');
  }
  commands.set(cmd.id, {
    id: cmd.id,
    title: cmd.title || cmd.id,
    category: cmd.category || 'Other',
    allowInInput: !!cmd.allowInInput,
    run: cmd.run,
  });
}

export function unregisterCommand(id) {
  commands.delete(id);
}

export function getCommand(id) {
  return commands.get(id);
}

export function getAllCommands() {
  return Array.from(commands.values()).sort((a, b) => {
    if (a.category !== b.category) return a.category.localeCompare(b.category);
    return a.title.localeCompare(b.title);
  });
}

export async function executeCommand(id, ...args) {
  const cmd = commands.get(id);
  if (!cmd) {
    console.warn('Unknown command:', id);
    return;
  }
  try {
    await cmd.run(...args);
  } catch (err) {
    console.error(`Command "${id}" failed:`, err);
  }
}
