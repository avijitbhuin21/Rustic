import { el } from '../../utils/dom.js';
import { updateSetting } from '../../state/settings.js';
import * as api from '../../lib/tauri-api.js';

const providers = [
  { id: 'Claude', label: 'Anthropic Claude', models: ['claude-sonnet-4-20250514', 'claude-opus-4-20250514', 'claude-haiku-4-20250307'] },
  { id: 'OpenAi', label: 'OpenAI', models: ['gpt-4o', 'gpt-4o-mini', 'o3-mini'] },
  { id: 'Gemini', label: 'Google Gemini', models: ['gemini-2.5-pro', 'gemini-2.5-flash'] },
  { id: 'Compatible', label: 'OpenAI-Compatible', models: [] },
];

export function createAiSettings(settings) {
  const container = el('div', { class: 'settings-section' });
  container.appendChild(el('h3', { class: 'settings-section__title' }, 'AI Providers'));

  // Default provider
  const providerRow = el('div', { class: 'settings-row' });
  const providerInfo = el('div', { class: 'settings-row__info' });
  providerInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Default Provider'));
  providerInfo.appendChild(el('div', { class: 'settings-row__desc' }, 'Which AI provider to use by default'));
  providerRow.appendChild(providerInfo);

  const providerSelect = el('select', { class: 'settings-select' });
  providerSelect.appendChild(el('option', { value: '' }, '-- None --'));
  for (const p of providers) {
    const opt = el('option', { value: p.id }, p.label);
    if (p.id === settings.ai.default_provider) opt.selected = true;
    providerSelect.appendChild(opt);
  }
  providerSelect.addEventListener('change', () => {
    updateSetting('ai.default_provider', providerSelect.value || null);
  });
  providerRow.appendChild(providerSelect);
  container.appendChild(providerRow);

  // Temperature
  const tempRow = el('div', { class: 'settings-row' });
  const tempInfo = el('div', { class: 'settings-row__info' });
  tempInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Temperature'));
  tempInfo.appendChild(el('div', { class: 'settings-row__desc' }, 'Controls randomness (0.0 = deterministic, 1.0 = creative)'));
  tempRow.appendChild(tempInfo);

  const tempWrap = el('div', { class: 'settings-slider-wrap' });
  const tempSlider = el('input', {
    class: 'settings-slider',
    type: 'range', min: '0', max: '1', step: '0.05',
    value: String(settings.ai.temperature),
  });
  const tempValue = el('span', { class: 'settings-slider__value' }, String(settings.ai.temperature));
  tempSlider.addEventListener('input', () => {
    tempValue.textContent = tempSlider.value;
  });
  tempSlider.addEventListener('change', () => {
    updateSetting('ai.temperature', parseFloat(tempSlider.value));
  });
  tempWrap.appendChild(tempSlider);
  tempWrap.appendChild(tempValue);
  tempRow.appendChild(tempWrap);
  container.appendChild(tempRow);

  // Max tokens
  const tokensRow = el('div', { class: 'settings-row' });
  const tokensInfo = el('div', { class: 'settings-row__info' });
  tokensInfo.appendChild(el('div', { class: 'settings-row__label' }, 'Max Tokens'));
  tokensInfo.appendChild(el('div', { class: 'settings-row__desc' }, 'Maximum output tokens per response'));
  tokensRow.appendChild(tokensInfo);

  const tokensInput = el('input', {
    class: 'settings-input settings-input--number',
    type: 'number', value: String(settings.ai.max_tokens),
    min: '256', max: '32000', step: '256',
  });
  tokensInput.addEventListener('change', () => {
    updateSetting('ai.max_tokens', parseInt(tokensInput.value, 10));
  });
  tokensRow.appendChild(tokensInput);
  container.appendChild(tokensRow);

  // Per-provider API key configuration
  container.appendChild(el('h4', { class: 'settings-subsection-title' }, 'Provider Configuration'));

  for (const p of providers) {
    const section = el('div', { class: 'settings-provider' });
    section.appendChild(el('div', { class: 'settings-provider__name' }, p.label));

    // API Key
    const keyRow = el('div', { class: 'settings-row settings-row--compact' });
    keyRow.appendChild(el('div', { class: 'settings-row__label' }, 'API Key'));
    const keyInput = el('input', {
      class: 'settings-input', type: 'password',
      placeholder: 'Enter API key...',
    });
    keyRow.appendChild(keyInput);
    section.appendChild(keyRow);

    // Model
    const modelRow = el('div', { class: 'settings-row settings-row--compact' });
    modelRow.appendChild(el('div', { class: 'settings-row__label' }, 'Model'));
    if (p.models.length > 0) {
      const modelSelect = el('select', { class: 'settings-select' });
      for (const m of p.models) {
        modelSelect.appendChild(el('option', { value: m }, m));
      }
      modelRow.appendChild(modelSelect);
    } else {
      modelRow.appendChild(el('input', { class: 'settings-input', type: 'text', placeholder: 'Model name' }));
    }
    section.appendChild(modelRow);

    // Base URL (for compatible)
    if (p.id === 'Compatible') {
      const urlRow = el('div', { class: 'settings-row settings-row--compact' });
      urlRow.appendChild(el('div', { class: 'settings-row__label' }, 'Base URL'));
      urlRow.appendChild(el('input', { class: 'settings-input', type: 'text', placeholder: 'https://api.example.com/v1' }));
      section.appendChild(urlRow);
    }

    // Save button
    const saveBtn = el('button', { class: 'settings-btn settings-btn--small' }, 'Save Provider');
    saveBtn.addEventListener('click', async () => {
      const key = section.querySelector('input[type="password"]').value;
      const model = section.querySelector('select')?.value || section.querySelectorAll('input[type="text"]')[0]?.value || p.models[0] || '';
      const baseUrl = p.id === 'Compatible' ? section.querySelectorAll('input[type="text"]')[1]?.value || null : null;

      if (!key) return;
      try {
        await api.setAiProvider(p.id, key, model, baseUrl);
        saveBtn.textContent = 'Saved!';
        setTimeout(() => { saveBtn.textContent = 'Save Provider'; }, 1500);
      } catch (e) {
        console.error('Failed to save provider:', e);
      }
    });
    section.appendChild(saveBtn);

    container.appendChild(section);
  }

  return container;
}
