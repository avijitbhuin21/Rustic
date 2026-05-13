/**
 * P0.4 — Settings panel for cross-task budgets.
 *
 * Two knobs mapping to `rustic_agent::budget::BudgetSettings`:
 *   - **Max concurrent provider streams** — cap on parallel API calls
 *     across every task + their sub-agents. `null` disables.
 *   - **Daily cost ceiling (USD)** — hard limit on native-API spend per
 *     UTC day. Harness mode (Claude Code / Codex subscriptions) is
 *     shown separately and doesn't count against this. `null` disables.
 *
 * Sub-agent concurrency cap moved to the Sub Agent settings panel —
 * the user's mental model is that fan-out limits are a sub-agent
 * concern, not a cross-task budget. The backend preserves that field
 * here when this panel saves, so the two UIs don't fight.
 *
 * Layout follows the standard `settings-row` / `settings-row__info`
 * idiom: title + description on the left, compact control on the right.
 * Each row also carries an enable toggle; unchecking sends `null` so
 * the corresponding gate is off. Takes effect on the NEXT message —
 * running tasks use the Budget that was current at their `ToolContext`
 * build time.
 */

import { el } from '../../utils/dom.js';
import * as api from '../../lib/tauri-api.js';

const DEFAULT_MAX_STREAMS = 6;
const DEFAULT_DAILY_CEILING_USD = 20;

// Build one row matching the standard settings-row layout. Returns the
// row element plus handles to the checkbox + input so the caller can
// hydrate and read state without traversing the DOM.
function buildRow(label, description, prefix, suffix, defaultVal, min, step) {
  const row = el('div', { class: 'settings-row budget-settings__row' });
  const info = el('div', { class: 'settings-row__info' });
  info.appendChild(el('div', { class: 'settings-row__label' }, label));
  info.appendChild(el('div', { class: 'settings-row__desc' }, description));
  row.appendChild(info);

  const control = el('div', { class: 'budget-settings__control' });
  const toggleLabel = el('label', { class: 'budget-settings__toggle' });
  const check = el('input', { type: 'checkbox', class: 'budget-settings__check' });
  toggleLabel.appendChild(check);
  toggleLabel.appendChild(el('span', { class: 'budget-settings__toggle-track' }));
  control.appendChild(toggleLabel);

  if (prefix) {
    control.appendChild(el('span', { class: 'budget-settings__affix' }, prefix));
  }
  const input = el('input', {
    type: 'number',
    class: 'settings-input settings-input--number budget-settings__input',
    min: String(min),
    step: String(step),
    value: String(defaultVal),
  });
  control.appendChild(input);
  if (suffix) {
    control.appendChild(el('span', { class: 'budget-settings__affix budget-settings__affix--suffix' }, suffix));
  }
  row.appendChild(control);

  return { row, check, input };
}

export function createBudgetSettings() {
  const container = el('div', { class: 'budget-settings' });

  const desc = el('div', { class: 'budget-settings__intro' },
    'Cross-task limits. Stop runaway parallelism or spend before it bites. ' +
    'Harness tasks (Claude Code / Codex on a subscription) are shown ' +
    "separately and don't count against the daily ceiling.");
  container.appendChild(desc);

  const streams = buildRow(
    'Cap concurrent provider streams',
    'Parallel API calls across every task and their sub-agents. ' +
    `Default ${DEFAULT_MAX_STREAMS}. Raise only if your provider's rate limit can handle it.`,
    null, 'streams',
    DEFAULT_MAX_STREAMS, 1, 1,
  );
  container.appendChild(streams.row);

  const ceiling = buildRow(
    'Daily cost ceiling (native API)',
    'Stops new turns when today\'s native-API spend hits the cap. Resets at midnight UTC.',
    null, 'usd/day',
    DEFAULT_DAILY_CEILING_USD, 0, 0.01,
  );
  container.appendChild(ceiling.row);

  const footer = el('div', { class: 'budget-settings__footer' });
  const status = el('div', { class: 'budget-settings__status' });
  const saveBtn = el('button', { class: 'settings-btn budget-settings__save' }, 'Save budget settings');
  footer.appendChild(status);
  footer.appendChild(saveBtn);
  container.appendChild(footer);

  function syncDisabledState() {
    streams.input.disabled = !streams.check.checked;
    ceiling.input.disabled = !ceiling.check.checked;
    // Visual hint: dim the whole control group when its gate is off so
    // it's obvious the input value is inert.
    streams.row.classList.toggle('budget-settings__row--off', !streams.check.checked);
    ceiling.row.classList.toggle('budget-settings__row--off', !ceiling.check.checked);
  }

  streams.check.addEventListener('change', syncDisabledState);
  ceiling.check.addEventListener('change', syncDisabledState);

  async function load() {
    try {
      const s = await api.getBudgetSettings();
      const maxStreams = s?.max_concurrent_streams;
      const ceilingCents = s?.daily_cost_ceiling_cents;

      streams.check.checked = maxStreams != null;
      streams.input.value = String(maxStreams != null ? maxStreams : DEFAULT_MAX_STREAMS);

      ceiling.check.checked = ceilingCents != null;
      ceiling.input.value = ceilingCents != null
        ? (Number(ceilingCents) / 100).toFixed(2)
        : String(DEFAULT_DAILY_CEILING_USD);

      syncDisabledState();
    } catch (e) {
      status.textContent = `Couldn't load budget settings: ${e}`;
    }
  }

  saveBtn.addEventListener('click', async () => {
    saveBtn.disabled = true;
    status.textContent = 'Saving…';
    try {
      const maxStreams = streams.check.checked
        ? Math.max(1, parseInt(streams.input.value, 10) || DEFAULT_MAX_STREAMS)
        : null;
      const ceilingUsd = ceiling.check.checked
        ? Math.max(0, parseFloat(ceiling.input.value) || 0)
        : null;
      const ceilingCents = ceilingUsd == null ? null : Math.round(ceilingUsd * 100);

      await api.setBudgetSettings(maxStreams, ceilingCents);
      status.textContent = 'Saved. Takes effect on the next message.';
    } catch (e) {
      status.textContent = `Save failed: ${e}`;
    } finally {
      saveBtn.disabled = false;
    }
  });

  load();
  return container;
}
