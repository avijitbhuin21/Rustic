// First-run setup wizard. Walks the user through:
//   1. Welcome
//   2. Add a project (or skip → use Global)
//   3. Connect at least one AI provider
//   4. Brief tour-style "you're set" screen
//
// Triggered from main.js if `localStorage.rustic_onboarding_completed` is
// missing. Can also be re-launched manually via the `onboarding.show` command
// in the command palette so users can re-run it after dismissing.

import { el, icon } from '../../utils/dom.js';
import { workspaceStore, addProject } from '../../state/workspace.js';
import {
  hasAnyConnectedProvider,
  quickConnectProvider,
  loadProviderConfigs,
  saveProviderConfigs,
} from '../settings/ai-settings.js';
import { showToast } from '../toast.js';
import { trapFocus } from '../confirm-dialog.js';
import * as api from '../../lib/tauri-api.js';
import { createTerminal as createTerminalSession, terminalStore } from '../../state/terminal.js';

const STORAGE_KEY = 'rustic_onboarding_completed';

const PROVIDERS = [
  { id: 'Claude', label: 'Anthropic',     placeholder: 'sk-ant-…', helpUrl: 'https://console.anthropic.com/' },
  { id: 'OpenAi', label: 'OpenAI',        placeholder: 'sk-…',     helpUrl: 'https://platform.openai.com/api-keys' },
  { id: 'Gemini', label: 'Google Gemini', placeholder: 'AIza…',    helpUrl: 'https://aistudio.google.com/apikey' },
];

let activeWizard = null;

export function isOnboardingComplete() {
  try {
    return localStorage.getItem(STORAGE_KEY) === 'true';
  } catch {
    return false;
  }
}

function markOnboardingComplete() {
  try {
    localStorage.setItem(STORAGE_KEY, 'true');
  } catch {}
}

export function showOnboardingWizard({ force = false } = {}) {
  if (activeWizard) return;
  if (!force && isOnboardingComplete()) return;

  let stepIndex = 0;
  const steps = ['welcome', 'project', 'provider', 'done'];

  const overlay = el('div', { class: 'onboarding-overlay' });
  const dialog = el('div', {
    class: 'onboarding',
    role: 'dialog',
    'aria-modal': 'true',
    'aria-labelledby': 'onboarding-title',
  });

  const header = el('div', { class: 'onboarding__header' });
  const dots = el('div', { class: 'onboarding__dots' });
  const dotEls = steps.map((name) => {
    const d = el('span', { class: 'onboarding__dot', dataset: { step: name } });
    dots.appendChild(d);
    return d;
  });
  header.appendChild(dots);

  const skipBtn = el('button', { class: 'onboarding__skip' }, 'Skip setup');
  skipBtn.addEventListener('click', () => finish({ completed: true }));
  header.appendChild(skipBtn);

  dialog.appendChild(header);

  const body = el('div', { class: 'onboarding__body' });
  dialog.appendChild(body);

  const footer = el('div', { class: 'onboarding__footer' });
  const backBtn = el('button', { class: 'onboarding__btn onboarding__btn--ghost' }, 'Back');
  const nextBtn = el('button', { class: 'onboarding__btn onboarding__btn--primary' }, 'Continue');
  footer.appendChild(backBtn);
  footer.appendChild(nextBtn);
  dialog.appendChild(footer);

  backBtn.addEventListener('click', () => goTo(stepIndex - 1));
  nextBtn.addEventListener('click', () => goTo(stepIndex + 1));

  function render() {
    body.innerHTML = '';
    dotEls.forEach((d, i) => {
      d.classList.toggle('onboarding__dot--active', i === stepIndex);
      d.classList.toggle('onboarding__dot--past', i < stepIndex);
    });
    backBtn.style.visibility = stepIndex === 0 ? 'hidden' : '';
    nextBtn.disabled = false;
    nextBtn.textContent = stepIndex === steps.length - 1 ? 'Start using Rustic' : 'Continue';

    const step = steps[stepIndex];
    if (step === 'welcome') renderWelcome();
    else if (step === 'project') renderProject();
    else if (step === 'provider') renderProvider();
    else if (step === 'done') renderDone();
  }

  function goTo(target) {
    if (target < 0 || target >= steps.length) {
      if (target >= steps.length) finish({ completed: true });
      return;
    }
    stepIndex = target;
    render();
  }

  function renderWelcome() {
    const wrap = el('div', { class: 'onboarding__step onboarding__step--welcome' });
    wrap.appendChild(el('h1', { class: 'onboarding__title', id: 'onboarding-title' },
      'Welcome to Rustic'));
    wrap.appendChild(el('p', { class: 'onboarding__lede' },
      'A VS Code-inspired IDE with a built-in AI agent. Three quick steps and you\'re ready.'));

    const tips = el('ul', { class: 'onboarding__tips' });
    const tip = (label) => {
      const li = el('li', {}, [
        icon('M5 13l4 4L19 7', 14),
        el('span', {}, label),
      ]);
      tips.appendChild(li);
    };
    tip('Open one or more project folders');
    tip('Connect at least one AI provider (Anthropic, OpenAI, or Gemini)');
    tip('Press Ctrl+P to open files, Ctrl+Shift+P for commands');
    wrap.appendChild(tips);

    body.appendChild(wrap);
  }

  function renderProject() {
    const wrap = el('div', { class: 'onboarding__step' });
    wrap.appendChild(el('h2', { class: 'onboarding__title' }, 'Open a project'));
    wrap.appendChild(el('p', { class: 'onboarding__lede' },
      'Rustic works on folders. Pick one to get started, or skip and use the Global scope (no project context).'));

    const list = el('div', { class: 'onboarding__project-list' });
    const renderList = () => {
      list.innerHTML = '';
      const projects = (workspaceStore.getState('projects') || [])
        .filter((p) => p.id !== '__global__');
      if (projects.length === 0) {
        list.appendChild(el('div', { class: 'onboarding__empty' },
          'No projects yet. Pick a folder below or skip for now.'));
        return;
      }
      for (const p of projects) {
        const row = el('div', { class: 'onboarding__project-row' });
        row.appendChild(icon('M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z', 14));
        row.appendChild(el('div', { class: 'onboarding__project-name' }, p.name || p.root_path));
        row.appendChild(el('div', { class: 'onboarding__project-path' }, p.root_path));
        list.appendChild(row);
      }
    };
    renderList();
    wrap.appendChild(list);

    const actions = el('div', { class: 'onboarding__actions' });

    const pickBtn = el('button', { class: 'onboarding__btn onboarding__btn--secondary' });
    pickBtn.appendChild(icon('M12 4v16m8-8H4', 14));
    pickBtn.appendChild(el('span', {}, 'Choose folder…'));
    pickBtn.addEventListener('click', async () => {
      pickBtn.disabled = true;
      try {
        const project = await addProject();
        if (project) {
          renderList();
          showToast(`Added ${project.name || project.root_path}`, { kind: 'success' });
        }
      } catch (e) {
        showToast(`Failed to add project: ${e.message || e}`, { kind: 'error' });
      } finally {
        pickBtn.disabled = false;
      }
    });
    actions.appendChild(pickBtn);

    const hint = el('div', { class: 'onboarding__inline-hint' },
      'Tip: you can always add more projects later from the Explorer panel.');
    actions.appendChild(hint);
    wrap.appendChild(actions);

    body.appendChild(wrap);

    // Continue is enabled if at least one project exists, or the user clicks
    // the "Skip — use Global" path (which still progresses but doesn't add).
    const projects = (workspaceStore.getState('projects') || [])
      .filter((p) => p.id !== '__global__');
    nextBtn.textContent = projects.length === 0 ? 'Skip — use Global' : 'Continue';

    // Re-evaluate the next-button label whenever the project list changes
    // while this step is visible.
    const sub = workspaceStore.subscribe('projects', () => {
      if (steps[stepIndex] !== 'project') return;
      renderList();
      const ps = (workspaceStore.getState('projects') || [])
        .filter((p) => p.id !== '__global__');
      nextBtn.textContent = ps.length === 0 ? 'Skip — use Global' : 'Continue';
    });
    body._cleanupSub = sub;
  }

  function renderProvider() {
    const wrap = el('div', { class: 'onboarding__step' });
    wrap.appendChild(el('h2', { class: 'onboarding__title' }, 'Connect an AI provider'));
    wrap.appendChild(el('p', { class: 'onboarding__lede' },
      'Add an API key for at least one provider. Keys are stored in your OS keychain — never on disk in plaintext.'));

    const grid = el('div', { class: 'onboarding__providers' });
    for (const p of PROVIDERS) {
      grid.appendChild(buildProviderCard(p));
    }
    wrap.appendChild(grid);

    // Subscription-mode providers (Claude Pro / Max via the `claude` CLI).
    // No API key — the user signs in to the CLI itself and Rustic just spawns
    // it. Mirrors the Settings → AI Providers Subscriptions card (plan §B.4)
    // so first-run users discover this path even if they never visit Settings.
    wrap.appendChild(el('div', { class: 'onboarding__divider' }));
    wrap.appendChild(el('div', { class: 'onboarding__subsection-title' }, 'Use a subscription instead'));
    wrap.appendChild(el('p', { class: 'onboarding__sub-hint' },
      'Have a Claude Pro or Max plan? Sign in once with the `claude` CLI and Rustic will spawn it for you — no API key required, billing flows through your subscription.'));
    const subsGrid = el('div', { class: 'onboarding__providers' });
    subsGrid.appendChild(buildSubscriptionCard({
      storageKey: 'ClaudeCode',
      label: 'Claude Code',
      placeholderModel: 'claude-code',
      cliCommand: 'claude',
    }));
    // Codex (ChatGPT subscription) — same Sign in / Enable flow, drives
    // `codex app-server` over JSON-RPC instead of NDJSON. Plan §B.10.
    subsGrid.appendChild(buildSubscriptionCard({
      storageKey: 'Codex',
      label: 'Codex',
      placeholderModel: 'codex',
      cliCommand: 'codex login',
    }));
    wrap.appendChild(subsGrid);

    wrap.appendChild(el('p', { class: 'onboarding__sub-hint' }, [
      'Need an OpenAI-compatible endpoint (Ollama, Groq, OpenRouter)? ',
      el('span', { class: 'onboarding__hint-strong' }, 'Add it later from Settings → Agent.'),
    ]));

    body.appendChild(wrap);

    // Continue is always enabled: we let the user "Skip" provider setup, but
    // we relabel the button so it's clear what they're doing.
    function refreshNextLabel() {
      nextBtn.textContent = hasAnyConnectedProvider() ? 'Continue' : 'Skip for now';
    }
    refreshNextLabel();
    const handler = () => refreshNextLabel();
    window.addEventListener('rustic:provider-configs-changed', handler);
    body._cleanupListener = () => {
      window.removeEventListener('rustic:provider-configs-changed', handler);
    };
  }

  function buildProviderCard(meta) {
    const card = el('div', { class: 'onboarding__provider' });

    const head = el('div', { class: 'onboarding__provider-head' });
    head.appendChild(el('div', { class: 'onboarding__provider-name' }, meta.label));
    const status = el('div', { class: 'onboarding__provider-status' });
    head.appendChild(status);
    card.appendChild(head);

    const inputRow = el('div', { class: 'onboarding__provider-input-row' });
    const input = el('input', {
      type: 'password',
      class: 'onboarding__provider-input',
      placeholder: meta.placeholder,
      autocomplete: 'off',
      spellcheck: 'false',
    });
    const btn = el('button', { class: 'onboarding__btn onboarding__btn--inline' }, 'Connect');
    inputRow.appendChild(input);
    inputRow.appendChild(btn);
    card.appendChild(inputRow);

    const help = el('div', { class: 'onboarding__provider-help' });
    const helpLink = el('a', {
      href: meta.helpUrl,
      target: '_blank',
      rel: 'noopener noreferrer',
    }, 'Get an API key →');
    help.appendChild(helpLink);
    card.appendChild(help);

    function refreshStatus() {
      const cfg = loadProviderConfigs()[meta.id];
      if (cfg?.hasKey && cfg.models?.length) {
        status.innerHTML = '';
        status.classList.add('onboarding__provider-status--ok');
        const checkIcon = icon('M5 13l4 4L19 7', 12);
        status.appendChild(checkIcon);
        status.appendChild(el('span', {}, `${cfg.models.length} model${cfg.models.length === 1 ? '' : 's'}`));
        input.placeholder = 'Connected — replace key to change';
        input.value = '';
        btn.textContent = 'Replace key';
      } else {
        status.innerHTML = '';
        status.classList.remove('onboarding__provider-status--ok');
        btn.textContent = 'Connect';
      }
    }
    refreshStatus();

    btn.addEventListener('click', async () => {
      const apiKey = input.value.trim();
      if (!apiKey) {
        status.innerHTML = '';
        status.classList.remove('onboarding__provider-status--ok');
        status.appendChild(el('span', { class: 'onboarding__provider-error' },
          'Enter an API key first'));
        input.focus();
        return;
      }
      btn.disabled = true;
      status.innerHTML = '';
      status.classList.remove('onboarding__provider-status--ok');
      status.appendChild(el('span', { class: 'onboarding__provider-pending' }, 'Connecting…'));
      try {
        const { models } = await quickConnectProvider(meta.id, apiKey);
        showToast(`Connected ${meta.label} — ${models.length} model${models.length === 1 ? '' : 's'} available`, { kind: 'success' });
        refreshStatus();
      } catch (e) {
        status.innerHTML = '';
        status.classList.remove('onboarding__provider-status--ok');
        const msg = (e && e.message) ? e.message : String(e || 'Connection failed');
        status.appendChild(el('span', { class: 'onboarding__provider-error' }, msg));
      } finally {
        btn.disabled = false;
      }
    });

    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        btn.click();
      }
    });

    return card;
  }

  /// Card for a harness-backed (subscription) provider in the onboarding
  /// wizard. Visually parallels the API-key card (`buildProviderCard`) but
  /// the action is "Sign in" rather than "Connect" — we open a terminal in
  /// the bottom panel pre-filled with the CLI's auth command and re-probe
  /// when the terminal closes (plan §B.4).
  function buildSubscriptionCard({ storageKey, label, placeholderModel, cliCommand }) {
    const card = el('div', { class: 'onboarding__provider' });

    const head = el('div', { class: 'onboarding__provider-head' });
    head.appendChild(el('div', { class: 'onboarding__provider-name' }, label));
    const status = el('div', { class: 'onboarding__provider-status' });
    head.appendChild(status);
    card.appendChild(head);

    const help = el('div', { class: 'onboarding__provider-help' });
    help.appendChild(el('span', {}, [
      'Run ',
      el('code', { class: 'onboarding__inline-code' }, cliCommand),
      ' once to sign in. Then come back here.',
    ]));
    card.appendChild(help);

    const buttonRow = el('div', { class: 'onboarding__provider-input-row' });
    const signInBtn = el('button', { class: 'onboarding__btn onboarding__btn--inline' }, 'Sign in');
    const enableBtn = el('button', { class: 'onboarding__btn onboarding__btn--inline', style: 'display:none;' }, 'Enable');
    const recheckBtn = el('button', {
      class: 'onboarding__btn onboarding__btn--inline',
      title: 'Re-run the install + signin probe.',
    }, 'Re-check');
    buttonRow.appendChild(signInBtn);
    buttonRow.appendChild(enableBtn);
    buttonRow.appendChild(recheckBtn);
    card.appendChild(buttonRow);

    let lastProbe = null;

    function refreshStatus() {
      const cfg = loadProviderConfigs()[storageKey];
      const enabled = !!cfg?.hasKey;

      // Hide Sign in once authenticated; show Enable when ready to register.
      // Hide Enable when already enabled (no need to re-register).
      let probeText;
      let canEnable = false;
      if (!lastProbe) {
        probeText = 'Probing…';
      } else {
        switch (lastProbe.status) {
          case 'authenticated':
            probeText = `Installed & signed in${lastProbe.version ? ` (${lastProbe.version})` : ''}.`;
            canEnable = true;
            break;
          case 'not_authenticated':
            probeText = 'Installed but not signed in.';
            break;
          case 'not_installed':
            probeText = 'CLI not found on PATH. Install Claude Code first.';
            break;
          case 'probe_failed':
            probeText = `Probe failed: ${lastProbe.detail || 'unknown error'}.`;
            break;
          default:
            probeText = 'Unknown probe result.';
        }
      }

      status.innerHTML = '';
      status.classList.toggle('onboarding__provider-status--ok', enabled);
      if (enabled) {
        status.appendChild(icon('M5 13l4 4L19 7', 12));
        status.appendChild(el('span', {}, 'Enabled'));
        signInBtn.style.display = 'none';
        enableBtn.style.display = 'none';
      } else if (canEnable) {
        status.appendChild(el('span', {}, probeText));
        signInBtn.style.display = 'none';
        enableBtn.style.display = '';
      } else {
        status.appendChild(el('span', { class: lastProbe && lastProbe.status !== 'authenticated' ? 'onboarding__provider-pending' : '' }, probeText));
        signInBtn.style.display = '';
        enableBtn.style.display = 'none';
        // The Sign in button is the primary action when the CLI is installed
        // but not signed in. When the CLI itself is missing, the same button
        // would just spawn a shell and fail — relabel as "Install help" so
        // the user knows clicking won't magically install anything.
        signInBtn.textContent = lastProbe?.status === 'not_installed' ? 'How to install' : 'Sign in';
        signInBtn.disabled = lastProbe?.status === 'probe_failed';
      }
    }

    async function probe() {
      recheckBtn.disabled = true;
      try {
        lastProbe = await api.probeHarnessAuth(storageKey, null);
      } catch (err) {
        lastProbe = { status: 'probe_failed', detail: err?.message || String(err) };
      } finally {
        recheckBtn.disabled = false;
        refreshStatus();
      }
    }

    recheckBtn.addEventListener('click', probe);

    signInBtn.addEventListener('click', async () => {
      // Not-installed path: link to the install docs in a new tab. We don't
      // know the user's package manager so we just point at the official
      // install page for whichever CLI this card represents.
      if (lastProbe?.status === 'not_installed') {
        const installUrl = storageKey === 'Codex'
          ? 'https://developers.openai.com/codex/cli/'
          : 'https://docs.claude.com/en/docs/claude-code/quickstart';
        try {
          window.open(installUrl, '_blank', 'noopener');
        } catch {}
        return;
      }

      signInBtn.disabled = true;
      const oldLabel = signInBtn.textContent;
      signInBtn.textContent = 'Opening terminal…';
      try {
        const term = await createTerminalSession(null, `Sign in: ${label}`);
        if (!term) throw new Error('Could not open a terminal.');
        // Give the shell a beat to print its prompt before we type into it,
        // otherwise the command appears above the prompt and looks awkward.
        await new Promise((r) => setTimeout(r, 250));
        try {
          await api.writeTerminal(term.id, `${cliCommand}\n`);
        } catch (e) {
          // Non-fatal: the terminal is still open and the user can type the
          // command themselves. Just surface a hint.
          showToast(`Terminal opened — type \`${cliCommand}\` to begin.`, { kind: 'info' });
          console.warn('writeTerminal failed', e);
        }
        showToast('Sign in via the terminal, then come back. Detection will refresh automatically.', { kind: 'info' });

        // Watch for this session disappearing from the terminal store — that
        // means the user closed the tab (typically after the CLI exits its
        // login flow). Re-probe at that point so the row updates without a
        // manual click.
        const sub = terminalStore.subscribe('sessions', (sessions) => {
          if (!sessions.some((s) => s.id === term.id)) {
            sub();
            // Status check runs out-of-band so the unsubscribe is final.
            probe();
          }
        });
        // Stash for cleanup so navigating away from the step doesn't leak.
        const prev = body._cleanupTerminalSub;
        body._cleanupTerminalSub = () => {
          if (prev) try { prev(); } catch {}
          try { sub(); } catch {}
        };
      } catch (e) {
        status.innerHTML = '';
        status.appendChild(el('span', { class: 'onboarding__provider-error' }, `Could not open terminal: ${e?.message || e}`));
      } finally {
        signInBtn.textContent = oldLabel;
        signInBtn.disabled = false;
      }
    });

    enableBtn.addEventListener('click', async () => {
      enableBtn.disabled = true;
      const oldLabel = enableBtn.textContent;
      enableBtn.textContent = 'Enabling…';
      try {
        // Re-probe right before enable in case auth state changed since
        // the cached result.
        lastProbe = await api.probeHarnessAuth(storageKey, null);
        if (lastProbe.status !== 'authenticated') {
          refreshStatus();
          return;
        }
        await api.setAiProvider(
          storageKey, '', placeholderModel, null, null,
          0, 0, 0, 0, 0,
          null, null, label,
        );
        const configs = loadProviderConfigs();
        configs[storageKey] = {
          hasKey: true,
          model: placeholderModel,
          models: [placeholderModel],
          baseUrl: null,
          name: label,
        };
        saveProviderConfigs(configs);
        showToast(`Enabled ${label}.`, { kind: 'success' });
        // Notify the wizard's footer to re-evaluate Continue/Skip label.
        try {
          window.dispatchEvent(new Event('rustic:provider-configs-changed'));
        } catch {}
      } catch (err) {
        showToast(`Failed to enable: ${err?.message || err}`, { kind: 'error' });
      } finally {
        enableBtn.textContent = oldLabel;
        enableBtn.disabled = false;
        refreshStatus();
      }
    });

    refreshStatus();
    probe(); // fire-and-forget — refreshStatus runs again on completion.

    return card;
  }

  function renderDone() {
    const wrap = el('div', { class: 'onboarding__step onboarding__step--done' });

    const checkWrap = el('div', { class: 'onboarding__check' });
    checkWrap.appendChild(icon('M5 13l4 4L19 7', 28));
    wrap.appendChild(checkWrap);

    wrap.appendChild(el('h2', { class: 'onboarding__title' }, 'You\'re set'));
    wrap.appendChild(el('p', { class: 'onboarding__lede' },
      'A few shortcuts to keep on hand:'));

    const tips = el('ul', { class: 'onboarding__tips' });
    const tip = (kbd, label) => {
      const li = el('li', {});
      li.appendChild(el('kbd', { class: 'onboarding__kbd' }, kbd));
      li.appendChild(el('span', {}, label));
      tips.appendChild(li);
    };
    tip('Ctrl+P', 'Quick-open a file');
    tip('Ctrl+Shift+P', 'Command palette — every action lives here');
    tip('Ctrl+`', 'Toggle the integrated terminal');
    tip('Ctrl+,', 'Open settings (model, theme, API keys, shortcuts)');
    wrap.appendChild(tips);

    wrap.appendChild(el('p', { class: 'onboarding__sub-hint' },
      'You can re-run this wizard any time from the command palette: "Run setup wizard".'));

    body.appendChild(wrap);
  }

  function finish({ completed }) {
    if (completed) markOnboardingComplete();
    if (body._cleanupSub) body._cleanupSub();
    if (body._cleanupListener) body._cleanupListener();
    if (body._cleanupTerminalSub) body._cleanupTerminalSub();
    if (releaseTrap) releaseTrap();
    document.removeEventListener('keydown', onKey);
    overlay.remove();
    activeWizard = null;
  }

  function onKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      // Esc closes but does NOT mark complete — we re-show next launch unless
      // the user explicitly skipped via the header button.
      finish({ completed: false });
    } else if (e.key === 'Enter' && document.activeElement?.tagName !== 'INPUT') {
      e.preventDefault();
      goTo(stepIndex + 1);
    }
  }

  overlay.appendChild(dialog);
  document.body.appendChild(overlay);
  document.addEventListener('keydown', onKey);
  const releaseTrap = trapFocus(dialog);
  activeWizard = { close: () => finish({ completed: false }) };

  render();
  return activeWizard;
}
