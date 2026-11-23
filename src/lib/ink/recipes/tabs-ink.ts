import { flatFill, wash } from '@/lib/ink/fills'
import { defineRecipe } from '@/lib/ink/recipes'
import { focusRing } from '@/lib/ink/shapes'
import type { Ink } from '@/lib/ink/builder'
import type { PaintKey } from '@/lib/ink/paint'
import type { Pt } from '@/lib/ink/types'

export interface TabsParams {
  variant: 'default' | 'line'
  orientation: 'horizontal' | 'vertical'
  disabled?: boolean
}

const R = 5.5
const HAIR = 1.2
const EDGE = 1.6
const REACH = 2
const LIFT = 2

function tabEdge(w: number, h: number, vertical: boolean, rtl: boolean, grow: number): Pt[] {
  const r = Math.max(1, Math.min(R, (vertical ? h : w) / 2 - 1))
  const out: Pt[] = []
  const arc = (cx: number, cy: number, a0: number, a1: number) => {
    for (let i = 0; i <= 8; i++) {
      const a = a0 + ((a1 - a0) * i) / 8
      out.push([cx + r * Math.cos(a), cy + r * Math.sin(a)])
    }
  }
  const x0 = 0.6, y0 = 0.6
  if (vertical) {
    const y1 = h - 0.6
    out.push([w + grow, y0])
    arc(x0 + r, y0 + r, 1.5 * Math.PI, Math.PI)
    arc(x0 + r, y1 - r, Math.PI, 0.5 * Math.PI)
    out.push([w + grow, y1])
  } else {
    const x1 = w - 0.6
    out.push([x0, h + grow])
    arc(x0 + r, y0 + r, Math.PI, 1.5 * Math.PI)
    arc(x1 - r, y0 + r, 1.5 * Math.PI, 2 * Math.PI)
    out.push([x1, h + grow])
  }
  return rtl ? out.map(([x, y]) => [w - x, y] as Pt) : out
}

function ribbon(pts: Pt[], width: number): Pt[] {
  const hw = width / 2
  const left: Pt[] = [], right: Pt[] = []
  for (let i = 0; i < pts.length; i++) {
    const a = pts[Math.max(0, i - 1)], b = pts[Math.min(pts.length - 1, i + 1)]
    const tx = b[0] - a[0], ty = b[1] - a[1]
    const l = Math.hypot(tx, ty) || 1
    const nx = -ty / l, ny = tx / l
    left.push([pts[i][0] + nx * hw, pts[i][1] + ny * hw])
    right.push([pts[i][0] - nx * hw, pts[i][1] - ny * hw])
  }
  return [...left, ...right.reverse()]
}

function rule(ink: Ink, pts: Pt[], color: PaintKey, width: number, alpha = 1) {
  flatFill(ink, ribbon(pts, width), color, alpha, 'text')
}

export const tabsRecipe = defineRecipe<TabsParams>({
  id: 'tabs',
  kind: 'tab',
  layers: ['rest', 'hover', 'checked', 'focus', 'disabled'],
  lazy: ['focus'],
  pad: 12,
  draw(ink, { w, h }, { variant, orientation, disabled }, ctx) {
    const vertical = orientation === 'vertical'
    const rtl = ctx.dir === 'rtl'
    const base = (color: PaintKey, width = HAIR, alpha = 1) => {
      if (vertical) {
        const x = rtl ? 0 : w
        rule(ink, [[x, -REACH], [x, h + REACH]], color, width, alpha)
      } else rule(ink, [[-REACH, h], [w + REACH, h]], color, width, alpha)
    }

    if (!disabled) ctx.lazy('focus', (f) => focusRing(f, 0, 0, w, h, { r: R }))
    if (ctx.lazyOnly) return

    if (disabled) {
      ink.layer('disabled', () => {
        base('pencil-mark', HAIR, 0.6)
        if (variant === 'default') rule(ink, tabEdge(w, h, vertical, rtl, 0), 'pencil-mark', HAIR, 0.45)
      })
      return
    }

    if (variant === 'line') {
      ink.layer('rest', () => base('pencil-mark'))
      ink.layer('hover', () => base('pencil'))
      ink.layer('checked', () => {
        ink.part(
          'swipe',
          () => {
            if (vertical) {
              const x = rtl ? 1.4 : w - 1.4
              rule(ink, [[x, 3], [x, h - 3]], 'ink-you', 2.4)
            } else rule(ink, [[3, h - 1.4], [w - 3, h - 1.4]], 'ink-you', 2.4)
          },
          'top',
        )
      })
      return
    }

    const outline = tabEdge(w, h, vertical, rtl, 0)
    ink.layer('rest', () => {
      ink.group({ hideOn: 'checked' }, () => {
        base('pencil-mark')
        rule(ink, outline, 'pencil-mark', HAIR)
      }, 'text')
    })
    ink.layer('hover', () => {
      wash(ink, outline, 'hi', { alpha: 0.35 })
      rule(ink, outline, 'pencil', HAIR)
    })
    ink.layer('checked', () => {
      const lifted = tabEdge(w, h, vertical, rtl, vertical ? -LIFT : LIFT)
      ink.part('fill', () => flatFill(ink, lifted, 'hi', 0.92))
      rule(ink, lifted, 'ink', EDGE)
    })
  },
})
