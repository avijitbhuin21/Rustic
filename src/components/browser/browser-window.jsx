import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Globe,
  X,
  Minus,
  Maximize2,
  Minimize2,
  Plus,
  ArrowLeft,
  ArrowRight,
  RotateCw,
  RotateCcw,
  Bug,
  Smartphone,
  Loader2,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable';
import { useBrowser } from '@/state/browser';
import { pageReload, pageHistoryGo } from '@/lib/browser-cdp';
import { BrowserView } from './browser-view';
import { BrowserDevtools } from './browser-devtools';

const MIN_W = 360;
const MIN_H = 240;

// Device-emulation presets (dpr 0 = platform default). Drives the live page's
// `Emulation.setDeviceMetricsOverride`, mirroring Chrome's device toolbar.
const DEVICE_PRESETS = [
  { name: 'Responsive', width: 1280, height: 800, dpr: 0, mobile: false },
  { name: 'iPhone SE', width: 375, height: 667, dpr: 2, mobile: true },
  { name: 'iPhone 12/13/14', width: 390, height: 844, dpr: 3, mobile: true },
  { name: 'iPhone 14 Pro Max', width: 430, height: 932, dpr: 3, mobile: true },
  { name: 'Pixel 7', width: 412, height: 915, dpr: 2.625, mobile: true },
  { name: 'Galaxy S20 Ultra', width: 412, height: 915, dpr: 3.5, mobile: true },
  { name: 'iPad Mini', width: 768, height: 1024, dpr: 2, mobile: true },
  { name: 'iPad Pro 11"', width: 834, height: 1194, dpr: 2, mobile: true },
  { name: 'Surface Pro 7', width: 912, height: 1368, dpr: 2, mobile: true },
];
// The preset chosen when the device toolbar is first toggled on.
const DEFAULT_DEVICE = DEVICE_PRESETS[2];

function clamp(v, lo, hi) {
  return Math.min(Math.max(v, lo), hi);
}

// Normalize an address-bar entry into a navigable URL (bare host → https,
// search terms → about:blank guard left to the user).
function normalizeUrl(input) {
  const v = input.trim();
  if (!v) return '';
  if (/^[a-z]+:\/\//i.test(v) || v.startsWith('about:')) return v;
  if (v.startsWith('localhost') || /^\d+\.\d+\.\d+\.\d+/.test(v) || v.includes('.')) {
    return `http://${v}`;
  }
  return `https://${v}`;
}

const RESIZE_HANDLES = [
  ['n', 'top-0 left-0 right-0 h-1 cursor-ns-resize'],
  ['s', 'bottom-0 left-0 right-0 h-1 cursor-ns-resize'],
  ['e', 'top-0 bottom-0 right-0 w-1 cursor-ew-resize'],
  ['w', 'top-0 bottom-0 left-0 w-1 cursor-ew-resize'],
  ['ne', 'top-0 right-0 size-2.5 cursor-nesw-resize'],
  ['nw', 'top-0 left-0 size-2.5 cursor-nwse-resize'],
  ['se', 'bottom-0 right-0 size-2.5 cursor-nwse-resize'],
  ['sw', 'bottom-0 left-0 size-2.5 cursor-nesw-resize'],
];

export function BrowserWindow() {
  const windowState = useBrowser((s) => s.windowState);
  const windowRect = useBrowser((s) => s.windowRect);
  const tabs = useBrowser((s) => s.tabs);
  const activeTabId = useBrowser((s) => s.activeTabId);
  const busy = useBrowser((s) => s.busy);

  const [rect, setRect] = useState(windowRect);
  const [showDevtools, setShowDevtools] = useState(false);
  const [address, setAddress] = useState('');
  // null = desktop (fit container). Otherwise an emulated device descriptor.
  const [device, setDevice] = useState(null);

  const toggleDevice = () => setDevice((d) => (d ? null : { ...DEFAULT_DEVICE }));
  const selectPreset = (name) => {
    const p = DEVICE_PRESETS.find((x) => x.name === name);
    if (p) setDevice({ ...p });
  };
  const setDeviceDim = (key, value) => {
    const n = parseInt(value, 10);
    if (!Number.isFinite(n) || n <= 0) return;
    setDevice((d) => (d ? { ...d, name: 'Custom', [key]: n } : d));
  };
  const rotateDevice = () =>
    setDevice((d) => (d ? { ...d, width: d.height, height: d.width } : d));

  // Keep local geometry in sync when the store's rect changes (e.g. restore).
  useEffect(() => {
    setRect(windowRect);
  }, [windowRect]);

  const activeTab = tabs.find((t) => t.id === activeTabId) || null;

  // Reflect the active tab's URL in the address bar (unless the user is editing
  // — we only overwrite when the tab actually changes URL).
  const lastTabUrl = useRef('');
  useEffect(() => {
    const url = activeTab?.url ?? '';
    if (url !== lastTabUrl.current) {
      lastTabUrl.current = url;
      setAddress(url === 'about:blank' ? '' : url);
    }
  }, [activeTab?.url, activeTabId]);

  if (windowState === 'closed') return null;

  const maximized = windowState === 'maximized';
  const minimized = windowState === 'minimized';

  // ---- drag (title bar) ----
  const onTitlePointerDown = (e) => {
    if (maximized || e.target.closest('[data-window-control]')) return;
    e.preventDefault();
    const start = { mx: e.clientX, my: e.clientY, x: rect.x, y: rect.y, w: rect.w, h: rect.h };
    let cur = { ...rect };
    const move = (ev) => {
      const x = clamp(start.x + (ev.clientX - start.mx), -(start.w - 80), window.innerWidth - 80);
      const y = clamp(start.y + (ev.clientY - start.my), 0, window.innerHeight - 36);
      cur = { ...cur, x, y };
      setRect(cur);
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      useBrowser.getState().setRect(cur);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  // ---- resize (edges/corners) ----
  const onResizePointerDown = (dir) => (e) => {
    if (maximized) return;
    e.preventDefault();
    e.stopPropagation();
    const start = { mx: e.clientX, my: e.clientY, x: rect.x, y: rect.y, w: rect.w, h: rect.h };
    let cur = { ...rect };
    const move = (ev) => {
      const dx = ev.clientX - start.mx;
      const dy = ev.clientY - start.my;
      let { x, y, w, h } = start;
      if (dir.includes('e')) w = Math.max(MIN_W, start.w + dx);
      if (dir.includes('s')) h = Math.max(MIN_H, start.h + dy);
      if (dir.includes('w')) {
        w = Math.max(MIN_W, start.w - dx);
        x = start.x + (start.w - w);
      }
      if (dir.includes('n')) {
        h = Math.max(MIN_H, start.h - dy);
        y = start.y + (start.h - h);
      }
      cur = { x, y, w, h };
      setRect(cur);
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      useBrowser.getState().setRect(cur);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const submitAddress = (e) => {
    e.preventDefault();
    if (!activeTabId) return;
    const url = normalizeUrl(address);
    if (url) useBrowser.getState().navigate(activeTabId, url);
  };

  // Minimized: a compact pill bottom-right that restores on click.
  if (minimized) {
    return createPortal(
      <button
        onClick={() => useBrowser.getState().restore()}
        className="fixed bottom-8 right-4 z-[80] flex items-center gap-2 rounded-lg border border-white/10 bg-background/90 px-3 py-2 text-xs text-foreground shadow-xl backdrop-blur-xl"
      >
        <Globe className="size-4 text-primary/80" />
        Browser
        <span className="text-muted-foreground">({tabs.length})</span>
      </button>,
      document.body,
    );
  }

  const geo = maximized
    ? { left: 0, top: 0, width: '100vw', height: '100vh' }
    : { left: rect.x, top: rect.y, width: rect.w, height: rect.h };

  const viewportContent = activeTabId ? (
    <BrowserView targetId={activeTabId} device={device} />
  ) : (
    <div className="flex h-full w-full items-center justify-center bg-white text-sm text-neutral-400">
      No tab open
    </div>
  );

  return createPortal(
    <AnimatePresence>
      <motion.div
        key="browser-window"
        initial={{ opacity: 0, scale: 0.98 }}
        animate={{ opacity: 1, scale: 1 }}
        exit={{ opacity: 0, scale: 0.98 }}
        transition={{ type: 'spring', stiffness: 420, damping: 32, mass: 0.7 }}
        className={cn(
          'fixed z-[80] flex flex-col overflow-hidden border border-white/10 bg-[#1b1d21] shadow-[0_16px_64px_rgba(0,0,0,0.6)]',
          maximized ? 'rounded-none' : 'rounded-xl',
        )}
        style={geo}
      >
        {/* Title bar (drag handle) */}
        <div
          onPointerDown={onTitlePointerDown}
          onDoubleClick={() => useBrowser.getState().toggleMaximize()}
          className={cn(
            'flex h-9 shrink-0 items-center justify-between gap-2 border-b border-white/[0.06] bg-[#16181c] px-2',
            !maximized && 'cursor-grab active:cursor-grabbing',
          )}
        >
          <div className="flex items-center gap-2 pl-1 text-xs font-medium text-muted-foreground">
            <Globe className="size-3.5 text-primary/70" />
            Browser
            {busy && <Loader2 className="size-3 animate-spin" />}
          </div>
          <div className="flex items-center gap-0.5">
            <WindowBtn title="DevTools" active={showDevtools} onClick={() => setShowDevtools((v) => !v)}>
              <Bug className="size-3.5" />
            </WindowBtn>
            <WindowBtn title="Minimize" onClick={() => useBrowser.getState().minimize()}>
              <Minus className="size-3.5" />
            </WindowBtn>
            <WindowBtn
              title={maximized ? 'Restore' : 'Maximize'}
              onClick={() => useBrowser.getState().toggleMaximize()}
            >
              {maximized ? <Minimize2 className="size-3.5" /> : <Maximize2 className="size-3.5" />}
            </WindowBtn>
            <WindowBtn title="Close" danger onClick={() => useBrowser.getState().close()}>
              <X className="size-3.5" />
            </WindowBtn>
          </div>
        </div>

        {/* Tab strip */}
        <div className="flex h-8 shrink-0 items-center gap-1 overflow-x-auto border-b border-white/[0.06] bg-[#16181c] px-1.5">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              onClick={() => useBrowser.getState().setActiveTab(tab.id)}
              className={cn(
                'group flex h-6 max-w-[180px] cursor-pointer items-center gap-1.5 rounded-md px-2 text-xs',
                tab.id === activeTabId
                  ? 'bg-white/10 text-foreground'
                  : 'text-muted-foreground hover:bg-white/5',
              )}
            >
              {tab.favicon ? (
                <img src={tab.favicon} alt="" className="size-3.5 shrink-0 rounded-sm" />
              ) : (
                <Globe className="size-3.5 shrink-0 opacity-60" />
              )}
              <span className="truncate">{tab.title || tab.url || 'New tab'}</span>
              <button
                data-window-control
                onClick={(e) => {
                  e.stopPropagation();
                  useBrowser.getState().closeTab(tab.id);
                }}
                className="ml-0.5 hidden rounded p-0.5 hover:bg-white/10 group-hover:block"
              >
                <X className="size-3" />
              </button>
            </div>
          ))}
          <button
            data-window-control
            title="New tab"
            onClick={() => useBrowser.getState().newTab()}
            className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-white/10 hover:text-foreground"
          >
            <Plus className="size-3.5" />
          </button>
        </div>

        {/* Address bar */}
        <div className="flex h-9 shrink-0 items-center gap-1 border-b border-white/[0.06] bg-[#16181c] px-2">
          <NavBtn
            title="Back"
            disabled={!activeTabId}
            onClick={() => activeTabId && pageHistoryGo(activeTabId, -1).catch(() => {})}
          >
            <ArrowLeft className="size-4" />
          </NavBtn>
          <NavBtn
            title="Forward"
            disabled={!activeTabId}
            onClick={() => activeTabId && pageHistoryGo(activeTabId, 1).catch(() => {})}
          >
            <ArrowRight className="size-4" />
          </NavBtn>
          <NavBtn
            title="Reload"
            disabled={!activeTabId}
            onClick={() => activeTabId && pageReload(activeTabId).catch(() => {})}
          >
            <RotateCw className="size-3.5" />
          </NavBtn>
          <form onSubmit={submitAddress} className="flex-1">
            <input
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              placeholder="Enter URL (e.g. localhost:3000)"
              spellCheck={false}
              className="h-6 w-full rounded-md border border-white/10 bg-[#0e1013] px-2.5 text-xs text-foreground outline-none focus:border-primary/50"
            />
          </form>
          <NavBtn
            title="Toggle device toolbar"
            disabled={!activeTabId}
            active={!!device}
            onClick={toggleDevice}
          >
            <Smartphone className="size-3.5" />
          </NavBtn>
        </div>

        {/* Device-emulation sub-bar (like Chrome's device toolbar). Drives the
            live page in the main viewport, not a separate preview. */}
        {device && (
          <div className="flex h-8 shrink-0 items-center gap-2 border-b border-white/[0.06] bg-[#101317] px-2 text-xs text-muted-foreground">
            <select
              value={DEVICE_PRESETS.some((p) => p.name === device.name) ? device.name : 'Custom'}
              onChange={(e) => selectPreset(e.target.value)}
              className="h-6 rounded-md border border-white/10 bg-[#0e1013] px-1.5 text-xs text-foreground outline-none focus:border-primary/50"
            >
              {device.name === 'Custom' && <option value="Custom">Custom</option>}
              {DEVICE_PRESETS.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
            <div className="flex items-center gap-1">
              <input
                type="number"
                value={device.width}
                onChange={(e) => setDeviceDim('width', e.target.value)}
                className="h-6 w-16 rounded-md border border-white/10 bg-[#0e1013] px-1.5 text-center text-xs text-foreground outline-none focus:border-primary/50"
              />
              <span className="opacity-60">×</span>
              <input
                type="number"
                value={device.height}
                onChange={(e) => setDeviceDim('height', e.target.value)}
                className="h-6 w-16 rounded-md border border-white/10 bg-[#0e1013] px-1.5 text-center text-xs text-foreground outline-none focus:border-primary/50"
              />
            </div>
            <NavBtn title="Rotate" onClick={rotateDevice}>
              <RotateCcw className="size-3.5" />
            </NavBtn>
            {device.dpr ? <span className="opacity-60">DPR {device.dpr}</span> : null}
            {device.mobile ? <span className="opacity-60">· touch</span> : null}
          </div>
        )}

        {/* Body: viewport, with an optional resizable DevTools dock on the right
            (drag the divider to resize), like Chrome's "dock to right". */}
        <div className="relative flex min-h-0 flex-1">
          {showDevtools ? (
            <ResizablePanelGroup direction="horizontal">
              <ResizablePanel id="browser-viewport" defaultSize="62%" minSize="25%">
                {viewportContent}
              </ResizablePanel>
              <ResizableHandle withHandle />
              <ResizablePanel id="browser-devtools" defaultSize="38%" minSize="20%">
                <BrowserDevtools targetId={activeTabId} />
              </ResizablePanel>
            </ResizablePanelGroup>
          ) : (
            viewportContent
          )}
        </div>

        {/* Resize handles */}
        {!maximized &&
          RESIZE_HANDLES.map(([dir, cls]) => (
            <div
              key={dir}
              onPointerDown={onResizePointerDown(dir)}
              className={cn('absolute z-10', cls)}
            />
          ))}
      </motion.div>
    </AnimatePresence>,
    document.body,
  );
}

function WindowBtn({ children, title, onClick, active, danger }) {
  return (
    <button
      data-window-control
      title={title}
      onClick={onClick}
      className={cn(
        'flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors',
        active && 'bg-white/10 text-foreground',
        danger ? 'hover:bg-red-500/80 hover:text-white' : 'hover:bg-white/10 hover:text-foreground',
      )}
    >
      {children}
    </button>
  );
}

function NavBtn({ children, title, onClick, disabled, active }) {
  return (
    <button
      data-window-control
      title={title}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-white/10 hover:text-foreground disabled:opacity-40',
        active && 'bg-primary/20 text-primary',
      )}
    >
      {children}
    </button>
  );
}
