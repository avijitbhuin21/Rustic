import { defineRecipe } from '@/lib/ink/recipes'
import { line } from '@/lib/ink/shapes'

export const commandInputRecipe = defineRecipe<Record<string, never>>({
  id: 'command-input',
  kind: 'rule',
  layers: ['rest', 'focus', 'disabled'],
  lazy: ['focus'],
  pad: 12,
  draw(ink, { w, h }, _params, ctx) {
    ctx.lazy('focus', (f) => {
      line(f, 2, h - 0.5, w - 4, h - 1.1, { pen: 'blue.brush', opts: { alpha: 0.95, simple: true } })
    })
    if (ctx.lazyOnly) return
    ink.layer('rest', () => {
      if (ctx.calm) {
        line(ink, 0, h - 0.5, w, h - 0.5, { pen: 'pencil.fine', opts: { simple: true } })
        return
      }
      line(ink, -1, h - 0.6, w + 1.5, h - 1.3, { pen: 'ink.fine', opts: { alpha: 0.9, simple: true } })
      line(ink, 6, h + 2.6, w - 10, h + 2.1, { pen: 'pencil.fine', opts: { alpha: 0.55, simple: true } })
    })
  },
})
