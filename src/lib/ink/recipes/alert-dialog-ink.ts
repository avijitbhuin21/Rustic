import { blobFill } from '@/lib/ink/fills'
import { ellipsePoly } from '@/lib/ink/geom'
import { defineRecipe } from '@/lib/ink/recipes'
import { loop } from '@/lib/ink/shapes'
import type { PaintKey } from '@/lib/ink/paint'

export type AlertMediaTone = 'default' | 'danger' | 'info'

export interface AlertMediaParams {
  tone: AlertMediaTone
}

const FILL: Record<AlertMediaTone, PaintKey> = { default: 'hi', danger: 'tomato', info: 'sky' }

export const alertMediaRecipe = defineRecipe<AlertMediaParams>({
  id: 'alert-media',
  kind: 'plate',
  layers: ['rest'],
  pad: 8,
  draw(ink, { w, h }, { tone }, ctx) {
    const cx = w / 2
    const cy = h / 2
    ink.layer('rest', () => {
      ink.part('fill', () => blobFill(ink, ellipsePoly(cx, cy, w * 0.43, h * 0.42, 30, ink.next('plate').range(-0.3, 0.3)), FILL[tone], { alpha: tone === 'danger' ? 0.34 : 0.62 }))
      loop(ink, cx, cy, w * 0.45, h * 0.44, { pen: ctx.calm ? 'pencil.fine' : tone === 'danger' ? 'redfelt.fine' : 'ink.fine' })
    })
  },
})
