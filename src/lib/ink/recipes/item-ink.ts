import { EDGE_LEN, EDGE_TH, edgeTile } from '@/lib/ink/recipes/pencil-tiles-ink'

export const ITEM_FRAMES = 3

let cache: string | undefined

const LAYER_ORDER = '@layer theme,base,components,utilities;'

export function itemCss(): string {
  return (cache ??= build())
}

function build(): string {
  const t = EDGE_TH
  const end = `${t / 2 - 0.5}px`
  const span = `calc(100% - ${t - 1}px)`
  const frames = Array.from({ length: ITEM_FRAMES }, (_, f) => ({
    eh: edgeTile(`item|edge-h|${f}`, false),
    ev: edgeTile(`item|edge-v|${f}`, true),
    ru: edgeTile(`item|rule|${f}`, false),
  }))
  const vars = frames.map((f, i) => `--ll-it-eh${i}:${f.eh};--ll-it-ev${i}:${f.ev};--ll-it-ru${i}:${f.ru};`).join('')
  const v = (kind: 'eh' | 'ev' | 'ru', f: number) => `var(--ll-it-${kind}${f})`
  const outline = (f: number) => `${v('eh', f)},${v('eh', f)},${v('ev', f)},${v('ev', f)}`
  const both = (prop: string, value: string) => `-webkit-${prop}:${value};${prop}:${value};`
  const pick = frames
    .map((_, f) => `[data-slot=item][data-frame="${f}"],[data-slot=item-media][data-frame="${f}"],[data-slot=item-separator][data-frame="${f}"]{--ll-it-m:${outline(f)};--ll-it-r:${v('ru', f)}}`)
    .join('')
  return (
    `${LAYER_ORDER}@layer components{` +
    `[data-slot=item],[data-slot=item-media],[data-slot=item-separator]{${vars}--ll-it-m:${outline(0)};--ll-it-r:${v('ru', 0)}}` +
    pick +
    `[data-slot=item],[data-slot=item-media][data-variant=icon]{position:relative;isolation:isolate}` +
    `[data-slot=item][data-variant=outline]::before,[data-slot=item-media][data-variant=icon]::before{content:"";position:absolute;inset:-${t / 2}px;z-index:-1;pointer-events:none;background:var(--ll-pencil);opacity:.8;` +
    both('mask-image', 'var(--ll-it-m)') +
    both('mask-size', `${span} ${t}px,${span} ${t}px,${t}px ${span},${t}px ${span}`) +
    both('mask-position', `${end} 0,${end} 100%,0 ${end},100% ${end}`) +
    both('mask-repeat', 'no-repeat') +
    `}` +
    `[data-slot=item-media][data-variant=icon]{background-color:color-mix(in srgb,var(--ll-paper-shade) 75%,transparent)}` +
    `[data-slot=item][data-variant=muted]{background-color:color-mix(in srgb,var(--ll-paper-shade) 65%,transparent);border-radius:6px}` +
    `[data-slot=item-group][data-ruled] [data-slot=item]::after{content:"";position:absolute;left:0;right:0;bottom:-${t / 2}px;height:${t}px;z-index:-1;pointer-events:none;background:var(--ll-rule-blue);opacity:.85;` +
    both('mask-image', 'var(--ll-it-r)') +
    both('mask-size', `${EDGE_LEN}px ${t}px`) +
    both('mask-repeat', 'repeat-x') +
    `}` +
    `[data-slot=item-separator][data-variant=pencil]{position:relative;isolation:isolate}` +
    `[data-slot=item-separator][data-variant=pencil]::before{content:"";position:absolute;left:0;right:0;top:50%;height:${t}px;margin-top:-${t / 2}px;z-index:-1;pointer-events:none;background:var(--ll-pencil);opacity:.75;` +
    both('mask-image', 'var(--ll-it-r)') +
    both('mask-size', `${EDGE_LEN}px ${t}px`) +
    both('mask-repeat', 'repeat-x') +
    `}` +
    `[data-density=calm] [data-slot=item-separator][data-variant=pencil]::before{opacity:.5}` +
    `}` +
    `@media (forced-colors:active){[data-slot=item][data-variant=outline]{outline:1px solid CanvasText;outline-offset:-1px}[data-slot=item-media][data-variant=icon]{outline:1px solid CanvasText}[data-slot=item-group][data-ruled] [data-slot=item]{border-bottom:1px solid GrayText}[data-slot=item-separator][data-variant=pencil]{background-color:GrayText}[data-slot=item][data-variant=outline]::before,[data-slot=item-media][data-variant=icon]::before,[data-slot=item-group][data-ruled] [data-slot=item]::after,[data-slot=item-separator][data-variant=pencil]::before{display:none}}`
  )
}
