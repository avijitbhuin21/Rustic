import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  Play, Pause, StepBack, StepForward, RotateCw, Volume2, VolumeX, MessageSquarePlus, Pin, X, ShieldAlert,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import { useAgent } from '@/state/agent';
import { basename } from '@/state/editor';
import { appendToPrompt } from '@/lib/prompt-append';
import { isFilmAllowed, allowFilm } from '@/lib/film-consent';

// The film library's render defaults; a page draws for this viewport.
const FPS = 24;
const VIEW = { w: 1920, h: 1080 };
const MAX_DUR = 3 * 60 * 60;
const MAX_MARKERS = 400;
const ACK_TIMEOUT_MS = 1500;
const RELOAD_FILES = /\.(m?js|html?|css|json)$/i;

const norm = (p) => String(p || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
const clampT = (t, dur) => Math.max(0, Math.min(dur, Math.round(t * FPS) / FPS));
/** Seconds as m:ss.cc. */
const fmt = (t) => `${Math.floor(t / 60)}:${(t % 60).toFixed(2).padStart(5, '0')}`;

/**
 * Film view for an HTML page that follows the `window.render(t)` contract: the page runs
 * in a sandboxed frame served by a film session, and this view drives its clock.
 */
export default function FilmPreview({ tab }) {
  const [version, setVersion] = useState(0);
  const [session, setSession] = useState(null);
  const [error, setError] = useState(null);
  const [allowed, setAllowed] = useState(false);
  const [pageError, setPageError] = useState(null);
  const [ready, setReady] = useState(false);
  const [dur, setDur] = useState(0);
  const [events, setEvents] = useState([]);
  const [t, setT] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const [audioOk, setAudioOk] = useState(true);
  const [scale, setScale] = useState(0);
  const [notes, setNotes] = useState([]);
  const [noteText, setNoteText] = useState('');

  const frameRef = useRef(null);
  const stageRef = useRef(null);
  const audioRef = useRef(null);
  const pageRef = useRef(null);
  const tRef = useRef(0);
  const durRef = useRef(0);
  const resumeRef = useRef(0);
  const pendingRef = useRef(0);
  const queuedRef = useRef(null);

  /** Ask the page to draw second `time`; while a frame is outstanding only the latest request is kept. */
  const sendSeek = useCallback((time) => {
    const page = pageRef.current;
    if (!page) return;
    if (pendingRef.current && performance.now() - pendingRef.current < ACK_TIMEOUT_MS) {
      queuedRef.current = time;
      return;
    }
    pendingRef.current = performance.now();
    queuedRef.current = null;
    try {
      page.postMessage({ source: 'rustic-film-host', type: 'seek', t: time }, '*');
    } catch {
      pendingRef.current = 0;
    }
  }, []);

  /** Move the playhead to `time` (snapped to a frame) and redraw. */
  const seek = useCallback((time) => {
    const next = clampT(time, durRef.current);
    tRef.current = next;
    setT(next);
    sendSeek(next);
    const audio = audioRef.current;
    if (audio && audio.paused) {
      try { audio.currentTime = next; } catch { /* not loaded yet */ }
    }
  }, [sendSeek]);

  const reload = useCallback(() => {
    resumeRef.current = tRef.current;
    setVersion((v) => v + 1);
  }, []);

  // One session per (page, reload). The session id is the frame's only way in.
  useEffect(() => {
    let cancelled = false;
    let id = null;
    setSession(null);
    setError(null);
    setPageError(null);
    setReady(false);
    setPlaying(false);
    setAudioOk(true);
    pageRef.current = null;
    pendingRef.current = 0;
    queuedRef.current = null;
    invoke('film_open', { path: tab.path })
      .then((s) => {
        if (cancelled) {
          invoke('film_close', { id: s.id }).catch(() => {});
          return;
        }
        id = s.id;
        // Opening a session loads nothing; the frame is only mounted once the project is allowed.
        setAllowed(isFilmAllowed(s.project));
        setSession(s);
      })
      .catch((e) => { if (!cancelled) setError(String(e?.message || e)); });
    return () => {
      cancelled = true;
      if (id) invoke('film_close', { id }).catch(() => {});
    };
  }, [tab.path, version]);

  // Messages from the page's bridge. The page is untrusted: accept only the three
  // known message types, only from the window directly inside our frame, and
  // validate every field.
  useEffect(() => {
    const onMessage = (e) => {
      const shell = frameRef.current?.contentWindow;
      const d = e.data;
      if (!shell || !d || d.source !== 'rustic-film') return;
      let fromPage = false;
      try { fromPage = !!e.source && e.source !== shell && e.source.parent === shell; } catch { fromPage = false; }
      if (!fromPage) return;
      if (d.type === 'ready') {
        const total = Number(d.dur);
        if (!(total > 0) || !Number.isFinite(total)) {
          setPageError('The page set window.READY, but window.DUR is not a positive number of seconds.');
          return;
        }
        const list = [];
        for (const ev of Array.isArray(d.events) ? d.events : []) {
          const at = Number(ev?.t);
          if (list.length < MAX_MARKERS && Number.isFinite(at) && at >= 0 && at <= total) {
            list.push({ t: at, type: String(ev?.type ?? '').slice(0, 40) });
          }
        }
        pageRef.current = e.source;
        durRef.current = Math.min(total, MAX_DUR);
        setDur(durRef.current);
        setEvents(list);
        setReady(true);
        const start = clampT(resumeRef.current, durRef.current);
        tRef.current = start;
        setT(start);
        sendSeek(start);
      } else if (d.type === 'frame') {
        pendingRef.current = 0;
        if (queuedRef.current != null) sendSeek(queuedRef.current);
      } else if (d.type === 'error') {
        setPageError(String(d.message ?? '').slice(0, 600));
      }
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [sendSeek]);

  // Real-time playback. The sound mix is the clock when it is playing; the wall clock otherwise.
  useEffect(() => {
    if (!playing || !ready) return undefined;
    const audio = audioRef.current;
    const from = tRef.current >= durRef.current - 1 / FPS ? 0 : tRef.current;
    const started = performance.now();
    let audioRunning = false;
    let raf = 0;
    let last = -1;
    if (audio) {
      try { audio.currentTime = from; } catch { /* not loaded yet */ }
      audio.play().then(() => { audioRunning = true; }).catch(() => {});
    }
    const tick = () => {
      const now = audioRunning && audio && !audio.paused
        ? audio.currentTime
        : from + (performance.now() - started) / 1000;
      if (now >= durRef.current) {
        tRef.current = durRef.current;
        setT(durRef.current);
        sendSeek(durRef.current);
        setPlaying(false);
        return;
      }
      const frame = Math.floor(now * FPS) / FPS;
      if (frame !== last) {
        last = frame;
        tRef.current = frame;
        setT(frame);
        sendSeek(frame);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      if (audio) audio.pause();
    };
  }, [playing, ready, sendSeek]);

  // Fit the fixed-size film viewport into the pane.
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return undefined;
    const fit = () => {
      const r = el.getBoundingClientRect();
      setScale(Math.max(0, Math.min((r.width - 24) / VIEW.w, (r.height - 24) / VIEW.h)));
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Reload when the page's sources change on disk (render output and packages are ignored).
  const root = session?.root;
  useEffect(() => {
    if (!root) return undefined;
    const base = norm(root);
    let timer = null;
    let unlisten = null;
    let disposed = false;
    listen('rustic:fs-change', (e) => {
      const paths = Array.isArray(e.payload?.changed_paths) ? e.payload.changed_paths : [];
      const hit = paths.some((p) => {
        const n = norm(p);
        if (!n.startsWith(`${base}/`) || !RELOAD_FILES.test(n)) return false;
        return !/\/(node_modules|out)\//.test(n.slice(base.length));
      });
      if (!hit) return;
      clearTimeout(timer);
      timer = setTimeout(reload, 400);
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    }).catch(() => {});
    return () => {
      disposed = true;
      clearTimeout(timer);
      if (typeof unlisten === 'function') unlisten();
    };
  }, [root, reload]);

  const step = (frames) => {
    setPlaying(false);
    seek(tRef.current + frames / FPS);
  };

  const pinNote = () => {
    const text = noteText.trim();
    if (!text) return;
    setNotes((list) => [...list, { t: tRef.current, text }].sort((a, b) => a.t - b.t));
    setNoteText('');
  };

  const sendNotes = () => {
    if (notes.length === 0) return;
    const projectRoot = String(useAgent.getState().activeProject?.root || '').replace(/\\/g, '/').replace(/\/+$/, '');
    const path = String(tab.path).replace(/\\/g, '/');
    const shown = projectRoot && path.startsWith(`${projectRoot}/`) ? path.slice(projectRoot.length + 1) : path;
    const lines = notes.map((n) => `- at ${n.t.toFixed(2)} s: ${n.text}`);
    appendToPrompt(`Notes on the film page \`${shown}\` (times are seconds into the film):\n${lines.join('\n')}`);
    setNotes([]);
  };

  const audioSrc = session?.audio && audioOk
    ? `${convertFileSrc(session.audio, 'rustic-media')}${convertFileSrc(session.audio, 'rustic-media').includes('?') ? '&' : '?'}v=${version}`
    : null;
  const disabled = !ready;

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-2">
        <div className="truncate text-xs text-muted-foreground">{basename(tab.path)}</div>
        {tab.dirty && (
          <span className="shrink-0 text-[11px] text-muted-foreground">Unsaved changes are not shown</span>
        )}
        <div className="flex-1" />
        <Button size="icon-xs" variant="ghost" onClick={reload} title="Reload the page" aria-label="Reload the page">
          <RotateCw />
        </Button>
      </div>

      <div ref={stageRef} className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-muted/30">
        {error ? (
          <div className="max-w-md p-4 text-center text-sm text-destructive">{error}</div>
        ) : (
          <>
            {session && !allowed && (
              <div data-film-consent className="m-4 max-w-md rounded-lg border border-border bg-card p-4 text-card-foreground">
                <div className="mb-2 flex items-center gap-2 text-sm font-medium">
                  <ShieldAlert className="size-4 text-primary" /> Run this page&apos;s scripts?
                </div>
                <p className="text-xs text-muted-foreground">
                  Film view runs the page&apos;s own JavaScript in a sandbox. It cannot reach Rustic, hidden files
                  such as .env and .git, or anything outside this folder:
                </p>
                <p className="my-2 break-all font-mono text-[11px]">{session.root}</p>
                <p className="text-xs text-muted-foreground">
                  It can read the other files in that folder. Ordinary network requests are blocked, but a page
                  could still send those files out over WebRTC, which Rustic cannot block. Allow this only for
                  projects whose code you trust.
                </p>
                <div className="mt-3 flex items-center gap-2">
                  <Button size="sm" onClick={() => { allowFilm(session.project); setAllowed(true); }}>
                    Allow for this project
                  </Button>
                  <span className="text-[11px] text-muted-foreground">Asked once per project.</span>
                </div>
              </div>
            )}
            {session && allowed && scale > 0 && (
              <div className="overflow-hidden shadow-lg" style={{ width: VIEW.w * scale, height: VIEW.h * scale }}>
                <iframe
                  key={session.id}
                  ref={frameRef}
                  // The page's own scripts run here. No `allow-same-origin`: the frame has an
                  // opaque origin, so it cannot read Rustic's storage or cookies, and the
                  // session's CSP keeps every request inside the session.
                  sandbox="allow-scripts"
                  referrerPolicy="no-referrer"
                  src={session.url}
                  title={`Film view — ${basename(tab.path)}`}
                  className="pointer-events-none block border-0"
                  style={{ width: VIEW.w, height: VIEW.h, transform: `scale(${scale})`, transformOrigin: '0 0' }}
                />
              </div>
            )}
            {!ready && (!session || allowed) && (
              <div className="absolute inset-x-0 bottom-3 text-center text-xs text-muted-foreground">
                Waiting for the page to set window.READY…
              </div>
            )}
          </>
        )}
      </div>

      {pageError && !error && (
        <div className="shrink-0 truncate border-t border-border px-3 py-1 text-[11px] text-destructive" title={pageError}>
          Page error: {pageError}
        </div>
      )}

      <div className="flex shrink-0 items-center gap-2 border-t border-border px-2 py-1.5">
        <Button size="icon-xs" variant="ghost" disabled={disabled} onClick={() => step(-1)} title="Back one frame" aria-label="Back one frame">
          <StepBack />
        </Button>
        <Button
          size="icon-xs"
          variant="ghost"
          disabled={disabled}
          onClick={() => setPlaying((p) => !p)}
          title={playing ? 'Pause' : 'Play'}
          aria-label={playing ? 'Pause' : 'Play'}
        >
          {playing ? <Pause /> : <Play />}
        </Button>
        <Button size="icon-xs" variant="ghost" disabled={disabled} onClick={() => step(1)} title="Forward one frame" aria-label="Forward one frame">
          <StepForward />
        </Button>
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-muted-foreground" data-film-time>
          {fmt(t)} / {fmt(dur)}
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <Slider
            value={[t]}
            min={0}
            max={dur || 1}
            step={1 / FPS}
            disabled={disabled}
            onValueChange={([v]) => { setPlaying(false); seek(v); }}
            aria-label="Film time"
          />
          {/* Cue markers from the page's window.EV; inset by half a thumb so they line up with the handle. */}
          <div className="px-1.5">
            <div className="relative h-2">
              {dur > 0 && events.map((ev, i) => (
                <button
                  // eslint-disable-next-line react/no-array-index-key
                  key={i}
                  type="button"
                  data-film-marker
                  onClick={() => { setPlaying(false); seek(ev.t); }}
                  title={`${fmt(ev.t)}${ev.type ? ` · ${ev.type}` : ''}`}
                  className="absolute top-0 h-2 w-px bg-muted-foreground/60 hover:w-0.5 hover:bg-primary"
                  style={{ left: `${(ev.t / dur) * 100}%` }}
                />
              ))}
            </div>
          </div>
        </div>
        {session?.audio && (
          <Button
            size="icon-xs"
            variant="ghost"
            disabled={!audioOk}
            onClick={() => setMuted((m) => !m)}
            title={!audioOk ? 'The sound mix could not be played' : muted ? 'Unmute the sound mix' : 'Mute the sound mix'}
            aria-label={muted ? 'Unmute the sound mix' : 'Mute the sound mix'}
          >
            {muted || !audioOk ? <VolumeX /> : <Volume2 />}
          </Button>
        )}
      </div>

      <div className="flex shrink-0 flex-col gap-1.5 border-t border-border px-2 py-1.5">
        <div className="flex items-center gap-2">
          <input
            value={noteText}
            onChange={(e) => setNoteText(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') pinNote(); }}
            placeholder={`Note at ${fmt(t)}`}
            disabled={disabled}
            className="h-7 min-w-0 flex-1 rounded-md border border-border bg-background px-2 text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-50"
          />
          <Button size="xs" variant="ghost" disabled={disabled || !noteText.trim()} onClick={pinNote}>
            <Pin /> Pin
          </Button>
          <Button size="xs" variant="ghost" disabled={notes.length === 0} onClick={sendNotes} title="Put the pinned notes in the chat box. Nothing is sent until you send it.">
            <MessageSquarePlus /> Add to chat
          </Button>
        </div>
        {notes.length > 0 && (
          <div className="flex max-h-20 flex-wrap gap-1 overflow-y-auto">
            {notes.map((n, i) => (
              <span
                // eslint-disable-next-line react/no-array-index-key
                key={i}
                className="flex max-w-full items-center gap-1 rounded-md border border-border bg-muted/40 py-0.5 pl-1.5 pr-0.5 text-[11px]"
              >
                <button type="button" className="font-mono tabular-nums text-muted-foreground hover:text-foreground" onClick={() => { setPlaying(false); seek(n.t); }}>
                  {fmt(n.t)}
                </button>
                <span className="truncate text-foreground">{n.text}</span>
                <button
                  type="button"
                  aria-label="Remove note"
                  className="rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
                  onClick={() => setNotes((list) => list.filter((_, j) => j !== i))}
                >
                  <X className="size-3" />
                </button>
              </span>
            ))}
          </div>
        )}
      </div>

      {audioSrc && (
        <audio ref={audioRef} src={audioSrc} muted={muted} preload="auto" onError={() => setAudioOk(false)} className="hidden" />
      )}
    </div>
  );
}
