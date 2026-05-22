import { useEffect } from 'react';

const FONT_APP_KEY = 'rustic_font_applications';

function getFontApplications() {
  try { return JSON.parse(localStorage.getItem(FONT_APP_KEY) || '{}'); }
  catch { return {}; }
}

function quote(name) {
  return `"${name}"`;
}

// Build per-target CSS. Selectors target stable data-* attributes / unique
// classes that the relevant components emit, so renaming Tailwind utilities
// in those components won't silently break font application.
function buildFontCSS() {
  const apps = getFontApplications();
  const rules = [];

  if (apps.folderNames) {
    rules.push(`[data-explorer-node="folder"], [data-explorer-node="folder"] span.truncate { font-family: ${quote(apps.folderNames)}, sans-serif !important; }`);
  }
  if (apps.fileNames) {
    rules.push(`[data-explorer-node="file"], [data-explorer-node="file"] span.truncate { font-family: ${quote(apps.fileNames)}, sans-serif !important; }`);
  }
  if (apps.agentChat) {
    rules.push(`[data-agent-message], [data-agent-message] * { font-family: ${quote(apps.agentChat)}, sans-serif !important; }`);
  }
  if (apps.tabLabels) {
    rules.push(`.group\\/tab span.truncate { font-family: ${quote(apps.tabLabels)}, sans-serif !important; }`);
  }
  if (apps.searchResults) {
    rules.push(`[data-search-match], [data-search-match] mark { font-family: ${quote(apps.searchResults)}, sans-serif !important; }`);
  }

  return rules.join('\n');
}

export function FontBridge() {
  useEffect(() => {
    let styleEl = document.getElementById('rustic-font-bridge');
    if (!styleEl) {
      styleEl = document.createElement('style');
      styleEl.id = 'rustic-font-bridge';
      document.head.appendChild(styleEl);
    }

    styleEl.textContent = buildFontCSS();

    function handleFontChange() {
      styleEl.textContent = buildFontCSS();
    }

    window.addEventListener('rustic:font-applied', handleFontChange);
    return () => window.removeEventListener('rustic:font-applied', handleFontChange);
  }, []);

  return null;
}
