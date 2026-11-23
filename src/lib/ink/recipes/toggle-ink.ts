import { bandFill, blobFill, wash } from '@/lib/ink/fills'
import { roundRectPoly } from '@/lib/ink/geom'
import { clamp } from '@/lib/ink/rng'
import { box, chain, dashedBox, focusRing, RULER } from '@/lib/ink/shapes'
import { defineRecipe } from '@/lib/ink/recipes'

export const toggleVariantList = ['default', 'outline'] as const
export const toggleToneList = ['hi', 'marker'] as const
export type ToggleVariant = (typeof toggleVariantList)[number]
export type ToggleTone = (typeof toggleToneList)[number]

export interface ToggleParams {
  variant: ToggleVariant
  disabled?: boolean
  tone?: ToggleTone
  joined?: boolean
  vertical?: boolean
  invalid?: boolean
}

export const toggleRecipe = defineRecipe<ToggleParams>({
  id: 'toggle',
  kind: 'toggle',
  layers: ['rest', 'hover', 'checked', 'focus'],
  lazy: ['focus'],
  pad: 12,
  draw(ink, { w, h }, { variant, disabled, tone = 'hi', joined, vertical, invalid: invalidParam }, ctx) {
    const invalid = !!invalidParam && !disabled && !joined
    const r = clamp(Math.min(w, h) * 0.09, 2.2, 4.4)
    const inset = joined ? 0.9 : 0.5
    const body = roundRectPoly(inset, inset, w - 2 * inset, h - 2 * inset, joined ? 2 : r)
    const fillOpts = { ends: 'square' as const, W: clamp(Math.min(w, h) * 0.3, 5, 9), angle: w / h < 1.6 ? -0.04 : -0.02 }
    const small = Math.min(w, h) < 28
    const rtl = ctx.dir === 'rtl'
    const calm = ctx.calm

    if (!disabled) ctx.lazy('focus', (fink) => focusRing(fink, 0, 0, w, h, { r }))
    if (ctx.lazyOnly) return

    const wide = w > 360
    const disabledBox = () => dashedBox(ink, 0, 0, w, h, { r, pen: 'pencil', opts: { color: 'pencil', width: 1.2 } })
    const inkBox = (o: { inset?: number; alpha?: number; whisper?: boolean } = {}) => {
      const i = o.inset ?? 0
      const fine = calm || o.whisper
      const pen = invalid ? (fine ? 'red.fine' : 'red') : fine ? 'pencil.fine' : 'ink'
      const alpha = invalid ? undefined : o.alpha
      box(ink, i, i, w - 2 * i, h - 2 * i, {
        pen,
        corner: r,
        ...(alpha !== undefined || wide || calm || invalid
          ? { opts: { alpha, simple: wide || (calm && i > 0) || undefined, ...(invalid ? { width: 1.2 } : {}) } }
          : {}),
      })
    }

    ink.layer('rest', () => {
      if (joined) {
        ink.part(
          'seam',
          () => {
            const pen = disabled || variant === 'default' || calm ? 'pencil.fine' : 'ink.fine'
            const o = { ...RULER, alpha: disabled ? 0.55 : variant === 'default' ? 0.42 : 1 }
            if (vertical) ink.lines([{ pts: [[w * 0.06, 0.4], [w * 0.94, 0.4]], o }], pen)
            else {
              const x = rtl ? w - 0.4 : 0.4
              ink.lines([{ pts: [[x, h * 0.05], [x, h * 0.95]], o }], pen)
            }
            return { dx: 0, dy: 0 }
          },
          'top'
        )
        return
      }
      if (disabled) {
        if (variant === 'outline') wash(ink, body, 'pencil-mark', { alpha: 0.12, material: 'wash' })
        disabledBox()
      } else if (variant === 'outline') inkBox()
      else inkBox({ inset: 0.6, alpha: 0.4, whisper: true })
    })

    if (!disabled) {
      ink.layer('hover', () => {
        if (variant === 'default' && !joined) inkBox({ inset: 0.6, alpha: 0.6, whisper: true })
        else wash(ink, body, 'hi', { alpha: joined || variant === 'default' ? 0.34 : 0.5 })
        if (calm && !joined) {
          box(ink, 1, 1, w - 2, h - 2, {
            pen: 'pencil.fine',
            corner: r,
            loose: 0.6,
            opts: { alpha: 0.7, simple: true },
          })
          return
        }
        if (calm || joined || variant === 'default') return
        const d = 3.4 + h / 40
        ink.lines(
          [
            { pts: chain([[r + d + 2, h + d], [w + d, h + d], [w + d, r + d + 2]], [0.25, -0.2], Math.max(2.5, r + 1), ink.next('drop')), o: { start: 'soft', end: 'soft', wobble: 0.25, dry: 0 } },
          ],
          'ink.fine',
          { width: 1.3 }
        )
      })
    }

    ink.layer('checked', () => {
      if (disabled) {
        wash(ink, body, 'pencil-mark', { alpha: 0.35, material: 'wash' })
        return
      }
      const alpha = tone === 'marker' ? 0.86 : 0.82
      const mis = joined ? 1.3 : clamp(h * 0.06, 1.6, 3)
      ink.part('fill', () =>
        small ? blobFill(ink, body, tone, { alpha, mis }) : bandFill(ink, body, tone, { alpha, mis, passes: tone === 'marker' && !calm ? 2 : 1, ...fillOpts })
      )
      if (invalid) {
        box(ink, 0, 0, w, h, { pen: calm ? 'red.fine' : 'red', corner: r, opts: { width: 1.3, simple: wide || undefined } })
        return
      }
      if (variant === 'default' && !joined) {
        if (calm) box(ink, 0, 0, w, h, { pen: 'pencil.fine', corner: r, opts: { simple: wide || undefined } })
        else box(ink, 0, 0, w, h, { pen: 'ink.fine', corner: r, opts: wide ? { simple: true } : undefined })
      }
    })
  },
})
