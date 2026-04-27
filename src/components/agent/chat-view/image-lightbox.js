// Full-screen image lightbox used when the user clicks an inline image in
// the chat. Shows a backdrop overlay with the image; right-click on the
// image opens a small "Copy image" menu (re-encoded as PNG for clipboard
// compatibility — Chromium's ClipboardItem only reliably accepts PNG).

/** Re-encode any image blob as PNG via a canvas, for clipboard compatibility. */
function reencodeAsPng(blob) {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(blob);
    const im = new Image();
    im.onload = () => {
      const c = document.createElement('canvas');
      c.width = im.naturalWidth; c.height = im.naturalHeight;
      c.getContext('2d').drawImage(im, 0, 0);
      c.toBlob((b) => { URL.revokeObjectURL(url); b ? resolve(b) : reject(new Error('toBlob failed')); }, 'image/png');
    };
    im.onerror = (e) => { URL.revokeObjectURL(url); reject(e); };
    im.src = url;
  });
}

export function openImageLightbox(src) {
  document.getElementById('chat-image-lightbox')?.remove();

  const overlay = document.createElement('div');
  overlay.id = 'chat-image-lightbox';
  overlay.className = 'chat-lightbox';

  const img = document.createElement('img');
  img.className = 'chat-lightbox__img';
  img.src = src;
  overlay.appendChild(img);

  let menu = null;
  function closeMenu() {
    if (menu) { menu.remove(); menu = null; }
  }

  function close() {
    closeMenu();
    overlay.remove();
    document.removeEventListener('keydown', onKey);
  }

  function onKey(e) {
    if (e.key === 'Escape') { if (menu) closeMenu(); else close(); }
  }

  async function copyImage() {
    try {
      const res = await fetch(src);
      const blob = await res.blob();
      const pngBlob = blob.type === 'image/png' ? blob : await reencodeAsPng(blob);
      await navigator.clipboard.write([new ClipboardItem({ 'image/png': pngBlob })]);
    } catch (err) {
      console.error('Failed to copy image:', err);
    }
  }

  img.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    closeMenu();
    menu = document.createElement('div');
    menu.className = 'chat-lightbox__menu';

    const vw = window.innerWidth, vh = window.innerHeight;
    const left = Math.min(e.clientX, vw - 160);
    const top = Math.min(e.clientY, vh - 50);
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;

    const item = document.createElement('div');
    item.className = 'chat-lightbox__menu-item';
    item.textContent = 'Copy image';
    item.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      closeMenu();
      await copyImage();
    });
    menu.appendChild(item);
    document.body.appendChild(menu);
  });

  overlay.addEventListener('click', (e) => {
    if (menu) { closeMenu(); return; }
    if (e.target === overlay) close();
  });
  overlay.addEventListener('contextmenu', (e) => {
    if (e.target === overlay) { e.preventDefault(); closeMenu(); }
  });
  document.addEventListener('keydown', onKey);

  document.body.appendChild(overlay);
}
