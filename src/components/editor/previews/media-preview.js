import { el } from '../../../utils/dom.js';
import * as api from '../../../lib/tauri-api.js';
import { getMimeType } from '../../../utils/file-types.js';

/**
 * Video and audio preview component.
 */
export function createMediaPreview() {
  const container = el('div', { class: 'preview-container media-preview' });
  const info = el('div', { class: 'preview-info' });

  let mediaEl = null;
  let currentType = null; // 'video' or 'audio'

  async function load(path, fileType) {
    container.innerHTML = '';
    currentType = fileType;

    const mime = getMimeType(path);

    try {
      const result = await api.readFileBase64(path);
      const dataUrl = `data:${mime};base64,${result.data}`;

      if (fileType === 'video') {
        mediaEl = el('video', {
          class: 'media-preview-player',
          controls: 'true',
          preload: 'metadata',
        });
        mediaEl.src = dataUrl;
      } else {
        // Audio — show a nice centered player
        const audioWrap = el('div', { class: 'audio-preview-wrap' });
        const audioIcon = el('div', { class: 'audio-preview-icon' }, '\u266b');
        mediaEl = el('audio', {
          class: 'media-preview-audio',
          controls: 'true',
          preload: 'metadata',
        });
        mediaEl.src = dataUrl;
        audioWrap.appendChild(audioIcon);
        audioWrap.appendChild(mediaEl);
        container.appendChild(audioWrap);
      }

      if (fileType === 'video') {
        container.appendChild(mediaEl);
      }

      info.textContent = formatSize(result.size);
      container.appendChild(info);

      // Update info when metadata loads
      mediaEl.addEventListener('loadedmetadata', () => {
        const duration = formatDuration(mediaEl.duration);
        let details = `${duration}  \u2022  ${formatSize(result.size)}`;
        if (fileType === 'video' && mediaEl.videoWidth) {
          details = `${mediaEl.videoWidth} \u00d7 ${mediaEl.videoHeight}  \u2022  ${details}`;
        }
        info.textContent = details;
      });
    } catch (e) {
      info.textContent = `Failed to load media: ${e}`;
      container.appendChild(info);
    }
  }

  function destroy() {
    if (mediaEl) {
      mediaEl.pause();
      mediaEl.src = '';
      mediaEl = null;
    }
  }

  return { element: container, load, destroy };
}

function formatSize(bytes) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function formatDuration(seconds) {
  if (!isFinite(seconds)) return '';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
  return `${m}:${String(s).padStart(2, '0')}`;
}
