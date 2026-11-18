import React, { useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';
import { VIEWS } from './mascot/views';
import { PixelMascot } from './mascot/pixel-mascot';

const SLEEP_AFTER_MS = 15000;
const STATE_LABELS = {
  idle: 'Ready', thinking: 'Thinking', tool: 'Running a tool', waiting: 'Waiting on sub-agents',
  error: 'Something went wrong', done: 'Done', sleep: 'Napping',
  asking: 'Waiting for your answer', listening: 'Listening', condensing: 'Condensing context', retry: 'Retrying',
};
// The pixel fallback predates these poses.
const PIXEL_STATE = { asking: 'idle', listening: 'idle', condensing: 'thinking', retry: 'thinking' };
const now = () => performance.now() / 1000;

/**
 * Animated 3D agent mascot (fur shader over a Blender-built model, rendered by
 * the shared engine in ./mascot/scene). `state` drives the pose and idle naps on
 * its own. Changing `petKey` pets it from outside; `interactive={false}` lets a
 * wrapping button own the click. Falls back to the pixel mascot without WebGL.
 */
export function AgentMascot({ state = 'idle', size = 'hero', subagents = 0, petKey = 0, interactive = true, className, style }) {
  const canvasRef = useRef(null);
  const viewRef = useRef(null);
  const simRef = useRef(null);
  if (!simRef.current) {
    simRef.current = {
      state, stateStart: now(), petAt: -99, wakeAt: -99, subagents,
      lookX: 0, lookY: 0, pointerFresh: false, hasPointer: false, lastPointer: performance.now(),
    };
  }
  const [shown, setShown] = useState(state);
  const [failed, setFailed] = useState(false);
  const cfg = VIEWS[size] || VIEWS.hero;

  useEffect(() => {
    const sim = simRef.current;
    sim.subagents = subagents;
    if (sim.state !== state && !(sim.state === 'sleep' && state === 'idle')) {
      const t = now();
      if (sim.state === 'sleep') sim.wakeAt = t;
      sim.state = state;
      sim.stateStart = t;
      sim.lastPointer = performance.now();
      setShown(state);
    }
    viewRef.current?.invalidate();
  }, [state, subagents]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return undefined;
    const sim = simRef.current;
    const reduced = !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    let disposed = false;

    const wake = (t) => {
      sim.state = 'idle';
      sim.wakeAt = t;
      sim.stateStart = t;
      setShown('idle');
    };
    const onTick = (t) => {
      const idleMs = performance.now() - sim.lastPointer;
      sim.pointerFresh = sim.hasPointer && idleMs < 3000;
      if (!reduced && sim.state === 'idle' && idleMs > SLEEP_AFTER_MS) {
        sim.state = 'sleep';
        sim.stateStart = t;
        setShown('sleep');
      }
    };

    import('./mascot/scene')
      .then(({ getMascotEngine }) => getMascotEngine())
      .then((engine) => {
        if (disposed) return;
        viewRef.current = engine.createView({
          canvas, size, sim, onTick, reduced, host: canvas.parentElement || document.body,
        });
      })
      .catch((err) => {
        console.warn('[mascot] 3D renderer unavailable, using the pixel mascot:', err);
        if (!disposed) setFailed(true);
      });

    const onPointer = (e) => {
      const r = canvas.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height * 0.6;
      sim.lookX = Math.max(-1, Math.min(1, (e.clientX - cx) / 140));
      sim.lookY = Math.max(-1, Math.min(1, (e.clientY - cy) / 110));
      sim.hasPointer = true;
      sim.lastPointer = performance.now();
      if (sim.state === 'sleep') wake(now());
    };
    window.addEventListener('pointermove', onPointer, { passive: true });

    return () => {
      disposed = true;
      viewRef.current?.dispose();
      viewRef.current = null;
      window.removeEventListener('pointermove', onPointer);
    };
  }, [size]);

  const pet = () => {
    const sim = simRef.current;
    const t = now();
    sim.petAt = t;
    sim.lastPointer = performance.now();
    if (sim.state === 'sleep') {
      sim.state = 'idle';
      sim.wakeAt = t;
      sim.stateStart = t;
      setShown('idle');
    }
  };

  useEffect(() => {
    if (petKey) pet();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [petKey]);

  if (failed) {
    return (
      <PixelMascot
        state={PIXEL_STATE[state] || state}
        size={size}
        subagents={subagents}
        petKey={petKey}
        interactive={interactive}
        className={className}
        style={style}
      />
    );
  }

  return (
    <canvas
      ref={canvasRef}
      onClick={interactive ? pet : undefined}
      role="img"
      aria-label={`Agent: ${STATE_LABELS[shown] || shown}`}
      title={interactive ? STATE_LABELS[shown] || shown : undefined}
      className={cn('shrink-0 select-none', interactive ? 'cursor-pointer' : 'pointer-events-none', className)}
      style={{ width: cfg.w, height: cfg.h, ...style }}
    />
  );
}

const TERMINAL_FAIL = new Set(['failed', 'error']);
const TERMINAL_STOP = new Set(['cancelled', 'canceled', 'aborted']);

/**
 * Maps live agent run signals to a mascot pose, with a brief done/error beat
 * after each run. `needsInput`, `retrying` and `condensing` override the run pose.
 */
export function useMascotRunState({
  taskId, isStreaming, runningTool, waitingOnSubagents, status,
  needsInput = false, retrying = false, condensing = false,
}) {
  const base = waitingOnSubagents > 0 ? 'waiting' : isStreaming ? (runningTool ? 'tool' : 'thinking') : 'idle';
  const busy = base !== 'idle';
  const [beat, setBeat] = useState(null);
  const prevRef = useRef({ taskId, busy });
  const norm = String(status || '').toLowerCase();

  useEffect(() => {
    const prev = prevRef.current;
    prevRef.current = { taskId, busy };
    if (prev.taskId !== taskId) { setBeat(null); return; }
    if (busy) { setBeat(null); return; }
    if (prev.busy && !TERMINAL_STOP.has(norm)) {
      setBeat({ kind: TERMINAL_FAIL.has(norm) ? 'error' : 'done', at: Date.now() });
    }
  }, [taskId, busy, norm]);

  // The terminal status can land a moment after streaming stops; upgrade the beat if so.
  useEffect(() => {
    if (!beat || Date.now() - beat.at > 1500) return;
    if (TERMINAL_STOP.has(norm)) setBeat(null);
    else if (TERMINAL_FAIL.has(norm) && beat.kind !== 'error') setBeat({ kind: 'error', at: beat.at });
  }, [norm, beat]);

  useEffect(() => {
    if (!beat) return undefined;
    const id = setTimeout(() => setBeat(null), beat.kind === 'error' ? 5000 : 3200);
    return () => clearTimeout(id);
  }, [beat]);

  if (needsInput) return 'asking';
  if (retrying) return 'retry';
  if (condensing) return 'condensing';
  return beat ? beat.kind : base;
}

export default AgentMascot;
