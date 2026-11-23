import { solidFill, wash } from '@/lib/ink/fills'
import { pillPoly } from '@/lib/ink/geom'
import { dashedBox, focusRing, loop, RULER } from '@/lib/ink/shapes'
import { defineRecipe } from '@/lib/ink/recipes'
import type { Ink } from '@/lib/ink/builder'
import type { PenName, Pt } from '@/lib/ink/types'

export interface SwitchParams {
  disabled?: boolean
  invalid?: boolean
}

function pillLine(x: number, y: number, w: number, h: number): Pt[] {
  const r = h / 2
  const pts: Pt[] = [[x + w / 2, y]]
  const end = (cx: number, a0: number): void => {
    for (let i = 0; i <= 12; i++) {
      const a = a0 + (i / 12) * Math.PI
      pts.push([cx + Math.cos(a) * r, y + r + Math.sin(a) * r])
    }
  }
  end(x + w - r, -Math.PI / 2)
  end(x + r, Math.PI / 2)
  pts.push([x + w / 2 + Math.min(3, w * 0.08), y])
  return pts
}

function liftLine(w: number, h: number, d: number): Pt[] {
  const r = (h - 1) / 2
  const cx = w - 0.5 - r + d
  const cy = 0.5 + r + d
  const pts: Pt[] = [[0.5 + r + d + 2, cy + r], [cx, cy + r]]
  for (let i = 1; i <= 12; i++) {
    const a = Math.PI / 2 - (i / 12) * (Math.PI / 2 + 0.75)
    pts.push([cx + Math.cos(a) * r, cy + Math.sin(a) * r])
  }
  return pts
}

export const switchRecipe = defineRecipe<SwitchParams>({
  id: 'switch',
  kind: 'switch',
  layers: ['rest', 'hover', 'checked', 'focus'],
  lazy: ['focus'],
  pad: 12,
  draw(ink, { w, h }, { disabled, invalid }, ctx) {
    const r = h / 2
    const calm = ctx.calm
    const body = pillPoly(0.5, 0.5, w - 1, h - 1)

    if (!disabled) ctx.lazy('focus', (f) => focusRing(f, 0, 0, w, h, { r }))
    if (ctx.lazyOnly) return

    const outline = (width?: number): void => {
      const pen: PenName = invalid ? (calm ? 'red.fine' : 'red') : calm ? 'pencil.fine' : 'ink'
      ink.stroke(pillLine(0.5, 0.5, w - 1, h - 1), pen, { ...RULER, width, simple: true })
    }

    ink.layer('rest', () => {
      if (disabled) {
        wash(ink, body, 'pencil-mark', { alpha: 0.35, material: 'wash' })
        dashedBox(ink, 0, 0, w, h, { r, pen: 'pencil', opts: { color: 'pencil', width: 1.2 } })
        return
      }
      ink.poly('under', body, 'slip', 0.85)
      outline(invalid ? 1.2 : undefined)
    })

    if (!disabled) {
      ink.layer('hover', () => {
        if (calm) ink.stroke(pillLine(2, 2, w - 4, h - 4), invalid ? 'red.fine' : 'pencil.fine', { ...RULER, alpha: 0.75, simple: true })
        else ink.stroke(liftLine(w, h, 2 + h / 24), 'ink.fine', { ...RULER, width: 1.1 })
      })
    }

    ink.layer('checked', () => {
      if (disabled) {
        wash(ink, body, 'pencil-mark', { alpha: 0.4, material: 'wash' })
        return
      }
      ink.part('fill', () => solidFill(ink, body, 'marker', { alpha: 0.94 }))
      if (invalid) outline(1.3)
    })
  },
})

export interface SwitchThumbParams {
  disabled?: boolean
}

export const switchThumbRecipe = defineRecipe<SwitchThumbParams>({
  id: 'switch-thumb',
  kind: 'thumb',
  layers: ['rest'],
  pad: 6,
  draw(ink: Ink, { w, h }, { disabled }, ctx) {
    const d = Math.min(w, h)
    const cx = w / 2
    const cy = h / 2
    const rad = d / 2 - 0.8
    ink.layer('rest', () => {
      if (disabled) {
        dashedBox(ink, 0, 0, w, h, { r: d / 2, pen: 'pencil', opts: { color: 'pencil', width: 1.1 } })
        return
      }
      const disc: Pt[] = []
      for (let i = 0; i < 24; i++) disc.push([cx + Math.cos((i / 24) * Math.PI * 2) * (rad + 0.4), cy + Math.sin((i / 24) * Math.PI * 2) * (rad + 0.4)])
      ink.poly('under', disc, 'slip', 1)
      loop(ink, cx, cy, rad, rad, { pen: ctx.calm ? 'pencil.fine' : d < 14 ? 'ink.fine' : 'ink', clean: true, opts: { simple: true } })
    })
  },
})
