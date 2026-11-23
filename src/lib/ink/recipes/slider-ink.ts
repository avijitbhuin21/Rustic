import { solidFill, wash } from '@/lib/ink/fills'
import { ellipsePoly, pillPoly, roundRectPoly } from '@/lib/ink/geom'
import { box, dashedBox, focusRing, loop } from '@/lib/ink/shapes'
import { defineRecipe } from '@/lib/ink/recipes'
import type { Ink } from '@/lib/ink/builder'
import type { Pt } from '@/lib/ink/types'

export const THUMB_INSET = 10

const along = (vertical: boolean, t: number, c: number): Pt => (vertical ? [c, t] : [t, c])

export interface SliderTrackParams {
  vertical?: boolean
  ticks?: number
  disabled?: boolean
  invalid?: boolean
}

export const sliderTrackRecipe = defineRecipe<SliderTrackParams>({
  id: 'slider-track',
  kind: 'slider',
  layers: ['rest'],
  pad: 12,
  draw(ink, { w, h }, { vertical = false, ticks = 0, disabled, invalid }, ctx) {
    const len = vertical ? h : w
    const th = vertical ? w : h
    const calm = ctx.calm
    const c0 = th * 0.12
    const c1 = th * 0.88
    ink.layer('rest', () => {
      const groove = vertical ? roundRectPoly(c0, 0, c1 - c0, len, 2) : roundRectPoly(0, c0, len, c1 - c0, 2)
      if (disabled) {
        wash(ink, groove, 'pencil-mark', { alpha: 0.3, material: 'wash' })
        if (vertical) dashedBox(ink, c0, 0, c1 - c0, len, { r: 2, pen: 'pencil', opts: { color: 'pencil', width: 1 } })
        else dashedBox(ink, 0, c0, len, c1 - c0, { r: 2, pen: 'pencil', opts: { color: 'pencil', width: 1 } })
      } else {
        const pill = vertical ? pillPoly(0.7, 0, th - 1.4, len) : pillPoly(0, 0.7, len, th - 1.4)
        solidFill(ink, pill, 'paper-shade', { alpha: 1, material: 'wash' })
        box(ink, vertical ? 0.7 : 0, vertical ? 0 : 0.7, vertical ? th - 1.4 : len, vertical ? len : th - 1.4, {
          pen: invalid ? (calm ? 'red.fine' : 'red') : 'ink.fine',
          corner: (th - 1.4) / 2,
          opts: { width: 1.15, simple: len > 500 },
        })
      }
      if (ticks > 0) {
        const inset = Math.min(THUMB_INSET, len / 4)
        for (let i = 0; i <= ticks; i++) {
          const t = inset + ((len - 2 * inset) * i) / ticks
          const major = i === 0 || i === ticks
          const from = th + 2.2
          const to = from + (major ? 7.5 : 5)
          ink.stroke([along(vertical, t, from), along(vertical, t + 0.3, to)], disabled ? 'pencil' : 'ink.fine', {
            width: major ? 1.1 : 0.95, alpha: disabled ? 0.6 : 0.9, start: 'soft', end: 'flick', wobble: 0.3, simple: true,
          })
        }
      }
    })
  },
})

export const sliderToneList = ['marker', 'hi', 'blue'] as const
export type SliderTone = (typeof sliderToneList)[number]


export interface SliderRangeParams {
  vertical?: boolean
  tone?: SliderTone
  disabled?: boolean
}

export const sliderRangeRecipe = defineRecipe<SliderRangeParams>({
  id: 'slider-range',
  kind: 'slider',
  layers: ['rest'],
  pad: 12,
  draw(ink, { w, h }, { vertical = false, tone = 'marker', disabled }) {
    const len = vertical ? h : w
    const th = vertical ? w : h
    ink.layer('rest', () => {
      const color = disabled ? 'pencil-mark' : tone === 'hi' ? 'hi' : tone === 'blue' ? 'ink-you' : 'marker'
      const pill = vertical ? pillPoly(1.6, 0, th - 3.2, len) : pillPoly(0, 1.6, len, th - 3.2)
      solidFill(ink, pill, color, { alpha: disabled ? 0.5 : 0.96 })
    })
  },
})

export interface SliderThumbParams {
  disabled?: boolean
  invalid?: boolean
}

export const sliderThumbRecipe = defineRecipe<SliderThumbParams>({
  id: 'slider-thumb',
  kind: 'thumb',
  layers: ['rest', 'hover', 'focus'],
  lazy: ['focus'],
  pad: 12,
  draw(ink: Ink, { w, h }, { disabled, invalid }, ctx) {
    const d = Math.min(w, h)
    const cx = w / 2
    const cy = h / 2
    const R = d / 2 - 1
    const calm = ctx.calm
    if (!disabled) ctx.lazy('focus', (f) => focusRing(f, 0, 0, w, h, { shape: 'ellipse' }))
    if (ctx.lazyOnly) return
    const disc = ellipsePoly(cx, cy, R + 0.4, R + 0.4, 24)
    ink.layer('rest', () => {
      if (disabled) {
        ink.poly('under', disc, 'slip', 1)
        dashedBox(ink, 0, 0, w, h, { r: d / 2, pen: 'pencil', opts: { color: 'pencil', width: 1.1 } })
        wash(ink, ellipsePoly(cx, cy, 2.6, 2.6, 12), 'pencil-mark', { alpha: 0.6, material: 'wash' })
        return
      }
      ink.poly('under', disc, 'slip', 1)
      loop(ink, cx, cy, R, R, { pen: invalid ? (calm ? 'red.fine' : 'red') : calm ? 'pencil.fine' : 'ink', clean: true })
    })
    if (!disabled) {
      ink.layer('hover', () => {
        if (calm) {
          loop(ink, cx, cy, R - 1.6, R - 1.6, { pen: 'pencil.fine', turns: 1.05, opts: { alpha: 0.75, simple: true } })
          return
        }
        solidFill(ink, ellipsePoly(cx, cy, R - 1.2, R - 1.2, 24), 'hi', { alpha: 0.55 })
      })
    }
  },
})
