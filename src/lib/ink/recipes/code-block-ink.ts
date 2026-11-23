import { focusRing, line } from '@/lib/ink/shapes'
import { defineRecipe } from '@/lib/ink/recipes'

export const codeRuleRecipe = defineRecipe<Record<string, never>>({
  id: 'code-block-rule',
  kind: 'rule',
  layers: ['rest'],
  pad: 6,
  draw(ink, { w, h }, _params, ctx) {
    const y = h - 1.5
    const x0 = Math.min(14, w * 0.04)
    const x1 = w - x0
    ink.layer('rest', () => {
      if (ctx.calm) {
        line(ink, x0, y, x1, y, { pen: 'pencil.fine', opts: { alpha: 0.7, wobble: 0.2 } })
        return
      }
      const dash = 7
      const gap = 4.5
      for (let x = x0; x < x1 - dash; x += dash + gap) {
        ink.stroke([[x, y], [x + dash, y]], 'pencil', { simple: true, raw: true, poly: true, start: 'soft', end: 'soft', width: 0.9 })
      }
    })
  },
})

export const codeBodyRecipe = defineRecipe<Record<string, never>>({
  id: 'code-block-body',
  kind: 'code',
  layers: ['focus'],
  lazy: ['focus'],
  pad: 12,
  draw(_ink, { w, h }, _params, ctx) {
    ctx.lazy('focus', (f) => focusRing(f, 2, 2, w - 4, h - 4, { r: 3 }))
  },
})
