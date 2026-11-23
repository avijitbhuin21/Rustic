// The live theme preview: a mock Rustic window that repaints for whatever theme
// its host page sends. Runs in an iframe (see host.js); never applies a
// theme's custom CSS.
import React from 'react';
import ReactDOM from 'react-dom/client';
import '../styles/globals.css';
import { applyActiveTheme } from '@/lib/theme-apply';
import { sanitizePack, packToActive, packModes, obsidianPack } from '@/lib/theme-pack';
import { HOST_SOURCE, PREVIEW_SOURCE, SCENES } from './host';
import { MockWindow } from './mock-window';

const embedded = window.parent !== window;

/** Tell the host page something; only a same-origin parent can read it. */
function tellHost(message) {
  if (embedded) window.parent.postMessage({ source: PREVIEW_SOURCE, ...message }, window.location.origin);
}

/** Paint `raw` (a theme file object) and report what was ignored. Returns nothing. */
function paint(raw, mode) {
  const { pack, problems } = sanitizePack(raw);
  const active = packToActive(pack, mode === 'light' ? 'light' : 'dark');
  applyActiveTheme(active, false);
  tellHost({ type: 'problems', problems, mode: active.mode, modes: packModes(pack) });
}

function Preview() {
  const [scene, setScene] = React.useState(() => {
    const wanted = new URLSearchParams(window.location.search).get('scene');
    return SCENES.includes(wanted) ? wanted : 'workbench';
  });
  // Bumped after every repaint so parts that read computed colors re-read them.
  const [paints, setPaints] = React.useState(0);

  React.useEffect(() => {
    const onMessage = (e) => {
      if (e.source !== window.parent || e.origin !== window.location.origin) return;
      const data = e.data;
      if (!data || data.source !== HOST_SOURCE) return;
      if (data.type === 'theme') {
        paint(data.pack, data.mode);
        setPaints((n) => n + 1);
      } else if (data.type === 'scene' && SCENES.includes(data.scene)) {
        setScene(data.scene);
      }
    };
    if (embedded) {
      window.addEventListener('message', onMessage);
      tellHost({ type: 'ready' });
      return () => window.removeEventListener('message', onMessage);
    }
    // Opened directly: show the default theme.
    paint(obsidianPack(), 'dark');
    setPaints(1);
    return undefined;
  }, []);

  if (paints === 0) return null;
  return <MockWindow scene={scene} paints={paints} />;
}

ReactDOM.createRoot(document.getElementById('root')).render(<Preview />);
