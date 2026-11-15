import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Music, Play, Pause, SkipBack, Volume2, VolumeX } from 'lucide-react';
import { fileSize, fileStreamUrl } from '@/lib/file-bytes';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import { basename } from '@/state/editor';
import { PreviewSurface } from './preview-surface';

// Everything streams: the waveform is computed by the backend in one decode
// pass with constant memory, and playback pulls short PCM chunks around the
// playhead. No file — 3 MB or 30 GB — is ever held whole in the webview.
const OVERVIEW_BUCKETS = 4000;
const CHUNK_SECONDS = 4;
const LOOKAHEAD_SECONDS = 10;

function formatTime(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00.0';
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const frac = Math.floor((seconds - total) * 10);
  const mm = h ? String(m).padStart(2, '0') : String(m);
  return `${h ? `${h}:` : ''}${mm}:${String(s).padStart(2, '0')}.${frac}`;
}

function channelLabel(count) {
  if (count === 1) return 'mono';
  if (count === 2) return 'stereo';
  return `${count} ch`;
}

function tokenColor(el, name, fallback) {
  const value = getComputedStyle(el).getPropertyValue(name).trim();
  return value || fallback;
}

function b64ToF32(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Float32Array(bytes.buffer);
}

/** Canvas waveform drawn from precomputed (min,max) peaks, with a scrubbable playhead. */
function Waveform({ peaks, duration, position, onSeek }) {
  const canvasRef = useRef(null);
  const draggingRef = useRef(false);

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas || !peaks) return;
    const dpr = window.devicePixelRatio || 1;
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    if (width <= 0 || height <= 0) return;
    if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) {
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
    }
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    const played = tokenColor(canvas, '--primary', '#7c8cff');
    const pending = tokenColor(canvas, '--muted-foreground', '#8a8a8a');
    const mid = height / 2;
    const progressX = duration > 0 ? (position / duration) * width : 0;
    const n = peaks.length / 2;
    const cols = Math.max(1, Math.floor(width));
    for (let x = 0; x < cols; x++) {
      const lo = Math.floor((x * n) / cols);
      const hi = Math.max(lo + 1, Math.floor(((x + 1) * n) / cols));
      let min = 0;
      let max = 0;
      for (let i = lo; i < hi && i < n; i++) {
        min = Math.min(min, peaks[i * 2]);
        max = Math.max(max, peaks[i * 2 + 1]);
      }
      const top = mid - max * mid * 0.92;
      const bottom = mid - min * mid * 0.92;
      ctx.fillStyle = x <= progressX ? played : pending;
      ctx.globalAlpha = x <= progressX ? 1 : 0.45;
      ctx.fillRect(x, Math.min(top, mid - 0.5), 1, Math.max(1, bottom - top));
    }
    ctx.globalAlpha = 1;
    ctx.fillStyle = played;
    ctx.fillRect(Math.min(width - 1, Math.max(0, progressX)), 0, 1, height);
  }, [peaks, duration, position]);

  useEffect(() => { draw(); }, [draw]);
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return undefined;
    const ro = new ResizeObserver(() => draw());
    ro.observe(canvas);
    return () => ro.disconnect();
  }, [draw]);

  const seekFromEvent = (e) => {
    const canvas = canvasRef.current;
    if (!canvas || duration <= 0) return;
    const rect = canvas.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    onSeek(ratio * duration);
  };

  return (
    <canvas
      ref={canvasRef}
      className="h-28 w-full cursor-pointer rounded-md bg-muted/30"
      onPointerDown={(e) => {
        draggingRef.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
        seekFromEvent(e);
      }}
      onPointerMove={(e) => { if (draggingRef.current) seekFromEvent(e); }}
      onPointerUp={(e) => {
        draggingRef.current = false;
        try { e.currentTarget.releasePointerCapture(e.pointerId); } catch { /* released */ }
      }}
    />
  );
}

/**
 * Editor preview for audio files. Waveform + facts from `audio_overview`;
 * playback schedules backend-decoded PCM chunks on Web Audio, resampled to the
 * device rate — so rates the <audio> element rejects (2 kHz clinical
 * recordings) still play, and file size never matters.
 */
export default function AudioPreview({ tab }) {
  const [overview, setOverview] = useState(null);
  const [error, setError] = useState(null);
  const [fallbackSrc, setFallbackSrc] = useState(null);
  const [size, setSize] = useState(null);
  const [playing, setPlaying] = useState(false);
  const [position, setPosition] = useState(0);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [buffering, setBuffering] = useState(false);

  const ctxRef = useRef(null);
  const gainRef = useRef(null);
  const sourcesRef = useRef([]);
  const genRef = useRef(0);
  const startedAtRef = useRef(0);
  const nextMediaRef = useRef(0);
  const eosRef = useRef(false);
  const pumpingRef = useRef(false);
  const rafRef = useRef(0);
  const pumpTimerRef = useRef(0);
  const reloadVersion = useFileReloadVersion(tab.path);
  const duration = overview?.duration ?? 0;

  const stopAll = useCallback(() => {
    genRef.current += 1;
    for (const s of sourcesRef.current) {
      s.onended = null;
      try { s.stop(); } catch { /* already stopped */ }
      s.disconnect();
    }
    sourcesRef.current = [];
    cancelAnimationFrame(rafRef.current);
    clearInterval(pumpTimerRef.current);
    pumpingRef.current = false;
    setBuffering(false);
  }, []);

  useEffect(() => {
    let cancelled = false;
    stopAll();
    setOverview(null);
    setError(null);
    setFallbackSrc(null);
    setSize(null);
    setPosition(0);
    setPlaying(false);
    fileSize(tab.path, reloadVersion).then((n) => { if (!cancelled) setSize(n); }).catch(() => {});
    invoke('audio_overview', { path: tab.path, buckets: OVERVIEW_BUCKETS })
      .then((o) => { if (!cancelled) setOverview(o); })
      .catch((e) => {
        if (cancelled) return;
        // A container/codec our decoder doesn't know (Opus, WMA…): let the
        // platform player try it, streamed from disk.
        setFallbackSrc(fileStreamUrl(tab.path, reloadVersion));
        setError(`Waveform unavailable (${String(e?.message || e)}) — native playback.`);
      });
    return () => { cancelled = true; stopAll(); };
  }, [tab.path, reloadVersion, stopAll]);

  useEffect(() => () => {
    stopAll();
    ctxRef.current?.close();
    ctxRef.current = null;
  }, [stopAll]);

  useEffect(() => {
    if (gainRef.current) gainRef.current.gain.value = muted ? 0 : volume;
  }, [volume, muted]);

  const ensureCtx = () => {
    if (!ctxRef.current) {
      const Ctor = window.AudioContext || window.webkitAudioContext;
      ctxRef.current = new Ctor();
      gainRef.current = ctxRef.current.createGain();
      gainRef.current.gain.value = muted ? 0 : volume;
      gainRef.current.connect(ctxRef.current.destination);
    }
    if (ctxRef.current.state === 'suspended') ctxRef.current.resume();
    return ctxRef.current;
  };

  /** Keeps ~LOOKAHEAD_SECONDS of decoded audio scheduled ahead of the playhead. */
  const pump = useCallback(async () => {
    const ctx = ctxRef.current;
    if (!ctx || pumpingRef.current || eosRef.current) return;
    const gen = genRef.current;
    pumpingRef.current = true;
    try {
      while (gen === genRef.current && !eosRef.current) {
        const playhead = ctx.currentTime - startedAtRef.current;
        if (nextMediaRef.current - playhead > LOOKAHEAD_SECONDS) break;
        const chunk = await invoke('audio_pcm_chunk', {
          path: tab.path,
          start: nextMediaRef.current,
          seconds: CHUNK_SECONDS,
          outRate: Math.round(ctx.sampleRate),
        });
        if (gen !== genRef.current) return;
        if (chunk.frames > 0) {
          const pcm = b64ToF32(chunk.data);
          const buf = ctx.createBuffer(chunk.channels, chunk.frames, chunk.sample_rate);
          for (let c = 0; c < chunk.channels; c++) {
            const plane = buf.getChannelData(c);
            for (let i = 0; i < chunk.frames; i++) plane[i] = pcm[i * chunk.channels + c];
          }
          const src = ctx.createBufferSource();
          src.buffer = buf;
          src.connect(gainRef.current);
          const at = startedAtRef.current + nextMediaRef.current;
          const lateBy = Math.max(0, ctx.currentTime - at);
          src.start(Math.max(at, ctx.currentTime), lateBy);
          sourcesRef.current.push(src);
          src.onended = () => {
            sourcesRef.current = sourcesRef.current.filter((s) => s !== src);
          };
          nextMediaRef.current += chunk.frames / chunk.sample_rate;
        }
        setBuffering(false);
        if (chunk.end_of_stream || chunk.frames === 0) eosRef.current = true;
      }
    } catch (e) {
      if (gen === genRef.current) {
        setError(`Playback failed (${String(e?.message || e)}).`);
        stopAll();
        setPlaying(false);
      }
    } finally {
      if (gen === genRef.current) pumpingRef.current = false;
    }
  }, [tab.path, stopAll]);

  const startAt = useCallback((offset) => {
    const ctx = ensureCtx();
    stopAll();
    const from = Math.min(Math.max(0, offset), Math.max(0, duration - 0.01));
    // Small lead so the first chunk is scheduled before its start time.
    startedAtRef.current = ctx.currentTime + 0.15 - from;
    nextMediaRef.current = from;
    eosRef.current = false;
    setPosition(from);
    setPlaying(true);
    setBuffering(true);
    pump();
    pumpTimerRef.current = setInterval(pump, 400);
    const tick = () => {
      const pos = ctx.currentTime - startedAtRef.current;
      if (pos >= duration - 0.005) {
        stopAll();
        setPlaying(false);
        setPosition(duration);
        return;
      }
      setPosition(Math.max(from, pos));
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [duration, pump, stopAll]);

  const togglePlay = () => {
    if (playing) {
      const ctx = ctxRef.current;
      const at = ctx ? ctx.currentTime - startedAtRef.current : position;
      stopAll();
      setPosition(Math.min(duration, Math.max(0, at)));
      setPlaying(false);
      return;
    }
    startAt(position >= duration - 0.01 ? 0 : position);
  };

  const seek = (time) => {
    const clamped = Math.min(duration, Math.max(0, time));
    setPosition(clamped);
    if (playing) startAt(clamped);
  };

  if (!overview && !fallbackSrc) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-6">
        <Skeleton className="h-40 w-[32rem]" />
        <span className="text-[11px] text-muted-foreground">Reading waveform…</span>
      </div>
    );
  }

  const deviceRate = Math.round(ctxRef.current?.sampleRate ?? 0);
  const toolbar = (
    <>
      <div className="truncate text-xs text-muted-foreground">{basename(tab.path)}</div>
      <div className="flex shrink-0 items-center gap-3 text-[11px] text-muted-foreground">
        {overview && (
          <span>
            {overview.sample_rate.toLocaleString()} Hz · {channelLabel(overview.channels)}
            {overview.bits_per_sample ? ` · ${overview.bits_per_sample}-bit` : ''}
          </span>
        )}
        {size != null && <span>{(size / (1024 * 1024)).toFixed(1)} MB</span>}
      </div>
    </>
  );

  return (
    <PreviewSurface toolbar={toolbar}>
      <div className="flex h-full min-h-full w-full items-center justify-center p-6">
        <div className="flex w-full max-w-2xl flex-col gap-4 rounded-lg border border-border bg-card p-6 shadow-sm">
          <div className="flex items-center gap-3">
            <Music className="size-5 shrink-0 text-muted-foreground" />
            <div className="min-w-0 flex-1 truncate text-sm text-foreground">{basename(tab.path)}</div>
          </div>

          {fallbackSrc ? (
            <>
              <audio src={fallbackSrc} controls className="w-full" />
              <p className="text-xs text-muted-foreground">{error}</p>
            </>
          ) : (
            <>
              <Waveform peaks={overview.peaks} duration={duration} position={position} onSeek={seek} />
              <div className="flex items-center gap-3">
                <Button size="icon-sm" variant="secondary" onClick={togglePlay} title="Play / pause">
                  {playing ? <Pause /> : <Play />}
                </Button>
                <Button size="icon-sm" variant="ghost" onClick={() => seek(0)} title="Back to start">
                  <SkipBack />
                </Button>
                <span className="tabular-nums text-xs text-muted-foreground">
                  {formatTime(position)} / {formatTime(duration)}
                  {buffering && playing ? ' · buffering…' : ''}
                </span>
                <div className="ml-auto flex items-center gap-2">
                  <Button size="icon-sm" variant="ghost" onClick={() => setMuted((m) => !m)} title={muted ? 'Unmute' : 'Mute'}>
                    {muted ? <VolumeX /> : <Volume2 />}
                  </Button>
                  <Slider
                    value={[muted ? 0 : volume]}
                    min={0}
                    max={1}
                    step={0.01}
                    onValueChange={([v]) => {
                      setVolume(v);
                      if (v > 0) setMuted(false);
                    }}
                    className="w-24"
                  />
                </div>
              </div>
              {error && <p className="text-xs text-destructive">{error}</p>}
              {deviceRate > 0 && deviceRate !== overview.sample_rate && (
                <p className="text-[11px] text-muted-foreground">
                  Source runs at {overview.sample_rate.toLocaleString()} Hz — resampled to{' '}
                  {deviceRate.toLocaleString()} Hz for playback, pitch and duration preserved.
                </p>
              )}
            </>
          )}
        </div>
      </div>
    </PreviewSurface>
  );
}