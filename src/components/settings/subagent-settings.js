import { el, icon } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';
import { loadProviderConfigs } from './ai-settings.js';

// localStorage cache so the picker can render the saved selection
// before the backend's getAiConfig() round-trip lands. The backend is
// authoritative — this is just a paint-time hint.
const LOCAL_KEY = 'rustic_subagent_selection';

function loadLocal() {
  try {
    const raw = JSON.parse(localStorage.getItem(LOCAL_KEY) || 'null');
    if (raw && typeof raw.providerKey === 'string' && typeof raw.model === 'string') {
      return raw;
    }
  } catch {}
  return null;
}

function saveLocal(sel) {
  if (!sel) {
    localStorage.removeItem(LOCAL_KEY);
  } else {
    localStorage.setItem(LOCAL_KEY, JSON.stringify(sel));
  }
}

// Flatten every connected provider into [{ providerKey, providerLabel, model }]
// rows. We render them in a custom searchable picker (input + filtered dropdown)
// — a native <select> can't filter as the user types and was getting cramped
// inside the settings-row two-column layout.
function gatherOptions() {
  const configs = loadProviderConfigs();
  const out = [];
  for (const [storageKey, cfg] of Object.entries(configs)) {
    if (!cfg?.hasKey || !Array.isArray(cfg.models) || cfg.models.length === 0) continue;
    const providerLabel = cfg.name || storageKey;
    for (const model of cfg.models) {
      out.push({
        providerKey: storageKey,
        providerLabel,
        model,
      });
    }
  }
  return out;
}

export function createSubagentSettings() {
  const container = el('div', { class: 'subagent-settings' });

  const desc = el('div', { class: 'subagent-settings__desc' },
    'Pick a cheaper, faster model the agent can route mechanical sub-agent work to. ' +
    'When set, the main agent picks per-spawn whether the sub-agent runs on the main chat ' +
    'model (best for reasoning) or this one (best for bulk reads, simple edits, summarising). ' +
    'Leave unset to always reuse the main model.');
  container.appendChild(desc);

  // ── Searchable picker ───────────────────────────────────────────────
  const pickerWrap = el('div', { class: 'subagent-picker' });
  const inputRow = el('div', { class: 'subagent-picker__input-row' });

  const input = el('input', {
    class: 'subagent-picker__input',
    type: 'text',
    placeholder: 'Search a model — e.g. haiku, gpt-4.1-mini, gemini-flash…',
    autocomplete: 'off',
    spellcheck: 'false',
  });
  inputRow.appendChild(input);

  const clearBtn = el('button', {
    class: 'subagent-picker__clear-btn',
    type: 'button',
    title: 'Remove sub-agent override',
  });
  // Trash / dustbin icon — same path the rules and skills delete buttons use
  // so the affordance reads consistently across the settings panels.
  clearBtn.appendChild(icon('M3 6h18 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6', 14));
  inputRow.appendChild(clearBtn);
  pickerWrap.appendChild(inputRow);

  // Menu lives directly under <body> while open — the surrounding
  // settings-collapsible body has `overflow: hidden`, which would clip a
  // normally-positioned absolute child. Detaching to <body> + position:fixed
  // lets the dropdown escape that clipping context and stack over the panels
  // below it.
  const menu = el('div', { class: 'subagent-picker__menu' });
  container.appendChild(pickerWrap);

  const status = el('div', { class: 'subagent-settings__status' });
  container.appendChild(status);

  // ── Concurrency cap row ────────────────────────────────────────────
  // Bounds parallel `spawn_subagent` fan-out under one parent task.
  // Persisted server-side in `BudgetSettings.max_concurrent_subagents`
  // for storage convenience; the UI lives here because conceptually it's
  // a sub-agent concern, not a cross-task budget. Reuses the budget-
  // settings row CSS so visual styling stays in sync across the two
  // panels (toggle switch, dim-when-off, etc).
  const DEFAULT_MAX_SUBAGENTS = 4;
  const capSection = el('div', { class: 'subagent-settings__cap-section' });
  capSection.appendChild(el('div', { class: 'settings-subsection-title' }, 'Concurrency'));

  const capRow = el('div', { class: 'settings-row budget-settings__row' });
  const capInfo = el('div', { class: 'settings-row__info' });
  capInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Cap parallel sub-agents per task'));
  capInfo.appendChild(el('div', { class: 'settings-row__desc' },
    'How many `spawn_subagent` calls can run simultaneously under one parent task. ' +
    `Default ${DEFAULT_MAX_SUBAGENTS}. Uncheck to lift the cap entirely (rate-limit safety still comes from the global stream cap in the Budget panel).`));
  capRow.appendChild(capInfo);

  const capControl = el('div', { class: 'budget-settings__control' });
  const capToggleLabel = el('label', { class: 'budget-settings__toggle' });
  const capCheck = el('input', { type: 'checkbox', class: 'budget-settings__check' });
  capToggleLabel.appendChild(capCheck);
  capToggleLabel.appendChild(el('span', { class: 'budget-settings__toggle-track' }));
  capControl.appendChild(capToggleLabel);

  const capInput = el('input', {
    type: 'number',
    class: 'settings-input settings-input--number budget-settings__input',
    min: '1',
    step: '1',
    value: String(DEFAULT_MAX_SUBAGENTS),
  });
  capControl.appendChild(capInput);
  capControl.appendChild(el('span', { class: 'budget-settings__affix budget-settings__affix--suffix' }, 'sub-agents'));
  capRow.appendChild(capControl);
  capSection.appendChild(capRow);

  const capStatus = el('div', { class: 'subagent-settings__cap-status' });
  capSection.appendChild(capStatus);

  container.appendChild(capSection);

  function syncCapDisabledState() {
    capInput.disabled = !capCheck.checked;
    capRow.classList.toggle('budget-settings__row--off', !capCheck.checked);
  }

  // Debounced save — the cap is a single field, persisting on every
  // change keystroke is cheap server-side but we still coalesce so we
  // don't fire a flurry of writes while the user types a multi-digit
  // number.
  let capSaveTimer = null;
  function scheduleCapSave() {
    if (capSaveTimer) clearTimeout(capSaveTimer);
    capSaveTimer = setTimeout(commitCap, 300);
  }

  async function commitCap() {
    capSaveTimer = null;
    const cap = capCheck.checked
      ? Math.max(1, parseInt(capInput.value, 10) || DEFAULT_MAX_SUBAGENTS)
      : null;
    capStatus.textContent = 'Saving…';
    try {
      await api.setSubagentConcurrencyCap(cap);
      capStatus.textContent = cap == null
        ? 'Saved — sub-agents run uncapped.'
        : `Saved — at most ${cap} sub-agents at once.`;
    } catch (e) {
      capStatus.textContent = `Save failed: ${e?.message || e}`;
    }
  }

  capCheck.addEventListener('change', () => { syncCapDisabledState(); scheduleCapSave(); });
  capInput.addEventListener('input', scheduleCapSave);
  // Commit immediately on blur too, in case the user tabs / clicks away
  // before the debounce timer fires.
  capInput.addEventListener('change', () => { if (capSaveTimer) { clearTimeout(capSaveTimer); commitCap(); } });

  (async () => {
    try {
      const cap = await api.getSubagentConcurrencyCap();
      capCheck.checked = cap != null;
      capInput.value = String(cap != null ? cap : DEFAULT_MAX_SUBAGENTS);
      syncCapDisabledState();
    } catch (e) {
      capStatus.textContent = `Couldn't load cap: ${e?.message || e}`;
    }
  })();

  let allOptions = [];
  let currentSelection = null; // { providerKey, model } or null
  let menuOpen = false;
  let highlightIdx = -1;

  function setStatus(msg, type = '') {
    status.textContent = msg;
    status.className = 'subagent-settings__status' + (type ? ` subagent-settings__status--${type}` : '');
  }

  function formatSelected(sel) {
    if (!sel) return '';
    const opt = allOptions.find((o) => o.providerKey === sel.providerKey && o.model === sel.model);
    if (opt) return `${opt.providerLabel} — ${opt.model}`;
    // Selected entry is no longer in the connected provider list — show what
    // was saved so the user can tell something's off.
    return `${sel.providerKey} — ${sel.model}`;
  }

  function positionMenu() {
    // Anchor the floating menu under the input. Width matches the input
    // exactly so the dropdown lines up edge-to-edge with the search box —
    // the user asked for "same size as the input."
    const rect = input.getBoundingClientRect();
    menu.style.top = `${Math.round(rect.bottom + 4)}px`;
    menu.style.left = `${Math.round(rect.left)}px`;
    menu.style.width = `${Math.round(rect.width)}px`;
  }

  // Reposition while the menu is open so the dropdown follows the input if
  // the user scrolls the settings panel or resizes the window. Capture-phase
  // scroll catches nested scroll containers (e.g. the settings body).
  const onReposition = () => {
    if (menuOpen) positionMenu();
  };

  function openMenu() {
    if (menuOpen) return;
    menuOpen = true;
    pickerWrap.classList.add('subagent-picker--open');
    if (!menu.isConnected) document.body.appendChild(menu);
    positionMenu();
    renderMenu(input.value);
    window.addEventListener('scroll', onReposition, true);
    window.addEventListener('resize', onReposition);
  }

  function closeMenu() {
    if (!menuOpen) return;
    menuOpen = false;
    pickerWrap.classList.remove('subagent-picker--open');
    highlightIdx = -1;
    if (menu.isConnected) menu.remove();
    window.removeEventListener('scroll', onReposition, true);
    window.removeEventListener('resize', onReposition);
    // Restore the displayed text to the saved selection so a typed-but-not-
    // committed query doesn't linger after the menu closes.
    input.value = formatSelected(currentSelection);
  }

  function filteredOptions(query) {
    const q = (query || '').trim().toLowerCase();
    if (!q) return allOptions;
    // Match against either provider label or model id — the user might
    // search by provider ("openai", "claude") or by model id.
    return allOptions.filter((o) =>
      o.providerLabel.toLowerCase().includes(q) || o.model.toLowerCase().includes(q),
    );
  }

  function renderMenu(query) {
    menu.innerHTML = '';
    const matches = filteredOptions(query);

    if (allOptions.length === 0) {
      const empty = el('div', { class: 'subagent-picker__empty' },
        'No connected providers — connect one in AI Providers above.');
      menu.appendChild(empty);
      return;
    }
    if (matches.length === 0) {
      menu.appendChild(el('div', { class: 'subagent-picker__empty' }, 'No models match.'));
      return;
    }

    let lastProvider = null;
    matches.forEach((o, idx) => {
      // Provider headers act as section dividers when the list spans several
      // providers, matching the way the AI Providers section visually groups
      // models under each provider card.
      if (o.providerLabel !== lastProvider) {
        lastProvider = o.providerLabel;
        menu.appendChild(el('div', { class: 'subagent-picker__group' }, o.providerLabel));
      }
      const row = el('div', {
        class: 'subagent-picker__row' + (idx === highlightIdx ? ' subagent-picker__row--active' : ''),
      });
      row.dataset.idx = String(idx);
      row.appendChild(el('span', { class: 'subagent-picker__row-model' }, o.model));
      if (currentSelection
        && currentSelection.providerKey === o.providerKey
        && currentSelection.model === o.model) {
        row.classList.add('subagent-picker__row--selected');
        const check = icon('M5 12l5 5L20 7', 13);
        check.classList.add('subagent-picker__row-check');
        row.appendChild(check);
      }
      row.addEventListener('mouseenter', () => {
        highlightIdx = idx;
        updateHighlight();
      });
      row.addEventListener('mousedown', (e) => {
        // mousedown rather than click so the input doesn't lose focus and
        // re-trigger blur → close before the selection commits.
        e.preventDefault();
        commitSelection(o);
      });
      menu.appendChild(row);
    });
  }

  function updateHighlight() {
    const rows = menu.querySelectorAll('.subagent-picker__row');
    rows.forEach((r, i) => r.classList.toggle('subagent-picker__row--active', i === highlightIdx));
    if (highlightIdx >= 0 && rows[highlightIdx]) {
      rows[highlightIdx].scrollIntoView({ block: 'nearest' });
    }
  }

  async function commitSelection(opt) {
    setStatus('Saving…', '');
    try {
      await api.setSubagentConfig(opt.providerKey, opt.model);
      currentSelection = { providerKey: opt.providerKey, model: opt.model };
      saveLocal(currentSelection);
      input.value = formatSelected(currentSelection);
      closeMenu();
      setStatus(`Saved — sub-agents can pick "${opt.model}" for cheap/fast work.`, 'success');
    } catch (e) {
      setStatus(`Failed to save: ${e?.message || e}`, 'error');
    }
  }

  async function commitClear() {
    setStatus('Clearing…', '');
    try {
      await api.clearSubagentConfig();
      currentSelection = null;
      saveLocal(null);
      input.value = '';
      closeMenu();
      setStatus('Sub-agent override cleared — using main model.', '');
    } catch (e) {
      setStatus(`Failed to clear: ${e?.message || e}`, 'error');
    }
  }

  function refreshOptions() {
    allOptions = gatherOptions();
    // If the saved selection no longer matches an available model, surface
    // that so the user can fix it. Otherwise keep state silent.
    if (currentSelection) {
      const stillThere = allOptions.some(
        (o) => o.providerKey === currentSelection.providerKey && o.model === currentSelection.model,
      );
      if (!stillThere) {
        setStatus(
          `Saved sub-agent (${currentSelection.providerKey} — ${currentSelection.model}) is no ` +
          `longer available. Pick a different model or clear the override.`,
          'error',
        );
      }
    }
    if (!menuOpen) {
      input.value = formatSelected(currentSelection);
    } else {
      renderMenu(input.value);
    }
  }

  // ── Wiring ──────────────────────────────────────────────────────────
  input.addEventListener('focus', openMenu);
  input.addEventListener('click', openMenu);
  input.addEventListener('input', () => {
    if (!menuOpen) openMenu();
    highlightIdx = -1;
    renderMenu(input.value);
  });
  input.addEventListener('keydown', (e) => {
    const matches = filteredOptions(input.value);
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!menuOpen) openMenu();
      highlightIdx = Math.min(matches.length - 1, highlightIdx + 1);
      updateHighlight();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightIdx = Math.max(0, highlightIdx - 1);
      updateHighlight();
    } else if (e.key === 'Enter') {
      if (menuOpen && highlightIdx >= 0 && matches[highlightIdx]) {
        e.preventDefault();
        commitSelection(matches[highlightIdx]);
      }
    } else if (e.key === 'Escape') {
      closeMenu();
      input.blur();
    }
  });
  // Blur after a short delay so an in-progress mousedown on a row still wins.
  // The menu lives in <body>, so a hover-on-menu check needs to look at the
  // detached menu node directly — `pickerWrap.matches(':hover')` alone would
  // close the dropdown the moment the cursor moved into it.
  input.addEventListener('blur', () => {
    setTimeout(() => {
      const overPicker = pickerWrap.matches(':hover');
      const overMenu = menu.isConnected && menu.matches(':hover');
      if (!overPicker && !overMenu) closeMenu();
    }, 80);
  });

  clearBtn.addEventListener('click', commitClear);

  // Initial render uses the localStorage hint, then the backend overrides it.
  currentSelection = loadLocal();
  refreshOptions();

  api.getAiConfig()
    .then((cfg) => {
      const sub = cfg?.subagent;
      if (sub && sub.provider_key && sub.model) {
        currentSelection = { providerKey: sub.provider_key, model: sub.model };
        saveLocal(currentSelection);
      } else {
        currentSelection = null;
        saveLocal(null);
      }
      refreshOptions();
    })
    .catch((e) => console.warn('[subagent-settings] getAiConfig failed:', e));

  // Refresh whenever provider configs change so newly-connected providers
  // appear in the picker without a panel reopen.
  const onProviderChange = () => refreshOptions();
  window.addEventListener('rustic:provider-configs-changed', onProviderChange);

  return container;
}
