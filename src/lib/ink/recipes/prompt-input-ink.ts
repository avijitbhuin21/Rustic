import { line, RULER, underline } from '@/lib/ink/shapes'
import { defineRecipe } from '@/lib/ink/recipes'

export const promptInputBodyRecipe = defineRecipe<{ disabled?: boolean }>({
  id: 'prompt-input-body',
  kind: 'field',
  layers: ['rest', 'focus', 'disabled'],
  lazy: ['focus'],
  pad: 8,
  draw(ink, { w, h }, { disabled }, ctx) {
    const y = h - 3
    const x0 = 2
    const x1 = w - 2
    if (disabled) {
      if (ctx.lazyOnly) return
      ink.layer('disabled', () => {
        const count = Math.max(1, Math.round((x1 - x0 + 5) / 12))
        const step = (x1 - x0 + 5) / count
        for (let i = 0; i < count; i++) ink.stroke([[x0 + i * step, y], [x0 + i * step + step - 5, y]], 'pencil', { ...RULER, simple: true, width: 0.9, color: 'pencil-mark' })
      })
      return
    }
    ctx.lazy('focus', (f) => underline(f, x0 - 1, y + 0.6, x1 - x0 + 2, { pen: 'blue.brush', kind: 'single' }))
    if (ctx.lazyOnly) return
    ink.layer('rest', () => {
      if (ctx.calm) line(ink, x0, y, x1, y, { pen: 'pencil.fine', opts: { alpha: 0.7, wobble: 0.2 } })
      else line(ink, x0 - 2, y, x1 + 2, y, { pen: 'pencil' })
    })
  },
})
