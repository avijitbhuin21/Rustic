import { defineRecipe } from '@/lib/ink/recipes'
import { box } from '@/lib/ink/shapes'

export const hoverCardMediaRecipe = defineRecipe<Record<string, never>>({
  id: 'hover-card-media',
  kind: 'frame',
  layers: ['rest'],
  pad: 6,
  draw(ink, { w, h }, _params, ctx) {
    ink.layer('rest', () => {
      box(ink, 1, 1, w - 2, h - 2, { pen: ctx.calm ? 'pencil.fine' : 'pencil', corner: 2 })
    })
  },
})
