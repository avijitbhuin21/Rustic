import { pen } from '@/lib/ink/pens'
import { defineRecipe } from '@/lib/ink/recipes'
import type { Pt } from '@/lib/ink/types'
import { RULER } from '@/lib/ink/shapes'

export type ScrollOrientation = 'vertical' | 'horizontal'

function along(o: ScrollOrientation, len: number, mid: number, inset: number): Pt[] {
  const end = inset + Math.max(1, len - 2 * inset)
  return o === 'vertical' ? [[mid, inset], [mid, end]] : [[inset, mid], [end, mid]]
}

export const scrollTrackRecipe = defineRecipe<{ orientation: ScrollOrientation }>({
  id: 'scroll-track',
  kind: 'track',
  layers: ['rest'],
  pad: 2,
  draw(ink, { w, h }, { orientation }) {
    const vertical = orientation === 'vertical'
    const len = vertical ? h : w
    const mid = (vertical ? w : h) / 2
    ink.layer('rest', () => {
      ink.stroke(along(orientation, len, mid, 2), 'pencil.fine', { ...RULER, alpha: 0.4 })
    })
  },
})

const LEAD = pen('pencil', { W: 3.6, dryAmt: 0.3, dryWl: 9, dens: 0.9, wob: { amp: 0.5, wl: 80, tr: 0.15 } })

export const scrollThumbRecipe = defineRecipe<{ orientation: ScrollOrientation }>({
  id: 'scroll-thumb',
  kind: 'thumb',
  layers: ['rest', 'hover'],
  pad: 1,
  draw(ink, { w, h }, { orientation }, ctx) {
    const vertical = orientation === 'vertical'
    const len = vertical ? h : w
    const mid = (vertical ? w : h) / 2
    ink.layer('rest', () => {
      ink.stroke(along(orientation, len, mid, 1), ctx.calm ? 'pencil.fine' : LEAD, { ...RULER, width: ctx.calm ? 1.5 : 1 })
    })
    if (!ctx.calm && len > 44) {
      ink.layer('rest', () => {
        for (const d of [-3.2, 3.2]) {
          const c = len / 2 + d
          const a: Pt = vertical ? [mid - 3.6, c] : [c, mid - 3.6]
          const b: Pt = vertical ? [mid + 3.6, c] : [c, mid + 3.6]
          ink.stroke([a, b], 'pencil.fine', { ...RULER, alpha: 0.85, simple: true })
        }
      })
    }
    ink.layer('hover', () => {
      ink.stroke(along(orientation, len, mid + 0.7, 2), pen('pencil', { W: 2, alpha: 0.7 }), RULER)
    })
  },
})
