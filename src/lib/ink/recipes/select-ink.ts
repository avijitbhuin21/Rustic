import { defineRecipe } from '@/lib/ink/recipes'
import { drawSelectFrame } from '@/lib/ink/recipes/select-frame-ink'

export interface SelectParams {
  disabled?: boolean
  invalid?: boolean
}

export const selectRecipe = defineRecipe<SelectParams>({
  id: 'select',
  kind: 'field',
  layers: ['rest', 'hover', 'focus', 'open', 'invalid', 'disabled'],
  lazy: ['focus', 'hover'],
  pad: 12,
  draw(ink, box, params, ctx) {
    drawSelectFrame(ink, box, { ...params, openable: true }, ctx)
  },
})
