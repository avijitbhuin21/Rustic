import { EDGE_TH, HATCH, circleTile, edgeTile, hatchTile } from '@/lib/ink/recipes/pencil-tiles-ink'

export const SKELETON_FRAMES = 3
export const SKELETON_BOIL_S = 1.08

const tileVar = (kind: 'eh' | 'ev' | 'ha' | 'ci', frame: number): string => `var(--ll-sk-${kind}${frame})`

let cache: string | undefined

const LAYER_ORDER = '@layer theme,base,components,utilities;'

export function skeletonCss(): string {
  return (cache ??= build())
}

function build(): string {
  const frames = Array.from({ length: SKELETON_FRAMES }, (_, f) => ({
    eh: edgeTile(`skeleton|edge-h|${f}`, false),
    ev: edgeTile(`skeleton|edge-v|${f}`, true),
    ha: hatchTile(`skeleton|hatch|${f}`),
    ci: circleTile(`skeleton|circle|${f}`),
  }))
  const vars = frames.map((f, i) => `--ll-sk-eh${i}:${f.eh};--ll-sk-ev${i}:${f.ev};--ll-sk-ha${i}:${f.ha};--ll-sk-ci${i}:${f.ci};`).join('')
  const step = (100 / SKELETON_FRAMES).toFixed(2)
  const outline = (f: number) => `${tileVar('eh', f)},${tileVar('eh', f)},${tileVar('ev', f)},${tileVar('ev', f)}`
  const frameRule = (name: string, prop: string, value: (f: number) => string) =>
    `@keyframes ${name}{${Array.from({ length: SKELETON_FRAMES }, (_, f) => `${f === 0 ? '0%' : `${(f * Number(step)).toFixed(2)}%`}{${prop}:${value(f)};-webkit-${prop}:${value(f)}}`).join('')}100%{${prop}:${value(0)};-webkit-${prop}:${value(0)}}}`
  const dur = `${SKELETON_BOIL_S}s steps(1,end) infinite`
  const end = `${EDGE_TH / 2 - 0.5}px`
  const span = `calc(100% - ${EDGE_TH - 1}px)`
  const edges = `${span} ${EDGE_TH}px,${span} ${EDGE_TH}px,${EDGE_TH}px ${span},${EDGE_TH}px ${span}`
  const at = `${end} 0,${end} 100%,0 ${end},100% ${end}`
  const still = `[data-slot=skeleton]::before,[data-slot=skeleton]::after{animation:none!important}`
  return (
    `${LAYER_ORDER}@layer components{` +
    `[data-slot=skeleton]{${vars}position:relative;isolation:isolate;background-color:color-mix(in srgb,var(--ll-paper-shade) 55%,transparent);animation-delay:calc(var(--ll-sk-phase,0)*-.36s)}` +
    `[data-slot=skeleton]::before,[data-slot=skeleton]::after{content:"";position:absolute;pointer-events:none}` +
    `[data-slot=skeleton]::after{inset:0;border-radius:inherit;z-index:-1;background:var(--ll-pencil-mark);opacity:.4;-webkit-mask-image:${tileVar('ha', 0)};mask-image:${tileVar('ha', 0)};-webkit-mask-size:${HATCH}px ${HATCH}px;mask-size:${HATCH}px ${HATCH}px;animation:ll-sk-hatch ${dur}}` +
    `[data-slot=skeleton]::before{inset:-${EDGE_TH / 2}px;z-index:-1;background:var(--ll-pencil);opacity:.78;` +
    `-webkit-mask-image:${outline(0)};mask-image:${outline(0)};` +
    `-webkit-mask-size:${edges};mask-size:${edges};` +
    `-webkit-mask-position:${at};mask-position:${at};-webkit-mask-repeat:no-repeat;mask-repeat:no-repeat;animation:ll-sk-outline ${dur}}` +
    `[data-slot=skeleton].rounded-full::before{inset:-3px;-webkit-mask-image:${tileVar('ci', 0)};mask-image:${tileVar('ci', 0)};-webkit-mask-size:100% 100%;mask-size:100% 100%;-webkit-mask-position:center;mask-position:center;-webkit-mask-repeat:no-repeat;mask-repeat:no-repeat;animation-name:ll-sk-circle}` +
    frameRule('ll-sk-hatch', 'mask-image', (f) => tileVar('ha', f)) +
    frameRule('ll-sk-outline', 'mask-image', outline) +
    frameRule('ll-sk-circle', 'mask-image', (f) => tileVar('ci', f)) +
    `[data-slot=skeleton]::before,[data-slot=skeleton]::after{animation-delay:inherit}` +
    `@media (prefers-reduced-motion:reduce){${still}}[data-motion=reduced] [data-slot=skeleton]::before,[data-motion=reduced] [data-slot=skeleton]::after,[data-density=calm] [data-slot=skeleton]::before,[data-density=calm] [data-slot=skeleton]::after{animation:none!important}` +
    `}` +
    `@media (forced-colors:active){[data-slot=skeleton]{outline:1px dashed GrayText;outline-offset:-1px}[data-slot=skeleton]::before,[data-slot=skeleton]::after{display:none}}`
  )
}
