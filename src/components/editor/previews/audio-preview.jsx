import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Music, Play, Pause, SkipBack, Volume2, VolumeX } from 'lucide-react';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { Skeleton } from '@/components/ui/skeleton';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';
import { basename } from '@/state/editor';
import { PreviewSurface } from './preview-surface';
import { decodeAudio, computePeaks } from '@/lib/audio-decode';

const MIME = {
  mp3: 'audio/mpeg',
  m4a: 'audio/mp4',
  aac: 'audio/aac',
  wav: 'audio/wav',
  flac: 'audio/flac',
  ogg: 'audio/ogg',
  oga: 'audio/ogg',
  opus: 'audio/ogg',
  weba: 'audio/webm',
  aiff: 'audio/aiff',
  aif: 'audio/aiff',
  wma: 'audio/x-ms-wma',
};

function mimeFor(path) {
  const dot = path.lastIndexOf('.');
  const ext = dot < 0 ? '' : path.slice(dot + 1).toLowerCase();
  return MIME[ext] ?? 'audio/mpeg';
}

function base64ToBytes(b64) {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function formatTime(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00.0';
  const total = Math.floor(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  const frac = Math.floor((seconds - total) * 10);
  return `${m}:${String(s).padStart(2, '0')}.${frac}`;
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

/** Canvas waveform with a scrubbable playhead. */
function Waveform({ buffer, duration, position, onSeek }) {
  const canvasRef = useRef(null);
  const peaksRef = useRef(null);
  const bucketsRef = useRef(0);
  const draggingRef = useRef(false);

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas || !buffer) return;
    const dpr = window.devicePixelRatio || 1;
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    if (width <= 0 || height <= 0) return;
    if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) {
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
    }
    const buckets = Math.max(1, Math.floor(width));
    if (!peaksRef.current || bucketsRef.current !== buckets) {
      peaksRef.current = computePeaks(buffer, buckets);
      bucketsRef.current = buckets;
    }

    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    const played = tokenColor(canvas, '--primary', '#7c8cff');
    const pending = tokenColor(canvas, '--muted-foreground', '#8a8a8a');
    const mid = height / 2;
    const progressX = duration > 0 ? (position / duration) * width : 0;
    const peaks = peaksRef.current;

    for (let b = 0; b < buckets; b++) {
      const min = peaks[b * 2];
      const max = peaks[b * 2 + 1];
      const top = mid - max * mid * 0.92;
      const bottom = mid - min * mid * 0.92;
      ctx.fillStyle = b <= progressX ? played : pending;
      ctx.globalAlpha = b <= progressX ? 1 : 0.45;
      ctx.fillRect(b, Math.min(top, mid - 0.5), 1, Math.max(1, bottom - top));
    }

    ctx.globalAlpha = 1;
    ctx.fillStyle = played;
    ctx.fillRect(Math.min(width - 1, Math.max(0, progressX)), 0, 1, height);
  }, [buffer, duration, position]);

  useEffect(() => {
    peaksRef.current = null;
  }, [buffer]);

  useEffect(() => {
    draw();
  }, [draw]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
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
      onPointerMove={(e) => {
        if (draggingRef.current) seekFromEvent(e);
      }}
      onPointerUp={(e) => {
        draggingRef.current = false;
        try {
          e.currentTarget.releasePointerCapture(e.pointerId);
        } catch {
          // Already released.
        }
      }}
    />
  );
}

/**
 * Editor preview for audio files: decodes with Web Audio and plays through our
 * own transport, so sample rates the platform's <audio> element rejects
 * (clinical 2 kHz heart / lung recordings, for one) still play correctly.
 */
export default function AudioPreview({ tab }) {
  const [error, setError] = useState(null);
  const [size, setSize] = useState(null);
  const [info, setInfo] = useState(null);
  const [buffer, setBuffer] = useState(null);
  const [fallbackSrc, setFallbackSrc] = useState(null);
  const [playing, setPlaying] = useState(false);
  const [position, setPosition] = useState(0);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);

  const ctxRef = useRef(null);
  const gainRef = useRef(null);
  const sourceRef = useRef(null);
  const startedAtRef = useRef(0);
  const rafRef = useRef(0);

  const reloadVersion = useFileReloadVersion(tab.path);

  const stopSource = useCallback(() => {
    const source = sourceRef.current;
    sourceRef.current = null;
    if (source) {
      source.onended = null;
      try {
        source.stop();
      } catch {
        // Already stopped.
      }
      source.disconnect();
    }
    cancelAnimationFrame(rafRef.current);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let blobUrl = null;
    setError(null);
    setInfo(null);
    setBuffer(null);
    setFallbackSrc(null);
    setSize(null);
    setPosition(0);
    setPlaying(false);
    stopSource();

    invoke('read_file_base64', { path: tab.path })
      .then(async (res) => {
        if (cancelled) return;
        setSize(res.size);
        const bytes = base64ToBytes(res.data);
        if (!ctxRef.current) {
          const Ctor = window.AudioContext || window.webkitAudioContext;
          ctxRef.current = new Ctor();
          gainRef.current = ctxRef.current.createGain();
          gainRef.current.connect(ctxRef.current.destination);
        }
        try {
          const decoded = await decodeAudio(ctxRef.current, bytes.buffer);
          if (cancelled) return;
          setBuffer(decoded.buffer);
          setInfo(decoded);
        } catch (decodeError) {
          if (cancelled) return;
          // Last resort: hand the raw bytes to the platform element, which may
          // still handle a container our decoder doesn't know about.
          blobUrl = URL.createObjectURL(new Blob([bytes], { type: mimeFor(tab.path) }));
          setFallbackSrc(blobUrl);
          setError(`Could not decode this file (${String(decodeError.message || decodeError)}).`);
        }
      })
      .catch((e) => {
        if (cancelled) return;
        const msg = String(e || '');
        setError(
          msg.includes('too large')
            ? 'Audio file is larger than 100MB — preview unavailable.'
            : msg,
        );
      });

    return () => {
      cancelled = true;
      if (blobUrl) URL.revokeObjectURL(blobUrl);
      stopSource();
    };
  }, [tab.path, reloadVersion, stopSource]);

  useEffect(
    () => () => {
      stopSource();
      ctxRef.current?.close();
      ctxRef.current = null;
    },
    [stopSource],
  );

  useEffect(() => {
    if (gainRef.current) gainRef.current.gain.value = muted ? 0 : volume;
  }, [volume, muted]);

  const duration = info?.duration ?? 0;

  const tick = useCallback(() => {
    const ctx = ctxRef.current;
    if (!ctx || !sourceRef.current) return;
    setPosition(Math.min(duration, ctx.currentTime - startedAtRef.current));
    rafRef.current = requestAnimationFrame(tick);
  }, [duration]);

  const startAt = useCallback(
    (offset) => {
      const ctx = ctxRef.current;
      if (!ctx || !buffer) return;
      stopSource();
      if (ctx.state === 'suspended') ctx.resume();
      const source = ctx.createBufferSource();
      source.buffer = buffer;
      source.connect(gainRef.current);
      const from = Math.min(Math.max(0, offset), Math.max(0, duration - 0.01));
      source.start(0, from);
      startedAtRef.current = ctx.currentTime - from;
      sourceRef.current = source;
      source.onended = () => {
        if (sourceRef.current !== source) return;
        sourceRef.current = null;
        cancelAnimationFrame(rafRef.current);
        setPlaying(false);
        setPosition(duration);
      };
      setPlaying(true);
      rafRef.current = requestAnimationFrame(tick);
    },
    [buffer, duration, stopSource, tick],
  );

  const togglePlay = () => {
    if (playing) {
      const ctx = ctxRef.current;
      const at = ctx ? ctx.currentTime - startedAtRef.current : 0;
      stopSource();
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

  if (error && !fallbackSrc) {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  if (!buffer && !fallbackSrc) {
    return (
      <div className="flex h-full w-full items-center justify-center p-6">
        <Skeleton className="h-40 w-[32rem]" />
      </div>
    );
  }

  const toolbar = (
    <>
      <div className="truncate text-xs text-muted-foreground">{basename(tab.path)}</div>
      <div className="flex shrink-0 items-center gap-3 text-[11px] text-muted-foreground">
        {info && (
          <span>
            {info.sampleRate.toLocaleString()} Hz · {channelLabel(info.channelCount)}
            {info.bitsPerSample ? ` · ${info.bitsPerSample}-bit` : ''}
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
            <div className="min-w-0 flex-1 truncate text-sm text-foreground">
              {basename(tab.path)}
            </div>
          </div>

          {fallbackSrc ? (
            <>
              <audio src={fallbackSrc} controls className="w-full" />
              <p className="text-xs text-destructive">{error}</p>
            </>
          ) : (
            <>
              <Waveform buffer={buffer} duration={duration} position={position} onSeek={seek} />

              <div className="flex items-center gap-3">
                <Button size="icon-sm" variant="secondary" onClick={togglePlay} title="Play / pause">
                  {playing ? <Pause /> : <Play />}
                </Button>
                <Button size="icon-sm" variant="ghost" onClick={() => seek(0)} title="Back to start">
                  <SkipBack />
                </Button>
                <span className="tabular-nums text-xs text-muted-foreground">
                  {formatTime(position)} / {formatTime(duration)}
                </span>
                <div className="ml-auto flex items-center gap-2">
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    onClick={() => setMuted((m) => !m)}
                    title={muted ? 'Unmute' : 'Mute'}
                  >
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

              {info?.resampled && (
                <p className="text-[11px] text-muted-foreground">
                  Source runs at {info.sampleRate.toLocaleString()} Hz — resampled to{' '}
                  {Math.round(ctxRef.current?.sampleRate ?? 0).toLocaleString()} Hz for playback,
                  pitch and duration preserved.
                </p>
              )}
            </>
          )}
        </div>
      </div>
    </PreviewSurface>
  );
}
