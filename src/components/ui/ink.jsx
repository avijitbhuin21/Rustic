// Hand-drawn outlines (theme `style.ink`). Off by default: every helper here
// renders nothing and loads nothing until a theme turns it on.
import * as React from "react"
import { create } from "zustand"

import { startInkPainter, stopInkPainter, repaintInk } from "@/lib/ink-painter"

const useInkStore = create(() => ({ enabled: false, still: false, kit: null }))

let loading = null

/** Turn hand-drawn outlines on or off; the ink engine loads on first use. */
function setInk({ enabled, still = false }) {
  useInkStore.setState({ enabled, still })
  if (!enabled) {
    stopInkPainter()
    return
  }
  const { kit } = useInkStore.getState()
  if (kit) {
    startInkPainter(kit)
    repaintInk()
    return
  }
  loading ??= import("@/lib/ink-kit")
    .then((loaded) => {
      useInkStore.setState({ kit: loaded })
      if (useInkStore.getState().enabled) startInkPainter(loaded)
    })
    .catch((e) => {
      loading = null
      console.error("ink: engine failed to load", e)
    })
}

/** The loaded ink kit while outlines are on, otherwise null. */
function useInkKit() {
  return useInkStore((s) => (s.enabled ? s.kit : null))
}

/**
 * Draws `recipe` behind its parent element (which becomes the ink host). Must
 * be a direct child of a non-void, non-scrolling element.
 */
function InkLayer({ recipe, params, seed }) {
  const kit = useInkKit()
  const still = useInkStore((s) => s.still)
  const def = kit?.RECIPES[recipe]
  if (!def) return null
  const Ink = kit.Ink
  return <Ink recipe={def} params={params} seed={seed} still={still} />
}

/** A small decorative doodle; renders nothing unless outlines are on. */
function Doodle({ name, size = 48, seed = name, className }) {
  const kit = useInkKit()
  const art = React.useMemo(() => (kit ? kit.drawDoodle(name, size, seed) : null), [kit, name, size, seed])
  // Doodle strokes reference the engine's shared grain masks.
  React.useEffect(() => {
    if (art) kit.ensureInkDefs()
  }, [art, kit])
  if (!art) return null
  const { w, h, pad } = art
  return (
    <span
      data-ink-doodle={name}
      aria-hidden="true"
      className={className}
      style={{ display: "inline-block", width: w, height: h, lineHeight: 0 }}>
      <svg
        viewBox={`${-pad} ${-pad} ${w + 2 * pad} ${h + 2 * pad}`}
        width={w + 2 * pad}
        height={h + 2 * pad}
        style={{ margin: -pad, overflow: "visible" }}
        dangerouslySetInnerHTML={{ __html: art.svg }}
      />
    </span>
  )
}

/** A stable 0..1 value for `text`, so the same words always sit the same way. */
function unit(text) {
  let h = 2166136261
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619)
  return ((h >>> 0) % 10000) / 10000
}

/**
 * Sets each word of its text a hair off the baseline, like a line written by
 * hand. For titles only; renders its children untouched unless outlines are on.
 */
function Written({ children }) {
  const on = useInkStore((s) => s.enabled && !s.still)
  if (!on) return children
  return React.Children.map(children, (node) => {
    if (typeof node !== "string" || !node.trim()) return node
    return (
      <span data-ink-written="">
        {node.split(/(\s+)/).map((part, i) => {
          if (!part.trim()) return part
          const key = `${i}|${part}`
          const style = {
            "--jy": `${((unit(key) - 0.5) * 2.4).toFixed(2)}px`,
            "--jr": `${((unit(`${key}r`) - 0.5) * 2.6).toFixed(2)}deg`,
          }
          return <span key={i} data-ink-word="" style={style}>{part}</span>
        })}
      </span>
    )
  })
}

export { InkLayer, Doodle, Written, setInk, useInkKit }
