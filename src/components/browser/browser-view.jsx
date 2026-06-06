import React, { useEffect, useRef } from 'react';
import { cdpWsUrl } from '@/lib/browser-cdp';

// The page viewport: opens a CDP WebSocket to the active tab, draws
// `Page.screencastFrame` JPEGs onto a <canvas>, and forwards mouse/keyboard/
// wheel input back as `Input.dispatch*Event`.
//
// `device` (null = desktop/fit-container) drives real device emulation on the
// live page: when set, the viewport renders at the device's CSS size + DPR +
// touch, exactly like Chrome's device toolbar — and the canvas is centered and
// scaled-to-fit so you see the emulated screen. Input coords are mapped to the
// emulated CSS viewport so clicks land correctly at any scale.

const CDP_MODS = (e) =>
  (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);

const MOUSE_BUTTON = ['left', 'middle', 'right', 'back', 'forward'];

export function BrowserView({ targetId, device = null }) {
  const containerRef = useRef(null);
  const canvasRef = useRef(null);
  const cmdId = useRef(1);
  // The emulated viewport size in CSS px (what we passed to
  // setDeviceMetricsOverride). Input coords map against this, not the canvas
  // backing pixels (which include DPR), so clicks are correct under emulation.
  const viewportRef = useRef({ w: 0, h: 0 });
  const deviceRef = useRef(device);
  deviceRef.current = device;
  // Latest applySize, so the device-change effect can re-apply without tearing
  // down and reopening the screencast socket.
  const applyRef = useRef(null);

  useEffect(() => {
    if (!targetId) return;
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return;

    let closed = false;
    let ws = null;
    let reconnectTimer = 0;

    const send = (method, params) => {
      if (!ws || ws.readyState !== WebSocket.OPEN) return;
      ws.send(JSON.stringify({ id: cmdId.current++, method, params: params || {} }));
    };

    // Set the page's device metrics (emulated device, or the container size for
    // desktop) and (re)start the screencast at the matching resolution.
    const applySize = () => {
      const dev = deviceRef.current;
      let w;
      let h;
      let dpr;
      let mobile;
      if (dev && dev.width > 0 && dev.height > 0) {
        w = Math.round(dev.width);
        h = Math.round(dev.height);
        dpr = dev.dpr || 0; // 0 → platform default
        mobile = !!dev.mobile;
      } else {
        const r = container.getBoundingClientRect();
        w = Math.max(1, Math.round(r.width));
        h = Math.max(1, Math.round(r.height));
        dpr = 1;
        mobile = false;
      }
      viewportRef.current = { w, h };
      const scale = dpr || 1;
      send('Emulation.setDeviceMetricsOverride', {
        width: w,
        height: h,
        deviceScaleFactor: dpr,
        mobile,
        screenWidth: w,
        screenHeight: h,
      });
      send('Emulation.setTouchEmulationEnabled', { enabled: mobile });
      send('Page.startScreencast', {
        format: 'jpeg',
        quality: 70,
        maxWidth: Math.round(w * scale),
        maxHeight: Math.round(h * scale),
        everyNthFrame: 1,
      });
    };
    applyRef.current = applySize;

    const onMessage = (e) => {
      let msg;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }
      if (msg.method === 'Page.screencastFrame') {
        const { data, sessionId } = msg.params;
        const img = new Image();
        img.onload = () => {
          if (closed) return;
          const ctx = canvas.getContext('2d');
          if (canvas.width !== img.naturalWidth) canvas.width = img.naturalWidth;
          if (canvas.height !== img.naturalHeight) canvas.height = img.naturalHeight;
          ctx.drawImage(img, 0, 0);
        };
        img.src = `data:image/jpeg;base64,${data}`;
        send('Page.screencastFrameAck', { sessionId });
      }
    };

    // (Re)open the CDP socket and (re)start the screencast. Edge proxies can
    // drop an idle WebSocket and the VM browser can be reaped/respawned, so a
    // single connection isn't durable — reconnect instead of freezing the
    // viewport on the last frame.
    const connect = () => {
      if (closed) return;
      ws = new WebSocket(cdpWsUrl(targetId));
      ws.onopen = () => {
        send('Page.enable');
        send('Runtime.enable');
        applySize();
      };
      ws.onmessage = onMessage;
      ws.onerror = () => {};
      ws.onclose = () => {
        if (closed) return;
        clearTimeout(reconnectTimer);
        reconnectTimer = setTimeout(connect, 1000);
      };
    };
    connect();

    // Map a DOM pointer event to emulated-CSS-viewport coords.
    const toDeviceCoords = (e) => {
      const rect = canvas.getBoundingClientRect();
      const vp = viewportRef.current;
      const sx = rect.width ? vp.w / rect.width : 1;
      const sy = rect.height ? vp.h / rect.height : 1;
      return {
        x: Math.round((e.clientX - rect.left) * sx),
        y: Math.round((e.clientY - rect.top) * sy),
      };
    };

    const onMouse = (type) => (e) => {
      const { x, y } = toDeviceCoords(e);
      send('Input.dispatchMouseEvent', {
        type,
        x,
        y,
        button: type === 'mouseMoved' ? 'none' : MOUSE_BUTTON[e.button] || 'left',
        buttons: e.buttons,
        clickCount: type === 'mousePressed' || type === 'mouseReleased' ? 1 : 0,
        modifiers: CDP_MODS(e),
      });
    };
    const onMouseDown = (e) => {
      canvas.focus();
      onMouse('mousePressed')(e);
    };
    const onMouseUp = onMouse('mouseReleased');
    const onMouseMove = onMouse('mouseMoved');
    const onContextMenu = (e) => e.preventDefault();
    const onWheel = (e) => {
      e.preventDefault();
      const { x, y } = toDeviceCoords(e);
      send('Input.dispatchMouseEvent', {
        type: 'mouseWheel',
        x,
        y,
        // CDP wheel deltas use the same sign convention as the DOM wheel event
        // (positive deltaY = scroll down), so forward them as-is.
        deltaX: e.deltaX,
        deltaY: e.deltaY,
        modifiers: CDP_MODS(e),
      });
    };

    const sendKey = (type, e) => {
      send('Input.dispatchKeyEvent', {
        type,
        key: e.key,
        code: e.code,
        windowsVirtualKeyCode: e.keyCode,
        nativeVirtualKeyCode: e.keyCode,
        modifiers: CDP_MODS(e),
      });
    };
    const onKeyDown = (e) => {
      e.preventDefault();
      sendKey('rawKeyDown', e);
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        send('Input.dispatchKeyEvent', { type: 'char', text: e.key, key: e.key, modifiers: CDP_MODS(e) });
      }
    };
    const onKeyUp = (e) => {
      e.preventDefault();
      sendKey('keyUp', e);
    };

    canvas.addEventListener('mousedown', onMouseDown);
    canvas.addEventListener('mouseup', onMouseUp);
    canvas.addEventListener('mousemove', onMouseMove);
    canvas.addEventListener('contextmenu', onContextMenu);
    canvas.addEventListener('wheel', onWheel, { passive: false });
    canvas.addEventListener('keydown', onKeyDown);
    canvas.addEventListener('keyup', onKeyUp);

    let rafId = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(applySize);
    });
    ro.observe(container);

    return () => {
      closed = true;
      clearTimeout(reconnectTimer);
      applyRef.current = null;
      cancelAnimationFrame(rafId);
      ro.disconnect();
      canvas.removeEventListener('mousedown', onMouseDown);
      canvas.removeEventListener('mouseup', onMouseUp);
      canvas.removeEventListener('mousemove', onMouseMove);
      canvas.removeEventListener('contextmenu', onContextMenu);
      canvas.removeEventListener('wheel', onWheel);
      canvas.removeEventListener('keydown', onKeyDown);
      canvas.removeEventListener('keyup', onKeyUp);
      try {
        send('Page.stopScreencast');
      } catch {
        /* socket may already be gone */
      }
      try {
        ws?.close();
      } catch {
        /* ignore */
      }
    };
  }, [targetId]);

  // Re-apply metrics + screencast size when the emulated device changes, reusing
  // the live socket (no reconnect/flicker).
  useEffect(() => {
    applyRef.current?.();
  }, [device]);

  const emulating = !!(device && device.width > 0);

  return (
    <div
      ref={containerRef}
      className={`relative flex h-full w-full items-center justify-center overflow-hidden ${emulating ? 'bg-[#2a2c30]' : 'bg-white'}`}
    >
      <canvas
        ref={canvasRef}
        tabIndex={0}
        className={emulating ? 'block max-h-full max-w-full shadow-2xl outline-none' : 'block h-full w-full outline-none'}
      />
    </div>
  );
}
