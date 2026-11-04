// Cross-view sync for the agent library (skills, workflows, rules, MCP):
// every mutation announces itself so the Agent tools sheet and Settings ›
// Library (and anything else listening) reload their lists.

/** Tell every open library view that skills / workflows / rules changed. */
export function notifyExtensionsChanged(detail) {
  window.dispatchEvent(new CustomEvent('rustic:extensions-changed', { detail }));
}
